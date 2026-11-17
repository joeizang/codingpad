//! The terminal runner behind `cpad run` (D28).
//!
//! The command reuses the desktop's Rust core: the same kernel supervisor,
//! warm Python/C# kernels, SQL manager, connection store and secret vault. It
//! only replaces the results panel with plain text on the terminal.
//!
//! A kernel resolves a dump as it runs, but lazy nodes (C# object properties,
//! values deeper than six levels) are only materialised by a later `expand`
//! request, and a kernel answers those after the run. The runner therefore
//! records the run's output, expands the lazy parts, and then writes everything
//! in its original order. This keeps dumps complete without changing the
//! kernel protocol the desktop relies on.

use codingpad_kernel_core::{
    databases::{ConnectionStore, SqlManager},
    secrets::SecretStore,
    text::render_dump,
    KernelEvent, Language, Supervisor,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    env, fs,
    future::Future,
    io::{self, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// Lazy values expanded after a run. Each request starts another six-level
/// view, so a cyclic object must not expand forever.
const MAX_EXPANSIONS: usize = 512;
/// Extra expansion rounds for nested lazy values.
const MAX_EXPANSION_DEPTH: usize = 3;

/// The script types `cpad run` understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptLanguage {
    Python,
    Csharp,
    Sql,
}

/// Where CodingPad keeps connections, secret names and private toolchains.
///
/// This is the desktop's Tauri `app_data_dir` (`io.github.joeizang.codingpad`),
/// so a terminal run uses the same connections and vault entries as the app.
/// `CODINGPAD_DATA_DIR` overrides it for portable setups and tests.
pub fn data_dir() -> PathBuf {
    if let Some(path) = env::var_os("CODINGPAD_DATA_DIR") {
        return PathBuf::from(path);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("io.github.joeizang.codingpad")
}

/// Pick a runner from the file extension.
pub fn language_for(path: &Path) -> Result<ScriptLanguage, String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "py" => Ok(ScriptLanguage::Python),
        "cs" => Ok(ScriptLanguage::Csharp),
        "sql" => Ok(ScriptLanguage::Sql),
        other => Err(format!(
            "Cannot run \"{}\": expected a .py, .cs or .sql script{}.",
            path.display(),
            if other.is_empty() {
                String::new()
            } else {
                format!(", not .{other}")
            }
        )),
    }
}

/// Run one script and return the process exit code.
///
/// Successful runs return 0, script failures return 1, stopped runs (including
/// Ctrl-C) return 130, and usage or script-type errors return 2.
pub async fn run_file(path: &Path) -> i32 {
    let absolute = match fs::canonicalize(path) {
        Ok(absolute) => absolute,
        Err(error) => {
            eprintln!("Could not open {}: {error}", path.display());
            return 1;
        }
    };
    let language = match language_for(path) {
        Ok(language) => language,
        Err(message) => {
            eprintln!("{message}");
            return 2;
        }
    };
    let source = match fs::read_to_string(&absolute) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("Could not read {}: {error}", absolute.display());
            return 1;
        }
    };
    let data_directory = data_dir();
    if let Err(error) = fs::create_dir_all(&data_directory) {
        eprintln!(
            "Could not create the CodingPad data folder {}: {error}",
            data_directory.display()
        );
        return 1;
    }

    let secrets = Arc::new(SecretStore::new(&data_directory));
    let connections = Arc::new(ConnectionStore::new(&data_directory));
    let sql = Arc::new(SqlManager::new(connections.clone(), secrets.clone()));
    let supervisor = Arc::new(
        Supervisor::with_data_dir_and_secrets(data_directory, secrets)
            .with_connections(connections),
    );

    let script_id = absolute.to_string_lossy().into_owned();
    let interrupted = Arc::new(AtomicBool::new(false));
    watch_interrupt(
        supervisor.clone(),
        sql.clone(),
        script_id.clone(),
        interrupted.clone(),
    );

    let mut output = Vec::new();
    let result = match language {
        ScriptLanguage::Python | ScriptLanguage::Csharp => {
            let kernel_language = match language {
                ScriptLanguage::Python => Language::Python,
                _ => Language::Csharp,
            };
            let script = &absolute;
            supervisor
                .run_language_at(
                    &script_id,
                    &source,
                    kernel_language,
                    Some(&script_id),
                    |event| record_event(&event, script, &mut output),
                )
                .await
                .map(|result| Outcome {
                    ok: result.ok,
                    stopped: result.stopped,
                })
        }
        ScriptLanguage::Sql => {
            run_sql_script(&sql, &script_id, &source, &absolute, &mut output).await
        }
    };

    // Lazy values need an idle kernel. Apply live updates first so each dump
    // renders the value the script left behind, as the desktop's block does.
    apply_updates(&mut output);
    expand_dumps(&supervisor, &script_id, &mut output).await;
    write_output(&output);

    // Close kernels before the process exits; a dropped runtime would
    // otherwise leave a warm child behind.
    supervisor.shutdown().await;
    sql.shutdown();

    let interrupted = interrupted.load(Ordering::SeqCst);
    match result {
        Ok(outcome) if outcome.ok => 0,
        Ok(outcome) if outcome.stopped || interrupted => 130,
        Ok(_) => 1,
        Err(message) => {
            eprintln!("{message}");
            if interrupted {
                130
            } else {
                1
            }
        }
    }
}

