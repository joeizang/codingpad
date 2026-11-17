//! The terminal runner behind `cpad run` (D28).
//!
//! The command reuses the desktop's Rust core: the same kernel supervisor,
//! warm Python/C# kernels, SQL manager, connection store and secret vault. It
//! only replaces the results panel with plain text on the terminal.
//!
//! Kernels normally defer C# properties and deep values as lazy nodes so the
//! desktop can expand them on click. A terminal cannot click, so the runner
//! asks kernels for eager dumps. Events stream as they arrive; a dump that still
//! contains a live value or a lazy node is held back until the run finishes,
//! then updated or expanded within a bounded time budget, and printed in order.

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
    time::{Duration, Instant},
};

/// Lazy values expanded after a run. Each request starts another six-level
/// view, so a cyclic object must not expand forever.
const MAX_EXPANSIONS: usize = 512;
/// Extra expansion rounds for nested lazy values.
const MAX_EXPANSION_DEPTH: usize = 3;
/// Overall time spent resolving values a kernel left lazy.
const EXPANSION_BUDGET: Duration = Duration::from_secs(10);
/// One expansion matches the desktop's preview timeout.
const EXPANSION_CALL_TIMEOUT: Duration = Duration::from_secs(10);

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
    // Check the type before touching the disk: `cpad run notes.txt` is a usage
    // error even when that file does not exist.
    let language = match language_for(path) {
        Ok(language) => language,
        Err(message) => {
            eprintln!("{message}");
            return 2;
        }
    };
    // `dunce` removes Windows' `\\?\` prefix, so the script id and compiler
    // diagnostics stay in the form editors and NuGet discovery understand.
    let absolute = match dunce::canonicalize(path) {
        Ok(absolute) => absolute,
        Err(error) => {
            eprintln!("Could not open {}: {error}", path.display());
            return 1;
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
            .with_connections(connections)
            .with_eager_dumps(),
    );

    let script_id = absolute.to_string_lossy().into_owned();
    let interrupted = Arc::new(AtomicBool::new(false));
    watch_interrupt(
        supervisor.clone(),
        sql.clone(),
        script_id.clone(),
        interrupted.clone(),
    );

    let mut output = Output::default();
    let result = match language {
        ScriptLanguage::Python | ScriptLanguage::Csharp => {
            let kernel_language = match language {
                ScriptLanguage::Python => Language::Python,
                _ => Language::Csharp,
            };
            // The user's path keeps diagnostics short and clickable.
            let script = path;
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

    // A dump with a live value or a lazy node was held back. Apply the final
    // live values, then expand what the kernel could not make eager.
    if output.buffering {
        apply_updates(&mut output.events);
        expand_dumps(&supervisor, &script_id, &mut output.events).await;
        for record in &output.events {
            write_recorded(record);
        }
    }

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
    output: &mut Output,
) -> Result<Outcome, String> {
    let result = sql
        .run(script_id, source, Some(&path.to_string_lossy()))
        .await?;
    for table in &result.tables {
        push_dump(table.clone(), output);
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

/// Streamed events go straight to the terminal. Once one event needs
/// post-run work, later events are held so output order is preserved.
#[derive(Default)]
struct Output {
    events: Vec<Recorded>,
    buffering: bool,
}

fn record_event(event: &KernelEvent, path: &Path, output: &mut Output) {
    match event.method.as_str() {
        "stream" => {
            let stream = event.params["stream"].as_str().unwrap_or("stdout");
            let text = event.params["text"].as_str().unwrap_or("");
            let record = if stream == "stderr" {
                Recorded::Stderr(text.to_owned())
            } else {
                Recorded::Stdout(text.to_owned())
            };
            emit(record, output);
        }
        "dump" => push_dump(event.params["node"].clone(), output),
        "dump_update" => {
            if let (Some(node_id), Some(node)) = (
                event.params["nodeId"].as_str(),
                event.params.get("node").filter(|node| !node.is_null()),
            ) {
                emit(
                    Recorded::Update {
                        node_id: node_id.to_owned(),
                        node: node.clone(),
                    },
                    output,
                );
            }
        }
        "diagnostics" => {
            if let Some(text) = diagnostics_text(&event.params["diagnostics"], path) {
                emit(Recorded::Stderr(text), output);
            }
        }
        _ => {}
    }
}

fn emit(record: Recorded, output: &mut Output) {
    if output.buffering {
        output.events.push(record);
    } else {
        write_recorded(&record);
    }
}

fn push_dump(mut node: Value, output: &mut Output) {
    if output.buffering || needs_post_processing(&mut node) {
        output.buffering = true;
        output.events.push(Recorded::Dump(node));
    } else {
        write_recorded(&Recorded::Dump(node));
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

/// Walk the children of one dump node. Shared by update replacement, lazy
/// detection and expansion so a new container kind is handled in one place.
fn child_nodes_mut(node: &mut Value) -> Vec<&mut Value> {
    let kind = node.get("kind").and_then(Value::as_str).map(str::to_owned);
    let mut children = Vec::new();
    match kind.as_deref() {
        Some("live") => {
            if let Some(inner) = node.get_mut("node") {
                children.push(inner);
            }
        }
        Some("list") => {
            if let Some(items) = node.get_mut("items").and_then(Value::as_array_mut) {
                children.extend(items.iter_mut());
            }
        }
        Some("object") => {
            if let Some(fields) = node.get_mut("fields").and_then(Value::as_array_mut) {
                children.extend(fields.iter_mut().filter_map(|field| field.get_mut("value")));
            }
        }
        Some("table") => {
            if let Some(rows) = node.get_mut("rows").and_then(Value::as_array_mut) {
                for row in rows {
                    if let Some(cells) = row.as_array_mut() {
                        children.extend(cells.iter_mut());
                    }
                }
            }
        }
        _ => {}
    }
    children
}

fn needs_post_processing(node: &mut Value) -> bool {
    if matches!(
        node.get("kind").and_then(Value::as_str),
        Some("lazy" | "live")
    ) {
        return true;
    }
    for child in child_nodes_mut(node) {
        if needs_post_processing(child) {
            return true;
        }
    }
    false
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
    for child in child_nodes_mut(node) {
        replace_updates(child, updates);
    }
}

#[derive(Default)]
struct ExpandState {
    deadline: Option<Instant>,
    remaining: usize,
    expanded: usize,
    failed: bool,
}

impl ExpandState {
    fn new() -> Self {
        Self {
            deadline: Some(Instant::now() + EXPANSION_BUDGET),
            remaining: MAX_EXPANSIONS,
            expanded: 0,
            failed: false,
        }
    }

    fn out_of_time(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }
}

async fn expand_dumps(supervisor: &Supervisor, script_id: &str, output: &mut [Recorded]) {
    let lazy = output
        .iter_mut()
        .filter_map(|record| match record {
            Recorded::Dump(node) => Some(count_lazy(node)),
            _ => None,
        })
        .sum::<usize>();
    if lazy == 0 {
        return;
    }
    eprintln!("Expanding {lazy} lazy value(s)…");
    let mut state = ExpandState::new();
    for record in output {
        if let Recorded::Dump(node) = record {
            expand_node(supervisor, script_id, node, 0, &mut state).await;
        }
    }
    if state.expanded > 0 {
        eprintln!("Expanded {} lazy value(s).", state.expanded);
    }
    if state.failed {
        eprintln!("An expansion failed; some values stay collapsed.");
    } else if state.out_of_time() || state.remaining == 0 {
        eprintln!(
            "Stopped expanding after {} seconds; remaining values stay collapsed.",
            EXPANSION_BUDGET.as_secs()
        );
    }
}

fn count_lazy(node: &mut Value) -> usize {
    let mut count = usize::from(node.get("kind").and_then(Value::as_str) == Some("lazy"));
    for child in child_nodes_mut(node) {
        count += count_lazy(child);
    }
    count
}

fn expand_node<'a>(
    supervisor: &'a Supervisor,
    script_id: &'a str,
    node: &'a mut Value,
    depth: usize,
    state: &'a mut ExpandState,
) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        if depth > MAX_EXPANSION_DEPTH
            || state.failed
            || state.remaining == 0
            || state.out_of_time()
        {
            return;
        }
        if node.get("kind").and_then(Value::as_str) == Some("lazy") {
            let Some(handle) = node
                .get("handle")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                return;
            };
            state.remaining -= 1;
            let timeout = state
                .deadline
                .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                .unwrap_or(EXPANSION_CALL_TIMEOUT)
                .min(EXPANSION_CALL_TIMEOUT);
            if timeout.is_zero() {
                return;
            }
            match supervisor
                .expand_within(script_id, &handle, None, Some(100), timeout)
                .await
            {
                Ok(result) => {
                    *node = result.node;
                    state.expanded += 1;
                    expand_node(supervisor, script_id, node, depth + 1, state).await;
                }
                // A timeout drops the kernel, so further handles would fail too.
                Err(_) => state.failed = true,
            }
            return;
        }
        for child in child_nodes_mut(node) {
            expand_node(supervisor, script_id, child, depth, state).await;
        }
    })
}

fn write_recorded(record: &Recorded) {
    match record {
        Recorded::Stdout(text) => {
            let mut stdout = io::stdout().lock();
            let _ = stdout.write_all(text.as_bytes());
            let _ = stdout.flush();
        }
        Recorded::Stderr(text) => {
            let mut stderr = io::stderr().lock();
            let _ = stderr.write_all(text.as_bytes());
            let _ = stderr.flush();
        }
        Recorded::Dump(node) => {
            let mut text = render_dump(node);
            if !text.ends_with('\n') {
                text.push('\n');
            }
            let mut stdout = io::stdout().lock();
            let _ = stdout.write_all(text.as_bytes());
            let _ = stdout.flush();
        }
        // Updates were folded into the dumps before this pass.
        Recorded::Update { .. } => {}
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
