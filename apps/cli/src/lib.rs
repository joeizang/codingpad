//! The terminal runner behind `cpad run` (D28).
//!
//! The command reuses the desktop's Rust core: the same kernel supervisor,
//! Python, C#, TypeScript and Rust kernels, SQL manager, connection store and
//! secret vault. It replaces the results panel with terminal output.
//!
//! Kernels normally defer C# properties and deep values as lazy nodes so the
//! desktop can expand them on click. A terminal cannot click, so the runner
//! asks kernels for eager dumps. Events stream as they arrive; live updates are
//! reported as they happen, and only a dump that still contains a lazy node is
//! held back. That dump is expanded within a bounded time budget and printed in
//! its original position.
//!
//! `--html` changes the shape of the output, not the run: every event is held
//! until the script finishes so live updates can replace their dumps in place,
//! then one standalone page is printed.

use codingpad_kernel_core::{
    databases::{ConnectionStore, SqlManager},
    html,
    secrets::{OsVault, SecretStore, Vault},
    text::render_dump,
    KernelEvent, Language, Supervisor,
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    future::Future,
    io::{self, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
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
/// Do not start a call that has almost no budget left: a short timeout would
/// discard the kernel and be reported as a failure instead of an expired budget.
const MIN_EXPANSION_CALL: Duration = Duration::from_millis(250);
/// Bounded output held only when a dump needs post-run expansion.
const MAX_HELD_EVENTS: usize = 500;
/// Live progress lines printed before updates are suppressed.
const MAX_LIVE_UPDATES: usize = 100;
/// How long the second Ctrl-C waits for kernels to close before a hard exit.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// The script types `cpad run` understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptLanguage {
    Python,
    Csharp,
    Typescript,
    Rust,
    Sql,
}

/// How the run prints its dumps (D28).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputFormat {
    /// Readable plain text on stdout, streamed as the script runs.
    #[default]
    Text,
    /// One standalone HTML page on stdout, written when the run finishes.
    Html,
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
        ext if ext == Language::Python.extension() => Ok(ScriptLanguage::Python),
        ext if ext == Language::Csharp.extension() => Ok(ScriptLanguage::Csharp),
        ext if ext == Language::Typescript.extension() => Ok(ScriptLanguage::Typescript),
        ext if ext == Language::Rust.extension() => Ok(ScriptLanguage::Rust),
        "sql" => Ok(ScriptLanguage::Sql),
        ext => Err(format!(
            "Cannot run \"{}\": expected a .py, .cs, .ts, .rs or .sql script{}.",
            path.display(),
            if ext.is_empty() {
                String::new()
            } else {
                format!(", not .{ext}")
            }
        )),
    }
}

/// Run one script and return the process exit code, printing dumps as text.
pub async fn run_file(path: &Path) -> i32 {
    run_file_with_format(path, OutputFormat::Text).await
}

/// Run one script and return the process exit code.
///
/// Successful runs return 0, script failures return 1, stopped runs (including
/// Ctrl-C) return 130, and usage or script-type errors return 2.
pub async fn run_file_with_format(path: &Path, format: OutputFormat) -> i32 {
    run_file_in(path, format, data_dir(), Arc::new(OsVault)).await
}