struct Outcome {
    ok: bool,
    stopped: bool,
}

async fn run_sql_script(
    sql: &SqlManager,
    script_id: &str,
    source: &str,
    path: &Path,
    output: &mut Vec<Recorded>,
) -> Result<Outcome, String> {
    let result = sql
        .run(script_id, source, Some(&path.to_string_lossy()))
        .await?;
    for table in &result.tables {
        output.push(Recorded::Dump(table.clone()));
    }
    Ok(Outcome {
        ok: result.ok,
        stopped: result.stopped,
    })
}

/// Ctrl-C stops the active run the same way the desktop's Stop button does.
/// A second Ctrl-C exits immediately in case a script refuses to stop.
fn watch_interrupt(
    supervisor: Arc<Supervisor>,
    sql: Arc<SqlManager>,
    script_id: String,
    interrupted: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        let mut stopped = false;
        loop {
            if tokio::signal::ctrl_c().await.is_err() {
                return;
            }
            interrupted.store(true, Ordering::SeqCst);
            if stopped {
                std::process::exit(130);
            }
            stopped = true;
            sql.stop(&script_id);
            supervisor.stop(&script_id).await;
        }
    });
}

#[derive(Debug)]
enum Recorded {
    Stdout(String),
    Stderr(String),
    Dump(Value),
    Update { node_id: String, node: Value },
}

fn record_event(event: &KernelEvent, path: &Path, output: &mut Vec<Recorded>) {
    match event.method.as_str() {
        "stream" => {
            let stream = event.params["stream"].as_str().unwrap_or("stdout");
            let text = event.params["text"].as_str().unwrap_or("");
            if stream == "stderr" {
                output.push(Recorded::Stderr(text.to_owned()));
            } else {
                output.push(Recorded::Stdout(text.to_owned()));
            }
        }
        "dump" => output.push(Recorded::Dump(event.params["node"].clone())),
        "dump_update" => {
            if let (Some(node_id), Some(node)) = (
                event.params["nodeId"].as_str(),
                event.params.get("node").filter(|node| !node.is_null()),
            ) {
                output.push(Recorded::Update {
                    node_id: node_id.to_owned(),
                    node: node.clone(),
                });
            }
        }
        "diagnostics" => {
            if let Some(text) = diagnostics_text(&event.params["diagnostics"], path) {
                output.push(Recorded::Stderr(text));
            }
        }
        _ => {}
    }
}

fn apply_updates(output: &mut [Recorded]) {
    let updates: HashMap<String, Value> = output
        .iter()
        .filter_map(|record| match record {
            Recorded::Update { node_id, node } => Some((node_id.clone(), node.clone())),
            _ => None,
        })
        .collect();
    if updates.is_empty() {
        return;
    }
    for record in output {
        if let Recorded::Dump(node) = record {
            replace_updates(node, &updates);
        }
    }
}