async fn run_file_in(
    path: &Path,
    format: OutputFormat,
    data_directory: PathBuf,
    vault: Arc<dyn Vault>,
) -> i32 {
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
    if let Err(error) = fs::create_dir_all(&data_directory) {
        eprintln!(
            "Could not create the CodingPad data folder {}: {error}",
            data_directory.display()
        );
        return 1;
    }

    let secrets = Arc::new(SecretStore::with_vault(&data_directory, vault));
    let connections = Arc::new(ConnectionStore::new(&data_directory));
    if let Err(error) = codingpad_kernel_core::samples::ensure_database(&data_directory) {
        eprintln!("cpad could not prepare the sample database: {error}");
    }
    let sql = Arc::new(SqlManager::new(connections.clone(), secrets.clone()));
    let supervisor = Arc::new(
        Supervisor::with_data_dir_and_secrets(data_directory, secrets)
            .with_connections(connections)
            .with_eager_dumps(),
    );

    let script_id = absolute.to_string_lossy().into_owned();
    let title = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let html = (format == OutputFormat::Html).then(|| Arc::new(Mutex::new(HtmlReport::new(title))));
    let interrupted = Arc::new(AtomicBool::new(false));
    watch_interrupt(
        supervisor.clone(),
        sql.clone(),
        script_id.clone(),
        interrupted.clone(),
        html.clone(),
    );

    let mut recorder = match &html {
        Some(report) => Recorder::Html(report.clone()),
        None => Recorder::Text(Output::default()),
    };
    let mut run_kernel = async |kernel_language| {
        // Use the same header connection and db client as the desktop,
        // including generated data models for Python and C#.
        let (data, data_error) =
            codingpad_kernel_core::attached_data_context(&sql, &source, kernel_language).await;
        if let Some(error) = data_error {
            recorder.stderr(format!("{error}\n"));
        }
        // The user's path keeps diagnostics short and clickable.
        supervisor
            .run_language_at_with_data(
                &script_id,
                &source,
                kernel_language,
                Some(&script_id),
                data,
                |event| recorder.record(&event, path),
            )
            .await
            .map(|result| Outcome {
                ok: result.ok,
                stopped: result.stopped,
            })
    };
    let result = match language {
        ScriptLanguage::Python => run_kernel(Language::Python).await,
        ScriptLanguage::Csharp => run_kernel(Language::Csharp).await,
        ScriptLanguage::Typescript => run_kernel(Language::Typescript).await,
        ScriptLanguage::Rust => run_kernel(Language::Rust).await,
        ScriptLanguage::Sql => {
            run_sql_script(&sql, &script_id, &source, &absolute, &mut recorder).await
        }
    };

    // A dump with a lazy node was held back. Apply any buffered live updates,
    // expand what the kernel could not make eager, then print in order. HTML
    // collects the whole run, so its page is rendered once here.
    match &mut recorder {
        Recorder::Text(output) => {
            if output.buffering {
                apply_updates(&mut output.events);
                expand_dumps(&supervisor, &script_id, &mut output.events).await;
                flush_text(output);
            }
        }
        Recorder::Html(report) => {
            // Keep the collected events in the report while expanding, so a
            // forced exit can still write a page with the current previews.
            let (mut events, updates) = report
                .lock()
                .map(|report| (report.events.clone(), report.updates.clone()))
                .unwrap_or_default();
            apply_update_map(&mut events, &updates);
            expand_dumps(&supervisor, &script_id, &mut events).await;
            // Write under the report lock so a forced exit either writes the
            // previews first or waits and finds the finished page already out.
            if let Ok(mut report) = report.lock() {
                if !report.written {
                    let title = report.title.clone();
                    write_html_page(&title, &events);
                    report.written = true;
                }
            }
        }
    }

    // Close kernels before the process exits; a dropped runtime would
    // otherwise leave a warm child behind.
    supervisor.shutdown().await;
    sql.shutdown();

    let interrupted = interrupted.load(Ordering::SeqCst);
    if let Err(message) = &result {
        if !interrupted {
            eprintln!("{message}");
        }
    }
    if interrupted {
        // Ctrl-C during expansion still means the user stopped the run.
        return 130;
    }
    match result {
        Ok(outcome) if outcome.ok => 0,
        Ok(outcome) if outcome.stopped => 130,
        Ok(_) => 1,
        Err(_) => 1,
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
    recorder: &mut Recorder,
) -> Result<Outcome, String> {
    let result = sql
        .run(script_id, source, Some(&path.to_string_lossy()))
        .await?;
    for table in &result.tables {
        recorder.dump(table.clone());
    }
    Ok(Outcome {
        ok: result.ok,
        stopped: result.stopped,
    })
}

/// Ctrl-C stops the active run the same way the desktop's Stop button does.
/// A second Ctrl-C closes the kernels, still letting the main task print held
/// output and return exit code 130. A third, or a shutdown that stalls, forces
/// an exit once the kernels have had their chance to close.
fn watch_interrupt(
    supervisor: Arc<Supervisor>,
    sql: Arc<SqlManager>,
    script_id: String,
    interrupted: Arc<AtomicBool>,
    html: Option<Arc<Mutex<HtmlReport>>>,
) {
    tokio::spawn(async move {
        let mut stage = 0;
        loop {
            if tokio::signal::ctrl_c().await.is_err() {
                return;
            }
            interrupted.store(true, Ordering::SeqCst);
            sql.stop(&script_id);
            match stage {
                0 => {
                    stage = 1;
                    supervisor.stop(&script_id).await;
                }
                1 => {
                    stage = 2;
                    let shutdown = supervisor.shutdown();
                    tokio::pin!(shutdown);
                    tokio::select! {
                        _ = &mut shutdown => {}
                        _ = tokio::time::sleep(SHUTDOWN_GRACE) => {
                            force_exit(&html);
                        }
                        _ = tokio::signal::ctrl_c() => {
                            force_exit(&html);
                        }
                    }
                }
                // Kernels are closed; a further Ctrl-C must still end the process.
                _ => force_exit(&html),
            }
        }
    });
}

/// Write whatever the HTML report has collected, then exit. Text output was
/// already streamed by the main task, so only HTML needs this last chance.
fn force_exit(html: &Option<Arc<Mutex<HtmlReport>>>) -> ! {
    if let Some(report) = html {
        flush_html_report(report);
    }
    std::process::exit(130);
}

#[derive(Clone, Debug)]
enum Recorded {
    Stdout(String),
    Stderr(String),
    Dump(Value),
    Update { node_id: String, node: Value },
}

/// Text streams go straight to the terminal. Once a dump needs post-run
/// expansion, later events are held so output order is preserved. The holder is
/// bounded: reaching the cap flushes what is held without further expansion.
#[derive(Default)]
struct Output {
    events: Vec<Recorded>,
    buffering: bool,
    /// Dump node ids held in `events`; updates targeting them must be applied
    /// there, while updates for already printed live values still report now.
    held_ids: HashSet<String>,
    live_updates: usize,
}

/// Every event of one HTML run. Unlike the text holder nothing is printed
/// before the end: all dumps are kept so live updates can replace them in
/// place, and a forced exit can still render the report.
///
/// Live updates are folded into `updates` as they arrive, keeping only the
/// latest node per id. A progress value updated 100k times therefore costs one
/// entry, not 100k `Recorded::Update` records.
struct HtmlReport {
    title: String,
    events: Vec<Recorded>,
    updates: HashMap<String, Value>,
    written: bool,
}

impl HtmlReport {
    fn new(title: String) -> Self {
        Self {
            title,
            events: Vec::new(),
            updates: HashMap::new(),
            written: false,
        }
    }
}

/// A run's output destination. HTML gets its own sink because it cannot update
/// a block that has already been printed, while text streams as it goes.
enum Recorder {
    Text(Output),
    Html(Arc<Mutex<HtmlReport>>),
}

impl Recorder {
    fn record(&mut self, event: &KernelEvent, path: &Path) {
        match self {
            Recorder::Text(output) => record_event(event, path, output),
            Recorder::Html(report) => record_event_html(event, path, report),
        }
    }

    fn dump(&mut self, node: Value) {
        match self {
            Recorder::Text(output) => push_dump(node, output),
            Recorder::Html(report) => push_html(report, Recorded::Dump(node)),
        }
    }

    /// Report a host message. HTML keeps it in the page and also mirrors it to
    /// the terminal, so a failed run explains itself without opening the file.
    fn stderr(&mut self, text: String) {
        match self {
            Recorder::Text(output) => emit(Recorded::Stderr(text), output),
            Recorder::Html(report) => {
                write_terminal_stderr(&text);
                push_html(report, Recorded::Stderr(text));
            }
        }
    }
}

fn push_html(report: &Arc<Mutex<HtmlReport>>, record: Recorded) {
    if let Ok(mut report) = report.lock() {
        report.events.push(record);
    }
}

fn record_event_html(event: &KernelEvent, path: &Path, report: &Arc<Mutex<HtmlReport>>) {
    match event.method.as_str() {
        "stream" => {
            let stderr = event.params["stream"].as_str() == Some("stderr");
            let text = event.params["text"].as_str().unwrap_or("").to_owned();
            let record = if stderr {
                write_terminal_stderr(&text);
                Recorded::Stderr(text)
            } else {
                Recorded::Stdout(text)
            };
            push_html(report, record);
        }
        "dump" => {
            let node = event.params["node"].clone();
            // A runtime error arrives as a dump, not as stderr. Mirror its
            // one-line summary to the terminal so a failed run explains itself
            // without opening the saved page.
            if node.get("kind").and_then(Value::as_str) == Some("error") {
                let kind = node.get("type").and_then(Value::as_str).unwrap_or("Error");
                let message = node.get("message").and_then(Value::as_str).unwrap_or("");
                write_terminal_stderr(&format!("{kind}: {message}\n"));
            }
            push_html(report, Recorded::Dump(node));
        }
        "dump_update" => {
            let (Some(node_id), Some(node)) = (
                event.params["nodeId"].as_str(),
                event.params.get("node").filter(|node| !node.is_null()),
            ) else {
                return;
            };
            // Keep only the latest node per id: a live value updated 100k
            // times must not keep 100k copies until the run ends.
            if let Ok(mut report) = report.lock() {
                report.updates.insert(node_id.to_owned(), node.clone());
            }
        }
        "diagnostics" => {
            if let Some(text) = diagnostics_text(&event.params["diagnostics"], path) {
                write_terminal_stderr(&text);
                push_html(report, Recorded::Stderr(text));
            }
        }
        _ => {}
    }
}

fn write_terminal_stderr(text: &str) {
    let mut stderr = io::stderr().lock();
    let _ = stderr.write_all(text.as_bytes());
    let _ = stderr.flush();
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
            let (Some(node_id), Some(node)) = (
                event.params["nodeId"].as_str(),
                event.params.get("node").filter(|node| !node.is_null()),
            ) else {
                return;
            };
            if output.buffering && output.held_ids.contains(node_id) {
                hold(
                    Recorded::Update {
                        node_id: node_id.to_owned(),
                        node: node.clone(),
                    },
                    output,
                );
            } else {
                // The live value was already printed (or its dump streamed),
                // so its update cannot be applied to held output.
                report_live_update(node, output);
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
        hold(record, output);
    } else {
        write_recorded(&record);
    }
}

fn push_dump(mut node: Value, output: &mut Output) {
    if output.buffering || needs_expansion(&mut node) {
        output.buffering = true;
        hold(Recorded::Dump(node), output);
    } else {
        write_recorded(&Recorded::Dump(node));
    }
}

fn hold(mut record: Recorded, output: &mut Output) {
    match &mut record {
        Recorded::Dump(node) => collect_ids(node, &mut output.held_ids),
        Recorded::Update { node, .. } => collect_ids(node, &mut output.held_ids),
        _ => {}
    }
    output.events.push(record);
    if output.events.len() < MAX_HELD_EVENTS {
        return;
    }
    // A long stream after an expandable dump must not grow without bound.
    // Fold in the updates already held, print the previews as they are and go
    // back to streaming.
    eprintln!("Output buffer full; printing held values without further expansion.");
    apply_updates(&mut output.events);
    for record in &output.events {
        write_recorded(record);
    }
    output.events.clear();
    output.held_ids.clear();
    output.buffering = false;
}

fn collect_ids(node: &mut Value, ids: &mut HashSet<String>) {
    if let Some(id) = node.get("id").and_then(Value::as_str) {
        ids.insert(id.to_owned());
    }
    for child in child_nodes_mut(node) {
        collect_ids(child, ids);
    }
}

/// A terminal cannot update a live block in place, so report its new value as a
/// progress line on stderr. The initial dump was already printed on stdout.
fn report_live_update(node: &Value, output: &mut Output) {
    if output.live_updates >= MAX_LIVE_UPDATES {
        if output.live_updates == MAX_LIVE_UPDATES {
            eprintln!("Further live updates suppressed.");
            output.live_updates += 1;
        }
        return;
    }
    output.live_updates += 1;
    let mut stderr = io::stderr().lock();
    let _ = writeln!(stderr, "live: {}", render_dump(node));
    let _ = stderr.flush();
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

fn needs_expansion(node: &mut Value) -> bool {
    if node.get("kind").and_then(Value::as_str) == Some("lazy") {
        return true;
    }
    for child in child_nodes_mut(node) {
        if needs_expansion(child) {
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
    apply_update_map(output, &updates);
}

/// Fold a map of the latest node per live id into the held dumps. HTML keeps
/// its updates in a map instead of `Recorded::Update` records.
fn apply_update_map(output: &mut [Recorded], updates: &HashMap<String, Value>) {
    if updates.is_empty() {
        return;
    }
    for record in output {
        if let Recorded::Dump(node) = record {
            replace_updates(node, updates);
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
    timed_out: bool,
    limit_reached: bool,
}

impl ExpandState {
    fn new() -> Self {
        Self {
            deadline: Some(Instant::now() + EXPANSION_BUDGET),
            remaining: MAX_EXPANSIONS,
            expanded: 0,
            failed: false,
            timed_out: false,
            limit_reached: false,
        }
    }

    fn out_of_time(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn stopped(&self) -> bool {
        self.failed || self.timed_out || self.limit_reached
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
    for record in output.iter_mut() {
        if let Recorded::Dump(node) = record {
            expand_node(supervisor, script_id, node, 0, &mut state).await;
        }
    }
    if state.expanded > 0 {
        eprintln!("Expanded {} lazy value(s).", state.expanded);
    }
    let leftover = output
        .iter_mut()
        .filter_map(|record| match record {
            Recorded::Dump(node) => Some(count_lazy(node)),
            _ => None,
        })
        .sum::<usize>();
    if leftover == 0 {
        return;
    }
    if state.failed {
        eprintln!("An expansion failed; remaining values stay collapsed.");
    } else if state.timed_out {
        eprintln!(
            "Stopped expanding after {} seconds; remaining values stay collapsed.",
            EXPANSION_BUDGET.as_secs()
        );
    } else if state.limit_reached {
        eprintln!(
            "Stopped after expanding {MAX_EXPANSIONS} values; remaining values stay collapsed."
        );
    } else {
        eprintln!("{leftover} value(s) stay collapsed.");
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
        if depth > MAX_EXPANSION_DEPTH || state.stopped() {
            return;
        }
        if node.get("kind").and_then(Value::as_str) == Some("lazy") {
            if state.out_of_time() {
                state.timed_out = true;
                return;
            }
            if state.remaining == 0 {
                state.limit_reached = true;
                return;
            }
            let Some(handle) = node
                .get("handle")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                return;
            };
            let remaining = state
                .deadline
                .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                .unwrap_or(EXPANSION_CALL_TIMEOUT);
            if remaining < MIN_EXPANSION_CALL {
                state.timed_out = true;
                return;
            }
            state.remaining -= 1;
            let timeout = remaining.min(EXPANSION_CALL_TIMEOUT);
            match supervisor
                .expand_within(script_id, &handle, None, Some(100), timeout)
                .await
            {
                Ok(result) => {
                    *node = result.node;
                    state.expanded += 1;
                    expand_node(supervisor, script_id, node, depth + 1, state).await;
                }
                // Running out of budget drops the kernel; an actual kernel
                // error leaves time for other handles.
                Err(_) if state.out_of_time() => state.timed_out = true,
                Err(_) => state.failed = true,
            }
            return;
        }
        if state.out_of_time() {
            state.timed_out = true;
            return;
        }
        if state.remaining == 0 {
            state.limit_reached = true;
            return;
        }
        for child in child_nodes_mut(node) {
            expand_node(supervisor, script_id, child, depth, state).await;
        }
    })
}

/// Print held text events in their original order.
fn flush_text(output: &mut Output) {
    for record in &output.events {
        write_recorded(record);
    }
    output.events.clear();
    output.held_ids.clear();
    output.buffering = false;
}

/// Last-chance page for a forced exit: apply the updates collected so far and
/// write the report without expanding lazy nodes.
fn flush_html_report(report: &Arc<Mutex<HtmlReport>>) {
    let Ok(mut report) = report.lock() else {
        return;
    };
    if report.written {
        return;
    }
    report.written = true;
    let title = report.title.clone();
    let mut events = std::mem::take(&mut report.events);
    let updates = std::mem::take(&mut report.updates);
    apply_update_map(&mut events, &updates);
    write_html_page(&title, &events);
}

/// Write one finished page. Adjacent stream reads merge into a single block:
/// the kernel delivers stdout and stderr as arbitrary pipe-sized chunks, so a
/// logical line can otherwise be split across boxes.
fn write_html_page(title: &str, events: &[Recorded]) {
    let mut body = String::new();
    let mut index = 0;
    while index < events.len() {
        if matches!(&events[index], Recorded::Update { .. }) {
            index += 1;
            continue;
        }
        if matches!(&events[index], Recorded::Stdout(_) | Recorded::Stderr(_)) {
            let stderr = matches!(&events[index], Recorded::Stderr(_));
            let mut text = String::new();
            while index < events.len() {
                match &events[index] {
                    Recorded::Stderr(chunk) if stderr => text.push_str(chunk),
                    Recorded::Stdout(chunk) if !stderr => text.push_str(chunk),
                    // Updates are folded into their dumps before this pass;
                    // skip them so a live value between prints cannot split
                    // the surrounding stream block.
                    Recorded::Update { .. } => {}
                    _ => break,
                }
                index += 1;
            }
            if !text.is_empty() {
                body.push_str(&html::stream_block(
                    if stderr { "stderr" } else { "stdout" },
                    &text,
                ));
            }
            continue;
        }
        if let Recorded::Dump(node) = &events[index] {
            body.push_str("<section class=\"dump\">");
            body.push_str(&html::render_dump(node));
            body.push_str("</section>\n");
        }
        // Updates were folded into the dumps before this pass.
        index += 1;
    }
    let page = html::document(title, &body);
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(page.as_bytes());
    let _ = stdout.flush();
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
    use super::{language_for, run_file_in, OutputFormat, ScriptLanguage};
    use codingpad_kernel_core::secrets::Vault;
    use std::{fs, path::Path, sync::Arc};

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
            language_for(Path::new("hello.TS")),
            Ok(ScriptLanguage::Typescript)
        );
        assert_eq!(
            language_for(Path::new("hello.RS")),
            Ok(ScriptLanguage::Rust)
        );
        assert_eq!(
            language_for(Path::new("query.sql")),
            Ok(ScriptLanguage::Sql)
        );
    }

    #[test]
    fn unknown_extensions_are_rejected_with_the_expected_ones() {
        let message = language_for(Path::new("notes.txt")).unwrap_err();
        assert!(message.contains(".py, .cs, .ts, .rs or .sql"));
        assert!(message.contains("notes.txt"));
    }

    struct TestVault;

    impl Vault for TestVault {
        fn get(&self, name: &str) -> Result<Option<String>, String> {
            assert_eq!(name, "test-token");
            Ok(Some("vault-fixture".into()))
        }

        fn set(&self, _: &str, _: &str) -> Result<(), String> {
            panic!("Running a script must not write vault entries");
        }

        fn delete(&self, _: &str) -> Result<(), String> {
            panic!("Running a script must not delete vault entries");
        }
    }

    #[tokio::test]
    async fn typescript_cli_runner_reads_a_vault_secret() {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data");
        fs::create_dir(&data).unwrap();
        fs::write(data.join("secrets.json"), r#"["test-token"]"#).unwrap();
        let script = directory.path().join("secret.ts");
        fs::write(
            &script,
            r#"
import { secret } from "codingpad";
if (secret("test-token") !== "vault-fixture") {
  throw new Error("The CLI did not pass the vault value to the script");
}
"#,
        )
        .unwrap();
        assert_eq!(
            run_file_in(&script, OutputFormat::Text, data, Arc::new(TestVault),).await,
            0
        );
    }

    #[tokio::test]
    async fn rust_cli_runner_reads_a_vault_secret() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("secret.rs");
        fs::write(
            &script,
            r#"assert_eq!(codingpad::secret("test-token"), "vault-fixture");"#,
        )
        .unwrap();
        assert_eq!(
            run_file_in(
                &script,
                OutputFormat::Text,
                directory.path().join("data"),
                Arc::new(TestVault)
            )
            .await,
            0
        );
    }
}