fn replace_updates(node: &mut Value, updates: &HashMap<String, Value>) {
    if let Some(updated) = node
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| updates.get(id))
    {
        *node = updated.clone();
    }
    match node.get("kind").and_then(Value::as_str) {
        Some("live") => {
            if let Some(inner) = node.get_mut("node") {
                replace_updates(inner, updates);
            }
        }
        Some("list") => {
            if let Some(items) = node.get_mut("items").and_then(Value::as_array_mut) {
                for item in items {
                    replace_updates(item, updates);
                }
            }
        }
        Some("object") => {
            if let Some(fields) = node.get_mut("fields").and_then(Value::as_array_mut) {
                for field in fields {
                    if let Some(value) = field.get_mut("value") {
                        replace_updates(value, updates);
                    }
                }
            }
        }
        Some("table") => {
            if let Some(rows) = node.get_mut("rows").and_then(Value::as_array_mut) {
                for row in rows {
                    if let Some(cells) = row.as_array_mut() {
                        for cell in cells {
                            replace_updates(cell, updates);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

fn diagnostics_text(diagnostics: &Value, path: &Path) -> Option<String> {
    let diagnostics = diagnostics.as_array()?;
    if diagnostics.is_empty() {
        return None;
    }
    let mut text = String::new();
    for diagnostic in diagnostics {
        text.push_str(&format!(
            "{}:{}:{}: {}\n",
            path.display(),
            diagnostic["line"].as_u64().unwrap_or(0),
            diagnostic["column"].as_u64().unwrap_or(0),
            diagnostic["message"]
                .as_str()
                .unwrap_or("Compilation failed"),
        ));
    }
    Some(text)
}

async fn expand_dumps(supervisor: &Supervisor, script_id: &str, output: &mut [Recorded]) {
    let mut budget = MAX_EXPANSIONS;
    for record in output {
        if let Recorded::Dump(node) = record {
            expand_node(supervisor, script_id, node, 0, &mut budget).await;
        }
    }
}

fn expand_node<'a>(
    supervisor: &'a Supervisor,
    script_id: &'a str,
    node: &'a mut Value,
    depth: usize,
    budget: &'a mut usize,
) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        if depth > MAX_EXPANSION_DEPTH || *budget == 0 {
            return;
        }
        // Copy the kind so the borrow ends before a child is borrowed mutably.
        let kind = node.get("kind").and_then(Value::as_str).map(str::to_owned);
        match kind.as_deref() {
            Some("lazy") => {
                let Some(handle) = node
                    .get("handle")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                else {
                    return;
                };
                *budget -= 1;
                if let Ok(result) = supervisor.expand(script_id, &handle, None, Some(100)).await {
                    *node = result.node;
                    expand_node(supervisor, script_id, node, depth + 1, budget).await;
                }
            }
            Some("live") => {
                if let Some(inner) = node.get_mut("node") {
                    expand_node(supervisor, script_id, inner, depth, budget).await;
                }
            }
            Some("list") => {
                if let Some(items) = node.get_mut("items").and_then(Value::as_array_mut) {
                    for item in items {
                        expand_node(supervisor, script_id, item, depth, budget).await;
                    }
                }
            }
            Some("object") => {
                if let Some(fields) = node.get_mut("fields").and_then(Value::as_array_mut) {
                    for field in fields {
                        if let Some(value) = field.get_mut("value") {
                            expand_node(supervisor, script_id, value, depth, budget).await;
                        }
                    }
                }
            }
            Some("table") => {
                if let Some(rows) = node.get_mut("rows").and_then(Value::as_array_mut) {
                    for row in rows {
                        if let Some(cells) = row.as_array_mut() {
                            for cell in cells {
                                expand_node(supervisor, script_id, cell, depth, budget).await;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    })
}

fn write_output(output: &[Recorded]) {
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    for record in output {
        match record {
            Recorded::Stdout(text) => {
                let _ = stdout.write_all(text.as_bytes());
                let _ = stdout.flush();
            }
            Recorded::Stderr(text) => {
                let _ = stderr.write_all(text.as_bytes());
                let _ = stderr.flush();
            }
            Recorded::Dump(node) => {
                let mut text = render_dump(node);
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                let _ = stdout.write_all(text.as_bytes());
                let _ = stdout.flush();
            }
            // Updates were folded into the dumps before this pass.
            Recorded::Update { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{language_for, ScriptLanguage};
    use std::path::Path;

    #[test]
    fn extensions_pick_the_runner() {
        assert_eq!(
            language_for(Path::new("hello.py")),
            Ok(ScriptLanguage::Python)
        );
        assert_eq!(
            language_for(Path::new("Hello.CS")),
            Ok(ScriptLanguage::Csharp)
        );
        assert_eq!(
            language_for(Path::new("query.sql")),
            Ok(ScriptLanguage::Sql)
        );
    }

    #[test]
    fn unknown_extensions_are_rejected_with_the_expected_ones() {
        let message = language_for(Path::new("notes.txt")).unwrap_err();
        assert!(message.contains(".py, .cs or .sql"));
        assert!(message.contains("notes.txt"));
    }
}
