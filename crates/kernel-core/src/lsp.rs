//! Language-server multiplexer and the Python (basedpyright) workspace setup.
//!
//! The desktop talks to this module over Tauri commands. Each script gets its
//! own language-server process and its own temporary workspace, so scripts keep
//! independent package environments and diagnostics. The transport is LSP's
//! `Content-Length`-framed JSON-RPC over stdio, which Roslyn, rust-analyzer and
//! the TypeScript server can reuse later.
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Duration,
};
use tempfile::TempDir;
use tokio::{
    io::{
        AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, Lines,
    },
    process::{Child, ChildStdout, Command},
    sync::{mpsc, oneshot, Mutex, Notify},
    time::timeout,
};
use url::Url;

use crate::{
    data_context::GeneratedContext,
    databases::SqlManager,
    find_uv, roslyn,
    rust::{self, RustShadowProject, Source},
    shadow::{ShadowProject, ShadowProjects},
    toolchains, ts7, typescript, RUNTIME,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
// rust-analyzer can block briefly while Cargo and the standard library load.
const RUST_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const RUST_CHECK_DEBOUNCE: Duration = Duration::from_millis(400);
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
const DIAGNOSTICS_METHOD: &str = "textDocument/publishDiagnostics";

/// Opt-in transport timeline, without document text or completion payloads.
fn trace_lsp(transport: u32, phase: &str, message: &Value) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("CODINGPAD_LSP_TRACE").is_some()) {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    eprintln!(
        "lsp_trace {}",
        json!({
            "unix_us": now.as_micros() as u64, "transport_pid": transport, "phase": phase,
            "id": message.get("id"), "method": message.get("method"),
            "version": message.pointer("/params/textDocument/version"),
        })
    );
}

/// A notification forwarded from a language server to the window.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspEvent {
    pub method: String,
    pub params: Value,
}

/// The document URI the editor should use for versioned LSP requests.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspStartResult {
    pub uri: String,
}

type EventSink = Arc<dyn Fn(LspEvent) + Send + Sync>;
type Events = Arc<StdMutex<EventSink>>;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

pub struct LanguageServers {
    uv: PathBuf,
    data_dir: Option<PathBuf>,
    databases: Option<Arc<SqlManager>>,
    servers: Mutex<HashMap<String, Arc<LspServer>>>,
    starts: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    session: Mutex<Option<String>>,
    closed: Mutex<HashSet<String>>,
    shadow_projects: Arc<ShadowProjects>,
}

impl Default for LanguageServers {
    fn default() -> Self {
        Self::with_uv(find_uv())
    }
}

impl LanguageServers {
    pub fn with_uv(uv: PathBuf) -> Self {
        Self {
            uv,
            data_dir: None,
            databases: None,
            servers: Mutex::new(HashMap::new()),
            starts: Mutex::new(HashMap::new()),
            session: Mutex::new(None),
            closed: Mutex::new(HashSet::new()),
            shadow_projects: Arc::default(),
        }
    }

    /// Resolve user tools and the app-private installs on every server startup.
    pub fn with_data_dir(data_dir: PathBuf) -> Self {
        Self {
            data_dir: Some(data_dir.clone()),
            shadow_projects: Arc::new(ShadowProjects::with_data_dir(data_dir)),
            ..Self::default()
        }
    }

    /// Generate typed data contexts from this manager's saved connections.
    pub fn with_data_dir_and_databases(data_dir: PathBuf, databases: Arc<SqlManager>) -> Self {
        Self {
            databases: Some(databases),
            ..Self::with_data_dir(data_dir)
        }
    }

    pub fn with_shadow_projects(mut self, shadow_projects: Arc<ShadowProjects>) -> Self {
        self.shadow_projects = shadow_projects;
        self
    }

    /// Prepare the editor document and packages before starting TypeScript code intelligence.
    pub async fn prepare_typescript(
        &self,
        script_id: &str,
        source: &str,
        session: &str,
    ) -> Result<Option<LspStartResult>, String> {
        let project = self
            .prepare_typescript_workspace(script_id, source, session)
            .await?;
        Ok(Some(LspStartResult {
            uri: project.shadow.document_uri().to_owned(),
        }))
    }

    /// Prepare the shared project and return the document `tsgo` should open.
    ///
    /// The shared manager owns package installation and the staged document.
    /// A syntax error or an unsupported import version still gets a document so
    /// the language server can show the problem. An install failure falls back
    /// to a package-free project, keeping helper types and plain TypeScript
    /// available instead of disabling code intelligence for the tab.
    async fn prepare_typescript_workspace(
        &self,
        script_id: &str,
        source: &str,
        session: &str,
    ) -> Result<Arc<typescript::Project>, String> {
        if script_id.is_empty() {
            return Err("A script id is required".into());
        }
        self.check_session(script_id, session).await?;
        let imports = typescript::parse(source)
            .ok()
            .flatten()
            .map(|script| script.imports);
        let project = match self
            .shadow_projects
            .prepare_typescript(script_id, source, imports, true, |_| {})
            .await
        {
            Ok(project) => project,
            Err(error) => {
                // Close may have removed the previous project before this
                // request registered with the shared manager.
                if self.closed.lock().await.contains(script_id) {
                    self.shadow_projects.close(script_id).await;
                    return Err("The script was closed".into());
                }
                self.check_session(script_id, session).await?;
                // An install failure must not disable code intelligence. A
                // package-free project still provides the codingpad helper
                // types, and the unresolved import appears as a diagnostic.
                self.shadow_projects
                    .prepare_typescript(script_id, source, None, true, |_| {})
                    .await
                    .map_err(|fallback| {
                        format!(
                            "Could not prepare TypeScript code intelligence: {error}\n{fallback}"
                        )
                    })?
            }
        };
        if self.closed.lock().await.contains(script_id) {
            self.shadow_projects.close(script_id).await;
            return Err("The script was closed".into());
        }
        self.check_session(script_id, session).await?;
        Ok(project)
    }

    async fn data_context(&self, source: &str) -> Option<GeneratedContext> {
        let databases = self.databases.as_ref()?;
        let name = crate::store::parse_header(source)?
            .settings
            .get("connection")
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())?;
        databases.generated_models(&name).await.ok()
    }

    fn uv_path(&self) -> PathBuf {
        self.data_dir
            .as_deref()
            .map(toolchains::resolve_uv)
            .unwrap_or_else(|| self.uv.clone())
    }

    /// Start the Python language server for one script, or reuse the warm one
    /// when the script's inline dependency set has not changed.
    pub async fn start_python(
        &self,
        script_id: &str,
        source: &str,
        session: &str,
        emit: impl Fn(LspEvent) + Send + Sync + 'static,
    ) -> Result<LspStartResult, String> {
        if script_id.is_empty() {
            return Err("A script id is required".into());
        }
        let dependencies = inline_dependencies(source);
        let events: EventSink = Arc::new(emit);

        let _start = self.lock_start(script_id).await;
        if self.closed.lock().await.contains(script_id) {
            return Err("The script was closed".into());
        }
        if self.session.lock().await.as_deref() != Some(session) {
            return Err(
                "The editor session is stale. Reload the tab to restart code intelligence.".into(),
            );
        }
        let data = self.data_context(source).await;
        let restart_key = format!(
            "{}\n{}",
            dependencies.join("\n"),
            data.as_ref().map(|data| data.python.as_str()).unwrap_or("")
        );
        let existing = self.servers.lock().await.get(script_id).cloned();
        if let Some(existing) = existing {
            if existing.restart_key == restart_key && existing.alive() {
                existing.set_events(events.clone());
                if existing.did_change(source).await.is_ok() {
                    if self.closed.lock().await.contains(script_id) {
                        self.servers.lock().await.remove(script_id);
                        existing.stop().await;
                        return Err("The script was closed".into());
                    }
                    if self.session.lock().await.as_deref() != Some(session) {
                        self.servers.lock().await.remove(script_id);
                        existing.stop().await;
                        return Err("The editor session is stale. Reload the tab to restart code intelligence.".into());
                    }
                    return Ok(LspStartResult {
                        uri: existing.document_uri.clone(),
                    });
                }
            }
            self.servers.lock().await.remove(script_id);
            existing.stop().await;
        }

        let workspace = Workspace::Python(PythonWorkspace::create(source, data.as_ref())?);
        let mut python_dependencies = dependencies.clone();
        if data.is_some() {
            python_dependencies.push("sqlalchemy".into());
        }
        let mut command = server_command(&self.uv_path(), &python_dependencies);
        if let Ok(paths) = std::env::join_paths(workspace.extra_paths()) {
            command.env("PYTHONPATH", paths);
        }
        let server = LspServer::start(
            command,
            workspace,
            "python",
            "Python",
            restart_key,
            "uv was not found. Press Run and choose Install uv and run to enable code intelligence, or install uv on PATH yourself and reopen this tab.".to_owned(),
            events,
        )
        .await?;
        let previous = self
            .servers
            .lock()
            .await
            .insert(script_id.to_owned(), server.clone());
        if let Some(previous) = previous {
            previous.stop().await;
        }

        if let Err(error) = server.initialize().await {
            let message = server.failure_message(error).await;
            if self.remove_if_same(script_id, &server).await {
                server.stop().await;
            }
            return Err(message);
        }
        if let Err(error) = server.did_open(source).await {
            let message = server.failure_message(error).await;
            if self.remove_if_same(script_id, &server).await {
                server.stop().await;
            }
            return Err(message);
        }
        if !self.is_current(script_id, &server).await {
            server.stop().await;
            return Err("The language server was stopped".into());
        }
        if self.closed.lock().await.contains(script_id) {
            self.remove_if_same(script_id, &server).await;
            server.stop().await;
            return Err("The script was closed".into());
        }
        if self.session.lock().await.as_deref() != Some(session) {
            self.remove_if_same(script_id, &server).await;
            server.stop().await;
            return Err(
                "The editor session is stale. Reload the tab to restart code intelligence.".into(),
            );
        }
        Ok(LspStartResult {
            uri: server.document_uri.clone(),
        })
    }

    /// Start Roslyn for one C# script using a generated shadow project.
    pub async fn start_csharp(
        &self,
        script_id: &str,
        source: &str,
        path: Option<&Path>,
        session: &str,
        emit: impl Fn(LspEvent) + Send + Sync + 'static,
    ) -> Result<LspStartResult, String> {
        if script_id.is_empty() {
            return Err("A script id is required".into());
        }
        let directives = crate::shadow::Directives::parse(source)?;
        let data = self.data_context(source).await;
        let restart_key = format!(
            "{}\n{}\n{}",
            directives.key(),
            path.map(|p| p.display().to_string()).unwrap_or_default(),
            data.as_ref().map(|data| data.csharp.as_str()).unwrap_or("")
        );
        let events: EventSink = Arc::new(emit);

        let _start = self.lock_start(script_id).await;
        self.check_session(script_id, session).await?;
        if let Some(started) = self
            .reuse_server(script_id, source, session, "csharp", &restart_key, &events)
            .await?
        {
            return Ok(started);
        }

        let project_key = format!("{script_id}\n{restart_key}");
        let workspace = Workspace::Csharp(ShadowProject::create(
            source,
            path,
            &project_key,
            data.as_ref(),
        )?);
        let dotnet = toolchains::resolve_dotnet(self.data_dir.as_deref()).await;
        let roslyn = roslyn::find(self.data_dir.as_deref(), &dotnet)?;
        let mut command = roslyn.command();
        command.current_dir(workspace.root());
        let server = LspServer::start(
            command,
            workspace,
            "csharp",
            "C#",
            restart_key,
            "The Roslyn language server was not found. Install the C# extension in VS Code, or set CODINGPAD_ROSLYN_LSP to Microsoft.CodeAnalysis.LanguageServer.".to_owned(),
            events,
        )
        .await?;
        let previous = self
            .servers
            .lock()
            .await
            .insert(script_id.to_owned(), server.clone());
        if let Some(previous) = previous {
            previous.stop().await;
        }

        if let Err(error) = server.initialize().await {
            let message = server.failure_message(error).await;
            if self.remove_if_same(script_id, &server).await {
                server.stop().await;
            }
            return Err(message);
        }
        if let Err(error) = server.did_open(source).await {
            let message = server.failure_message(error).await;
            if self.remove_if_same(script_id, &server).await {
                server.stop().await;
            }
            return Err(message);
        }
        if !self.is_current(script_id, &server).await {
            server.stop().await;
            return Err("The language server was stopped".into());
        }
        self.check_session(script_id, session).await?;
        Ok(LspStartResult {
            uri: server.document_uri.clone(),
        })
    }

    /// Start the TypeScript 7 native language server on the shared shadow
    /// project that Run and the editor both use.
    pub async fn start_typescript(
        &self,
        script_id: &str,
        source: &str,
        session: &str,
        emit: impl Fn(LspEvent) + Send + Sync + 'static,
    ) -> Result<LspStartResult, String> {
        if script_id.is_empty() {
            return Err("A script id is required".into());
        }
        // Fail before preparing if the server is missing: the editor can still
        // run scripts, and there is no reason to install packages for a server
        // that cannot start.
        let server = ts7::find(self.data_dir.as_deref())?;
        // Prepare before taking the per-script start lock. A package install
        // must not make another start wait on the network.
        let project = self
            .prepare_typescript_workspace(script_id, source, session)
            .await?;
        let restart_key = project.shadow.document_uri().to_owned();
        let events: EventSink = Arc::new(emit);

        let _start = self.lock_start(script_id).await;
        self.check_session(script_id, session).await?;
        if let Some(started) = self
            .reuse_server(
                script_id,
                source,
                session,
                "typescript",
                &restart_key,
                &events,
            )
            .await?
        {
            return Ok(started);
        }

        let mut command = server.command();
        command.current_dir(project.shadow.root());
        let server = LspServer::start(
            command,
            Workspace::TypeScript(project),
            "typescript",
            "TypeScript",
            restart_key,
            "The TypeScript 7 language server was not found. Install it with `bun add -g @typescript/native-preview`, or set CODINGPAD_TS_LSP to the tsgo executable.".to_owned(),
            events,
        )
        .await?;
        self.register_started(script_id, source, session, server)
            .await
    }

    /// Start rust-analyzer in an editor shadow project generated by the Rust kernel.
    pub async fn start_rust(
        &self,
        script_id: &str,
        source: &str,
        path: Option<&Path>,
        session: &str,
        emit: impl Fn(LspEvent) + Send + Sync + 'static,
    ) -> Result<LspStartResult, String> {
        if script_id.is_empty() {
            return Err("A script id is required".into());
        }
        let manifest = rust::Manifest::parse(source, path).map_err(|error| error.to_string())?;
        let restart_key = manifest.editor_restart_key();
        let events: EventSink = Arc::new(emit);
        let _start = self.lock_start(script_id).await;
        self.check_session(script_id, session).await?;
        if let Some(started) = self
            .reuse_server(script_id, source, session, "rust", &restart_key, &events)
            .await?
        {
            return Ok(started);
        }

        let command = rust::analyzer_command(self.data_dir.as_deref()).await?;
        let workspace =
            RustShadowProject::create(&self.shadow_projects.rust_build, &manifest, source).await?;
        let server = LspServer::start(
            command,
            Workspace::Rust(workspace),
            "rust",
            "rust-analyzer",
            restart_key,
            rust::MISSING_ANALYZER.to_owned(),
            events,
        )
        .await?;
        self.register_started(script_id, source, session, server)
            .await
    }

    async fn lock_start(&self, script_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let start = self
            .starts
            .lock()
            .await
            .entry(script_id.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        start.lock_owned().await
    }

    async fn reuse_server(
        &self,
        script_id: &str,
        source: &str,
        session: &str,
        language: &str,
        restart_key: &str,
        events: &EventSink,
    ) -> Result<Option<LspStartResult>, String> {
        let existing = self.servers.lock().await.get(script_id).cloned();
        let Some(existing) = existing else {
            return Ok(None);
        };
        if existing.language_id == language
            && existing.restart_key == restart_key
            && existing.alive()
        {
            existing.set_events(events.clone());
            if existing.did_change(source).await.is_ok() {
                self.check_session(script_id, session).await?;
                return Ok(Some(LspStartResult {
                    uri: existing.document_uri.clone(),
                }));
            }
        }
        if self.remove_if_same(script_id, &existing).await {
            existing.stop().await;
        }
        Ok(None)
    }

    async fn register_started(
        &self,
        script_id: &str,
        source: &str,
        session: &str,
        server: Arc<LspServer>,
    ) -> Result<LspStartResult, String> {
        let previous = self
            .servers
            .lock()
            .await
            .insert(script_id.to_owned(), server.clone());
        if let Some(previous) = previous {
            previous.stop().await;
        }

        if let Err(error) = server.initialize().await {
            let message = server.failure_message(error).await;
            if self.remove_if_same(script_id, &server).await {
                server.stop().await;
            }
            return Err(message);
        }
        if let Err(error) = server.did_open(source).await {
            let message = server.failure_message(error).await;
            if self.remove_if_same(script_id, &server).await {
                server.stop().await;
            }
            return Err(message);
        }
        if !self.is_current(script_id, &server).await {
            server.stop().await;
            return Err("The language server was stopped".into());
        }
        self.check_session(script_id, session).await?;
        Ok(LspStartResult {
            uri: server.document_uri.clone(),
        })
    }

    async fn check_session(&self, script_id: &str, session: &str) -> Result<(), String> {
        if self.closed.lock().await.contains(script_id) {
            return Err("The script was closed".into());
        }
        if self.session.lock().await.as_deref() != Some(session) {
            return Err(
                "The editor session is stale. Reload the tab to restart code intelligence.".into(),
            );
        }
        Ok(())
    }

    /// Synchronize the full document after an edit.
    pub async fn did_change(&self, script_id: &str, source: &str) -> Result<(), String> {
        self.server(script_id).await?.did_change(source).await
    }

    /// Send an LSP request and return its result.
    pub async fn request(
        &self,
        script_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        if !method.starts_with("textDocument/") && method != "completionItem/resolve" {
            return Err("Only text-document and completion-resolve requests are allowed".into());
        }
        self.server(script_id).await?.request(method, params).await
    }

    pub async fn stop(&self, script_id: &str) {
        let server = self.servers.lock().await.remove(script_id);
        if let Some(server) = server {
            server.stop().await;
        }
    }

    /// Start a fresh editor session, stopping servers left by a reload.
    pub async fn begin_session(&self, session: &str) {
        let mut current = self.session.lock().await;
        if current.as_deref() == Some(session) {
            return;
        }
        *current = Some(session.to_owned());
        self.closed.lock().await.clear();
        self.starts.lock().await.clear();
        let servers: Vec<_> = self
            .servers
            .lock()
            .await
            .drain()
            .map(|(_, server)| server)
            .collect();
        drop(current);
        for server in servers {
            server.stop().await;
        }
    }

    /// Stop a closed tab's server and remember not to start it again.
    pub async fn close(&self, script_id: &str) {
        self.closed.lock().await.insert(script_id.to_owned());
        self.starts.lock().await.remove(script_id);
        self.shadow_projects.close(script_id).await;
        self.stop(script_id).await;
    }

    pub async fn shutdown(&self) {
        self.starts.lock().await.clear();
        let servers: Vec<_> = self
            .servers
            .lock()
            .await
            .drain()
            .map(|(_, server)| server)
            .collect();
        for server in servers {
            server.stop().await;
        }
    }

    async fn server(&self, script_id: &str) -> Result<Arc<LspServer>, String> {
        self.servers
            .lock()
            .await
            .get(script_id)
            .cloned()
            .ok_or_else(|| {
                "Code intelligence is not running for this script. Reopen the tab to restart it."
                    .to_owned()
            })
    }

    async fn remove_if_same(&self, script_id: &str, server: &Arc<LspServer>) -> bool {
        let mut servers = self.servers.lock().await;
        if servers
            .get(script_id)
            .is_some_and(|current| Arc::ptr_eq(current, server))
        {
            servers.remove(script_id);
            true
        } else {
            false
        }
    }

    async fn is_current(&self, script_id: &str, server: &Arc<LspServer>) -> bool {
        self.servers
            .lock()
            .await
            .get(script_id)
            .is_some_and(|current| Arc::ptr_eq(current, server))
    }
}

struct LspServer {
    transport_pid: u32,
    child: Mutex<Child>,
    writer: mpsc::UnboundedSender<Value>,
    pending: Pending,
    next_id: AtomicU64,
    version: Arc<Mutex<i64>>,
    alive: Arc<AtomicBool>,
    events: Events,
    restart_key: String,
    language_id: &'static str,
    server_name: &'static str,
    pull_diagnostics: bool,
    document_uri: String,
    workspace_uri: String,
    extra_paths: Vec<String>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    diagnostics_notify: Arc<Notify>,
    diagnostics_last: Arc<StdMutex<Option<Value>>>,
    rust_editor: Option<Arc<RustEditor>>,
    workspace: Workspace,
}

#[derive(Default)]
struct RustEditor {
    source: StdMutex<Option<Arc<Source>>>,
    check_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

enum Workspace {
    Python(PythonWorkspace),
    Csharp(ShadowProject),
    TypeScript(Arc<typescript::Project>),
    Rust(RustShadowProject),
}

impl Workspace {
    fn root(&self) -> &Path {
        match self {
            Workspace::Python(workspace) => &workspace.root,
            Workspace::Csharp(workspace) => workspace.root(),
            Workspace::TypeScript(project) => project.shadow.root(),
            Workspace::Rust(project) => project.shadow.root(),
        }
    }

    fn document_uri(&self) -> &str {
        match self {
            Workspace::Python(workspace) => &workspace.document_uri,
            Workspace::Csharp(workspace) => workspace.document_uri(),
            Workspace::TypeScript(project) => project.shadow.document_uri(),
            Workspace::Rust(project) => project.shadow.document_uri(),
        }
    }

    fn root_uri(&self) -> &str {
        match self {
            Workspace::Python(workspace) => &workspace.root_uri,
            Workspace::Csharp(workspace) => workspace.root_uri(),
            Workspace::TypeScript(project) => project.shadow.root_uri(),
            Workspace::Rust(project) => project.shadow.root_uri(),
        }
    }

    fn extra_paths(&self) -> &[String] {
        match self {
            Workspace::Python(workspace) => &workspace.extra_paths,
            Workspace::Csharp(_) | Workspace::TypeScript(_) | Workspace::Rust(_) => &[],
        }
    }

    fn touch(&self) {
        if let Workspace::Csharp(workspace) = self {
            workspace.touch();
        }
    }
}

impl LspServer {
    async fn start(
        mut command: Command,
        workspace: Workspace,
        language_id: &'static str,
        server_name: &'static str,
        restart_key: String,
        start_error: String,
        events: EventSink,
    ) -> Result<Arc<Self>, String> {
        let root_uri = workspace.root_uri().to_owned();
        let document_uri = workspace.document_uri().to_owned();
        let extra_paths = workspace.extra_paths().to_vec();
        command
            .current_dir(workspace.root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(0x08000000 | 0x00000200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP

        let mut child = command.spawn().map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                start_error
            } else {
                format!("Could not start {server_name} code intelligence: {error}")
            }
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Could not open the language server input".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Could not open the language server output".to_owned())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Could not open the language server error stream".to_owned())?;
        let transport_pid = child.id().unwrap_or_default();
        let (writer, mut writer_rx) = mpsc::unbounded_channel::<Value>();
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(message) = writer_rx.recv().await {
                trace_lsp(transport_pid, "write_started", &message);
                if write_message(&mut stdin, &message).await.is_err() {
                    break;
                }
                trace_lsp(transport_pid, "write_finished", &message);
            }
        });
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let version = Arc::new(Mutex::new(0));
        let alive = Arc::new(AtomicBool::new(true));
        let events: Events = Arc::new(StdMutex::new(events));
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));

        let server = Arc::new(Self {
            transport_pid,
            child: Mutex::new(child),
            writer: writer.clone(),
            pending: pending.clone(),
            next_id: AtomicU64::new(1),
            version: version.clone(),
            alive: alive.clone(),
            events: events.clone(),
            restart_key,
            language_id,
            server_name,
            pull_diagnostics: matches!(language_id, "csharp" | "typescript"),
            document_uri,
            workspace_uri: root_uri,
            extra_paths,
            stderr_tail: stderr_tail.clone(),
            diagnostics_notify: Arc::new(Notify::new()),
            diagnostics_last: Arc::new(StdMutex::new(None)),
            rust_editor: matches!(workspace, Workspace::Rust(_)).then(Arc::default),
            workspace,
        });

        let response_config = ResponseConfig {
            transport_pid,
            document_uri: server.document_uri.clone(),
            workspace_uri: server.workspace_uri.clone(),
            extra_paths: server.extra_paths.clone(),
            server_name,
            version,
            rust_editor: server.rust_editor.clone(),
        };
        tokio::spawn(read_loop(
            BufReader::new(stdout),
            pending,
            events,
            writer,
            response_config,
            alive,
        ));
        tokio::spawn(read_stderr(stderr, stderr_tail));
        if server.pull_diagnostics {
            // Pull diagnostics on demand and during project loading, then go
            // idle so background tabs do not poll Roslyn forever.
            let poll = server.clone();
            let wake = server.diagnostics_notify.clone();
            tokio::spawn(async move {
                let burst = std::time::Instant::now() + Duration::from_secs(30);
                loop {
                    if std::time::Instant::now() < burst {
                        tokio::select! {
                            _ = wake.notified() => {}
                            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                        }
                    } else {
                        tokio::select! {
                            _ = wake.notified() => {}
                            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                                if !poll.alive() {
                                    return;
                                }
                                // Keep the shadow project's liveness marker fresh
                                // so another script's sweep cannot remove it.
                                poll.workspace.touch();
                                continue;
                            }
                        }
                    }
                    if !poll.alive() {
                        return;
                    }
                    poll.workspace.touch();
                    let Some((version, diagnostics)) = poll.document_diagnostics().await else {
                        continue;
                    };
                    let changed = poll
                        .diagnostics_last
                        .lock()
                        .map(|mut last| {
                            if last.as_ref() == Some(&diagnostics) {
                                false
                            } else {
                                *last = Some(diagnostics.clone());
                                true
                            }
                        })
                        .unwrap_or(true);
                    if changed {
                        poll.emit_diagnostics(version, diagnostics);
                    }
                }
            });
        }
        Ok(server)
    }

    fn set_events(&self, events: EventSink) {
        if let Ok(mut slot) = self.events.lock() {
            *slot = events;
        }
        // A remounted editor starts with no markers, so force the next pull to
        // deliver the unchanged diagnostics to the new sink.
        if let Ok(mut last) = self.diagnostics_last.lock() {
            *last = None;
        }
        self.diagnostics_notify.notify_one();
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    async fn initialize(&self) -> Result<(), String> {
        let mut params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "CodingPad" },
            "locale": "en",
            "rootUri": &self.workspace_uri,
            "workspaceFolders": [
                { "uri": &self.workspace_uri, "name": "CodingPad" }
            ],
            "capabilities": {
                "textDocument": {
                    "synchronization": { "dynamicRegistration": false },
                    "completion": {
                        "completionItem": {
                            "snippetSupport": true,
                            "documentationFormat": ["markdown", "plaintext"],
                            "insertReplaceSupport": true,
                            "resolveSupport": {
                                // basedpyright can defer expensive deprecated-type checks
                                // until an item is selected when tags can be resolved.
                                "properties": ["documentation", "detail", "additionalTextEdits", "tags"]
                            }
                        }
                    },
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "publishDiagnostics": {
                        "relatedInformation": true,
                        "versionSupport": true
                    },
                    "diagnostic": {
                        "dynamicRegistration": false,
                        "relatedDocumentSupport": true
                    }
                },
                "workspace": {
                    "workspaceFolders": true,
                    "configuration": true,
                    "didChangeConfiguration": { "dynamicRegistration": true }
                },
                "window": {
                    "workDoneProgress": true
                }
            }
        });
        if self.rust_editor.is_some() {
            params["capabilities"]["general"] = json!({ "positionEncodings": ["utf-16"] });
            params["initializationOptions"] = rust_analyzer_settings();
        }
        self.request_with_timeout("initialize", params, INITIALIZE_TIMEOUT)
            .await?;
        self.notify("initialized", json!({})).await
    }

    fn stage_rust(&self, source: &str) -> Result<Option<Arc<Source>>, String> {
        let (Workspace::Rust(workspace), Some(editor)) = (&self.workspace, &self.rust_editor)
        else {
            return Ok(None);
        };
        let source = workspace.stage(source)?;
        *editor
            .source
            .lock()
            .map_err(|_| "Rust source mapping unavailable")? = Some(source.clone());
        Ok(Some(source))
    }

    async fn did_open(&self, source: &str) -> Result<(), String> {
        let mut version = self.version.lock().await;
        let mapped = self.stage_rust(source)?;
        let source = mapped.as_ref().map_or(source, |doc| &doc.generated);
        *version = 1;
        let notice = self.write(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": &self.document_uri,
                    "languageId": self.language_id,
                    "version": 1,
                    "text": source
                }
            }
        }));
        notice?;
        self.diagnostics_notify.notify_one();
        if let Some(editor) = &self.rust_editor {
            self.schedule_rust_check(editor).await;
        }
        Ok(())
    }

    async fn did_change(&self, source: &str) -> Result<(), String> {
        let mut version = self.version.lock().await;
        let mapped = self.stage_rust(source)?;
        let source = mapped.as_ref().map_or(source, |doc| &doc.generated);
        *version += 1;
        self.write(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": &self.document_uri, "version": *version },
                "contentChanges": [{ "text": source }]
            }
        }))?;
        self.diagnostics_notify.notify_one();
        if let Some(editor) = &self.rust_editor {
            self.schedule_rust_check(editor).await;
        }
        Ok(())
    }

    async fn schedule_rust_check(&self, editor: &RustEditor) {
        let mut task = editor.check_task.lock().await;
        if let Some(previous) = task.take() {
            previous.abort();
        }
        let writer = self.writer.clone();
        let uri = self.document_uri.clone();
        *task = Some(tokio::spawn(async move {
            // Completions see edits immediately. Defer Cargo check until idle.
            tokio::time::sleep(RUST_CHECK_DEBOUNCE).await;
            let _ = writer.send(json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didSave",
                "params": { "textDocument": { "uri": uri } }
            }));
        }));
    }

    /// Roslyn publishes document diagnostics on request rather than on edit.
    async fn document_diagnostics(&self) -> Option<(i64, Value)> {
        let version = *self.version.lock().await;
        // Version 0 means `didOpen` has not been sent, and it is only sent
        // after the `initialize` response. LSP forbids requests before that
        // response, and tsgo crashes on one, which a slow Windows start exposes.
        if version == 0 {
            return None;
        }
        let result = self
            .request(
                "textDocument/diagnostic",
                json!({ "textDocument": { "uri": &self.document_uri } }),
            )
            .await
            .ok()?;
        let diagnostics = match result {
            Value::Array(items) => Value::Array(items),
            Value::Object(report) => report.get("items").cloned().unwrap_or_else(|| json!([])),
            _ => json!([]),
        };
        Some((version, diagnostics))
    }

    fn emit_diagnostics(&self, version: i64, diagnostics: Value) {
        let sink = self.events.lock().ok().map(|events| (*events).clone());
        if let Some(sink) = sink {
            sink(LspEvent {
                method: DIAGNOSTICS_METHOD.to_owned(),
                params: json!({
                    "uri": &self.document_uri,
                    "version": version,
                    "diagnostics": diagnostics
                }),
            });
        }
    }

    async fn request(&self, method: &str, mut params: Value) -> Result<Value, String> {
        let Some(editor) = &self.rust_editor else {
            return self
                .request_with_timeout(method, params, REQUEST_TIMEOUT)
                .await;
        };

        let (version, source) = {
            let version = *self.version.lock().await;
            let source = editor
                .source
                .lock()
                .map_err(|_| "Rust source mapping unavailable")?
                .clone()
                .ok_or("The Rust editor document is not ready")?;
            (version, source)
        };
        if let Some(position) = params.get_mut("position") {
            let Some(mapped) = source.editor_to_shadow(position) else {
                return Ok(Value::Null);
            };
            *position = mapped;
        }
        let mut response = self
            .request_with_timeout(method, params, RUST_REQUEST_TIMEOUT)
            .await?;
        if *self.version.lock().await != version {
            return Ok(Value::Null);
        }
        source.map_response(method, &mut response);
        Ok(response)
    }

    async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        wait: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        if let Err(error) = self.write(message) {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        match timeout(wait, receiver).await {
            Ok(Ok(result)) => {
                trace_lsp(
                    self.transport_pid,
                    "request_delivered",
                    &json!({"id": id, "method": method}),
                );
                result
            }
            Ok(Err(_)) => Err(format!(
                "The {} language server stopped before answering. Reopen the tab to restart it.",
                self.server_name
            )),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(format!(
                    "The {} language server did not answer {method} in time. Reopen the tab to retry.",
                    self.server_name
                ))
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
    }

    fn write(&self, message: Value) -> Result<(), String> {
        trace_lsp(self.transport_pid, "queued", &message);
        self.writer.send(message).map_err(|_| {
            format!(
                "The {} language server is not accepting messages. Reopen the tab to restart it.",
                self.server_name
            )
        })
    }

    async fn failure_message(&self, error: String) -> String {
        // Give uv a beat to flush an actionable error (for example, a bad
        // dependency) to stderr before reporting it.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let tail = self.stderr_tail.lock().await;
        if tail.is_empty() {
            error
        } else {
            format!(
                "{error} {}",
                tail.iter().cloned().collect::<Vec<_>>().join(" ")
            )
        }
    }

    async fn stop(&self) {
        if let Some(editor) = &self.rust_editor {
            if let Some(task) = editor.check_task.lock().await.take() {
                task.abort();
            }
        }
        let _ = self
            .request_with_timeout("shutdown", Value::Null, Duration::from_millis(500))
            .await;
        let _ = self.notify("exit", Value::Null).await;
        let mut child = self.child.lock().await;
        if timeout(Duration::from_millis(500), child.wait())
            .await
            .is_err()
        {
            crate::kill_process_tree(&mut child);
        }
    }
}

impl Drop for LspServer {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.try_lock() {
            if !matches!(child.try_wait(), Ok(Some(_))) {
                crate::kill_process_tree(&mut child);
            }
        }
    }
}

struct PythonWorkspace {
    _directory: TempDir,
    root: PathBuf,
    document_uri: String,
    root_uri: String,
    extra_paths: Vec<String>,
}

impl PythonWorkspace {
    fn create(source: &str, data: Option<&GeneratedContext>) -> Result<Self, String> {
        let directory = tempfile::tempdir()
            .map_err(|error| format!("Could not create a code-intelligence workspace: {error}"))?;
        let root = directory.path().join("workspace");
        fs::create_dir_all(&root)
            .map_err(|error| format!("Could not create a code-intelligence workspace: {error}"))?;

        let runtime = root.join("runtime");
        fs::create_dir_all(runtime.join("codingpad"))
            .map_err(|error| format!("Could not create the code-intelligence runtime: {error}"))?;
        fs::write(runtime.join("codingpad/__init__.py"), RUNTIME)
            .map_err(|error| format!("Could not create the code-intelligence runtime: {error}"))?;

        // The public stub avoids loading the dump renderer's implementation
        // and its transitive standard-library imports just to type dump(value).
        fs::write(
            runtime.join("codingpad/__init__.pyi"),
            include_str!("../../../runtimes/python/codingpad/__init__.pyi"),
        )
        .map_err(|error| format!("Could not stage CodingPad helper types: {error}"))?;
        if let Some(data) = data {
            fs::write(runtime.join("codingpad_data.py"), &data.python)
                .map_err(|error| format!("Could not stage the generated data context: {error}"))?;
        }

        let script = root.join("script.py");
        fs::write(&script, source).map_err(|error| {
            format!("Could not stage the script for code intelligence: {error}")
        })?;

        let extra_paths = vec![runtime.to_string_lossy().into_owned()];

        let config = json!({
            "include": ["script.py"],
            "extraPaths": extra_paths,
            "typeCheckingMode": "basic",
            "useLibraryCodeForTypes": true,
            "reportMissingImports": "warning",
            "reportMissingModuleSource": "warning"
        });
        fs::write(
            root.join("pyrightconfig.json"),
            serde_json::to_vec_pretty(&config).map_err(|error| {
                format!("Could not create the code-intelligence config: {error}")
            })?,
        )
        .map_err(|error| format!("Could not create the code-intelligence config: {error}"))?;

        Ok(Self {
            document_uri: path_to_uri(&script)?,
            root_uri: path_to_uri(&root)?,
            _directory: directory,
            root,
            extra_paths,
        })
    }
}

struct ResponseConfig {
    transport_pid: u32,
    document_uri: String,
    workspace_uri: String,
    extra_paths: Vec<String>,
    server_name: &'static str,
    version: Arc<Mutex<i64>>,
    rust_editor: Option<Arc<RustEditor>>,
}

async fn read_loop(
    mut stdout: BufReader<ChildStdout>,
    pending: Pending,
    events: Events,
    writer: mpsc::UnboundedSender<Value>,
    config: ResponseConfig,
    alive: Arc<AtomicBool>,
) {
    loop {
        let message = match read_message(&mut stdout).await {
            Ok(Some(message)) => message,
            Ok(None) | Err(_) => break,
        };
        trace_lsp(config.transport_pid, "received", &message);
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id");
        match (method, id) {
            (Some(method), Some(id)) => {
                let result = server_request_result(method, &message, &config);
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result
                });
                let _ = writer.send(response);
            }
            (Some(method), None) => {
                if method == DIAGNOSTICS_METHOD {
                    let params = &message["params"];
                    let current = config.version.lock().await;
                    if diagnostics_are_current(params, &config.document_uri, *current) {
                        let source = config
                            .rust_editor
                            .as_ref()
                            .and_then(|editor| editor.source.lock().ok())
                            .and_then(|source| source.clone());
                        drop(current);
                        let sink = events.lock().ok().map(|events| (*events).clone());
                        if let Some(sink) = sink {
                            let mut params = params.clone();
                            if let Some(source) = source {
                                source.map_diagnostics(&mut params, &config.document_uri);
                            }
                            // The editor identifies this document by the URI we issued.
                            params["uri"] = config.document_uri.clone().into();
                            sink(LspEvent {
                                method: method.to_owned(),
                                params,
                            });
                        }
                    }
                }
            }
            (None, Some(id)) => {
                let Some(id) = id.as_u64() else {
                    continue;
                };
                let Some(sender) = pending.lock().await.remove(&id) else {
                    continue;
                };
                let result = match message.get("error") {
                    Some(error) => Err(error_message(error)),
                    None => Ok(message["result"].clone()),
                };
                let _ = sender.send(result);
            }
            (None, None) => {}
        }
    }

    alive.store(false, Ordering::Relaxed);
    let mut pending = pending.lock().await;
    for (_, sender) in pending.drain() {
        let _ = sender.send(Err(format!(
            "The {} language server stopped. Reopen the tab to restart it.",
            config.server_name
        )));
    }
}

fn diagnostics_are_current(params: &Value, document_uri: &str, version: i64) -> bool {
    let Some(uri) = params.get("uri").and_then(Value::as_str) else {
        return false;
    };
    if !same_document_uri(uri, document_uri) {
        return false;
    }
    params
        .get("version")
        .and_then(Value::as_i64)
        .is_none_or(|incoming| incoming >= version)
}

fn same_document_uri(incoming: &str, expected: &str) -> bool {
    if incoming == expected {
        return true;
    }
    let file_path = |uri: &str| Url::parse(uri).ok()?.to_file_path().ok();
    match (file_path(incoming), file_path(expected)) {
        // File paths decode URI escapes and normalize Windows drive prefixes.
        (Some(incoming), Some(expected)) => incoming == expected,
        _ => false,
    }
}

fn server_request_result(method: &str, message: &Value, config: &ResponseConfig) -> Value {
    match method {
        "workspace/configuration" => configuration_result(message, config),
        "workspace/workspaceFolders" => json!([{
            "uri": &config.workspace_uri,
            "name": "CodingPad"
        }]),
        _ => Value::Null,
    }
}

fn configuration_result(message: &Value, config: &ResponseConfig) -> Value {
    let items = message["params"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Value::Array(
        items
            .iter()
            .map(|item| {
                let section = item.get("section").and_then(Value::as_str).unwrap_or("");
                match section {
                    "python" => json!({
                        "analysis": analysis_settings(config)
                    }),
                    "python.analysis" | "basedpyright.analysis" => analysis_settings(config),
                    "basedpyright" => json!({
                        "analysis": analysis_settings(config)
                    }),
                    // The TypeScript server uses its own defaults; an empty
                    // object answers the registration without overriding them.
                    "typescript" | "javascript" | "js/ts" => json!({}),
                    "" | "rust-analyzer" if config.server_name == "rust-analyzer" => {
                        rust_analyzer_settings()
                    }
                    _ => Value::Null,
                }
            })
            .collect(),
    )
}

fn rust_analyzer_settings() -> Value {
    json!({
        "numThreads": 2,
        "cachePriming": { "numThreads": 2 },
        "cargo": { "allTargets": false },
        "check": { "allTargets": false },
        "checkOnSave": true,
    })
}

fn analysis_settings(config: &ResponseConfig) -> Value {
    json!({
        "extraPaths": &config.extra_paths,
        "typeCheckingMode": "basic",
        "useLibraryCodeForTypes": true,
        "reportMissingImports": "warning",
        "reportMissingModuleSource": "warning"
    })
}

async fn read_stderr(stderr: tokio::process::ChildStderr, tail: Arc<Mutex<VecDeque<String>>>) {
    let mut lines: Lines<BufReader<tokio::process::ChildStderr>> = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let mut tail = tail.lock().await;
        if tail.len() >= 20 {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}

fn server_command(uv: &Path, dependencies: &[String]) -> Command {
    let mut command = Command::new(uv);
    command.args(["tool", "run", "--no-build", "--from", "basedpyright"]);
    for dependency in dependencies {
        command.arg("--with").arg(dependency);
    }
    command.args(["--", "basedpyright-langserver", "--stdio"]);
    command
}

fn error_message(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("The language server reported an error")
        .to_owned()
}

pub(crate) fn path_to_uri(path: &Path) -> Result<String, String> {
    Url::from_file_path(path)
        .map(|url| url.to_string())
        .map_err(|()| format!("Could not convert {} to a file URI", path.display()))
}

async fn write_message<W>(writer: &mut W, message: &Value) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(message)
        .map_err(|error| format!("Invalid language-server message: {error}"))?;
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await
        .map_err(|error| format!("Could not write to the language server: {error}"))?;
    writer
        .write_all(&body)
        .await
        .map_err(|error| format!("Could not write to the language server: {error}"))?;
    writer
        .flush()
        .await
        .map_err(|error| format!("Could not flush the language server: {error}"))
}

async fn read_message<R>(reader: &mut R) -> Result<Option<Value>, String>
where
    R: AsyncBufRead + Unpin,
{
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(|error| format!("Could not read from the language server: {error}"))?;
        if read == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = value.trim().parse::<usize>().ok();
        }
    }
    let length =
        content_length.ok_or_else(|| "The language server sent no Content-Length".to_owned())?;
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|error| format!("Could not read the language-server body: {error}"))?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| format!("Invalid language-server message: {error}"))
}

fn inline_dependencies(source: &str) -> Vec<String> {
    inline_metadata(source)
        .and_then(|metadata| {
            metadata
                .get("dependencies")
                .and_then(toml::Value::as_array)
                .cloned()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|dependency| dependency.as_str().map(str::to_owned))
        .collect()
}

fn inline_metadata(source: &str) -> Option<toml::Value> {
    let mut inside = false;
    let mut closed = false;
    let mut body = String::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if !inside {
            if trimmed == "# /// script" {
                inside = true;
            }
            continue;
        }
        let content = trimmed.strip_prefix('#').unwrap_or(trimmed);
        let content = content.strip_prefix(' ').unwrap_or(content);
        if content.trim() == "///" {
            closed = true;
            break;
        }
        body.push_str(content);
        body.push('\n');
    }
    if !inside || !closed {
        return None;
    }
    toml::from_str(&body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, BufReader};

    #[test]
    fn reads_pep_723_dependencies() {
        let source = r#"# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "requests<3",
#   "rich",
# ]
# ///
print("hi")
"#;
        assert_eq!(inline_dependencies(source), vec!["requests<3", "rich"]);
    }

    #[test]
    fn ignores_scripts_without_inline_metadata() {
        assert!(inline_dependencies("print('hi')").is_empty());
        assert!(inline_dependencies("# dependencies = ['rich']").is_empty());
    }

    #[test]
    fn ignores_a_broken_inline_metadata_block() {
        assert!(inline_dependencies("# /// script\n# dependencies = [\n# ///\n").is_empty());
    }

    #[test]
    fn ignores_an_unclosed_inline_metadata_block() {
        assert!(inline_dependencies("# /// script\n# dependencies = [\"requests\"]\n").is_empty());
    }

    #[test]
    fn answers_configuration_for_python_sections() {
        let config = ResponseConfig {
            transport_pid: 0,
            document_uri: "file:///tmp/workspace/script.py".into(),
            workspace_uri: "file:///tmp/workspace".into(),
            extra_paths: vec!["/tmp/runtime".into()],
            server_name: "Python",
            version: Arc::new(Mutex::new(0)),
            rust_editor: None,
        };
        let result = configuration_result(
            &json!({"params": {"items": [{"section": "python"}, {"section": "python.analysis"}, {"section": "typescript"}, {"section": "other"}]}}),
            &config,
        );
        assert_eq!(result[0]["analysis"]["extraPaths"][0], "/tmp/runtime");
        assert_eq!(result[1]["extraPaths"][0], "/tmp/runtime");
        assert_eq!(result[2], json!({}));
        assert_eq!(result[3], Value::Null);
    }

    #[tokio::test]
    async fn lsp_framing_round_trips() {
        let (mut writer, reader) = duplex(4096);
        let message = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}});
        write_message(&mut writer, &message).await.unwrap();
        drop(writer);
        let mut reader = BufReader::new(reader);
        assert_eq!(read_message(&mut reader).await.unwrap().unwrap(), message);
        assert!(read_message(&mut reader).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn lsp_framing_rejects_a_missing_length() {
        let (mut writer, reader) = duplex(4096);
        writer
            .write_all(b"Content-Type: application/json\r\n\r\n{}")
            .await
            .unwrap();
        let mut reader = BufReader::new(reader);
        assert!(read_message(&mut reader)
            .await
            .unwrap_err()
            .contains("Content-Length"));
    }

    #[test]
    fn filters_diagnostics_from_another_document_or_an_older_version() {
        let params = json!({
            "uri": "file:///workspace/script.py",
            "version": 3,
            "diagnostics": []
        });
        assert!(diagnostics_are_current(
            &params,
            "file:///workspace/script.py",
            3
        ));
        assert!(diagnostics_are_current(
            &params,
            "file:///workspace/script.py",
            2
        ));
        assert!(!diagnostics_are_current(
            &params,
            "file:///workspace/script.py",
            4
        ));
        assert!(!diagnostics_are_current(
            &params,
            "file:///workspace/other.py",
            3
        ));
        assert!(diagnostics_are_current(
            &json!({
                "uri": "file:///workspace/script.py",
                "diagnostics": []
            }),
            "file:///workspace/script.py",
            4
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_diagnostics_accept_equivalent_drive_uris() {
        let params = json!({
            "uri": "file:///c%3A/workspace/my%20script.py",
            "version": 3,
            "diagnostics": []
        });
        assert!(diagnostics_are_current(
            &params,
            "file:///C:/workspace/my%20script.py",
            3
        ));
        assert!(!diagnostics_are_current(
            &params,
            "file:///C:/workspace/other.py",
            3
        ));
        assert!(!diagnostics_are_current(
            &params,
            "file:///C:/workspace/my%20script.py",
            4
        ));
    }

    #[test]
    fn starts_the_language_server_with_wheels_only() {
        let command = server_command(Path::new("uv"), &["requests".into()]);
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|argument| argument == "--no-build"));
        assert!(args.windows(2).any(|pair| pair == ["--with", "requests"]));
    }

    #[tokio::test]
    async fn allows_completion_resolve_but_not_arbitrary_commands() {
        let servers = LanguageServers::with_uv(PathBuf::from("uv"));
        let allowed = servers
            .request("missing", "completionItem/resolve", json!({}))
            .await
            .unwrap_err();
        assert!(allowed.contains("not running"));
        let rejected = servers
            .request("missing", "workspace/executeCommand", json!({}))
            .await
            .unwrap_err();
        assert!(rejected.contains("Only text-document"));
    }

    #[tokio::test]
    async fn rejects_a_stale_editor_session() {
        let servers = LanguageServers::with_uv(PathBuf::from("/missing/uv"));
        servers.begin_session("current").await;
        let error = servers
            .start_python("one", "42", "old", |_| {})
            .await
            .unwrap_err();
        assert!(error.contains("stale"));
    }

    #[tokio::test]
    async fn a_closed_script_is_not_started_again() {
        let servers = LanguageServers::with_uv(PathBuf::from("uv"));
        servers.close("closed").await;
        let error = servers
            .start_python("closed", "42", "test", |_| {})
            .await
            .unwrap_err();
        assert!(error.contains("closed"));
    }

    /// A language server that is slow to answer `initialize` and exits, as tsgo
    /// does, if any request arrives before that answer.
    const SLOW_INITIALIZE_SERVER: &str = r#"
import json, sys, threading, time
stdin, stdout = sys.stdin.buffer, sys.stdout.buffer
lock = threading.Lock()
initialized = threading.Event()
def send(message):
    body = json.dumps(message).encode()
    with lock:
        stdout.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        stdout.flush()
def answer_initialize(id):
    time.sleep(1.5)
    initialized.set()
    send({"jsonrpc": "2.0", "id": id, "result": {"capabilities": {}}})
while True:
    length = 0
    while True:
        line = stdin.readline()
        if not line:
            sys.exit(0)
        if line in (b"\r\n", b"\n"):
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    message = json.loads(stdin.read(length))
    if "id" not in message:
        continue
    if message["method"] == "initialize":
        threading.Thread(target=answer_initialize, args=(message["id"],)).start()
    elif not initialized.is_set():
        sys.stderr.write("request before initialize: " + message["method"] + "\n")
        sys.exit(3)
    else:
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"kind": "full", "items": []}})
"#;

    #[tokio::test]
    async fn diagnostics_wait_for_the_initialize_response() {
        let Some(python) = ["python3", "python"].into_iter().find(|python| {
            std::process::Command::new(python)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
        }) else {
            eprintln!("skipping: Python is needed for the fake language server");
            return;
        };
        let mut command = Command::new(python);
        command.args(["-c", SLOW_INITIALIZE_SERVER]);
        // Any pull-diagnostics language runs the poller; a C# shadow project is
        // the cheapest workspace to stage.
        let workspace = Workspace::Csharp(ShadowProject::temporary("Program.cs").unwrap());
        let server = LspServer::start(
            command,
            workspace,
            "csharp",
            "C#",
            String::new(),
            "missing".into(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        // The diagnostics poller wakes every 500 ms; initialize takes 1.5 s.
        server.initialize().await.unwrap();
        server.did_open("const value = 1;\n").await.unwrap();
        assert!(server.document_diagnostics().await.is_some());
        assert!(server.alive());
        server.stop().await;
    }

    #[tokio::test]
    async fn missing_uv_points_to_the_install_action() {
        let data = tempfile::tempdir().unwrap();
        let servers = LanguageServers::with_uv(data.path().join("missing-uv"));
        servers.begin_session("test").await;
        let error = servers
            .start_python("one", "42", "test", |_| {})
            .await
            .unwrap_err();
        assert!(error.contains("Install uv and run"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn private_uv_is_resolved_after_install_and_on_restart() {
        // Isolate PATH/HOME in a child process rather than racing other tests'
        // process-wide environment or depending on a developer's installed uv.
        if let Some(data) = std::env::var_os("CODINGPAD_LSP_PRIVATE_UV_TEST") {
            let data = PathBuf::from(data);
            let servers = LanguageServers::with_data_dir(data.clone());
            assert!(!servers.uv_path().is_file());
            let private = toolchains::private_uv(&data);
            fs::create_dir_all(private.parent().unwrap()).unwrap();
            fs::write(&private, "installed uv").unwrap();
            assert_eq!(servers.uv_path(), private);
            let restarted = LanguageServers::with_data_dir(data);
            assert_eq!(restarted.uv_path(), private);
            return;
        }
        let data = tempfile::tempdir().unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "lsp::tests::private_uv_is_resolved_after_install_and_on_restart",
                "--nocapture",
            ])
            .env("CODINGPAD_LSP_PRIVATE_UV_TEST", data.path())
            .env("PATH", "")
            .env("HOME", data.path())
            .current_dir(data.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn stages_generated_python_models_with_the_runtime() {
        let data = crate::data_context::GeneratedContext {
            python: "class DataContext:\n    pass\n".into(),
            ..Default::default()
        };
        let workspace =
            PythonWorkspace::create("from codingpad_data import DataContext\n", Some(&data))
                .unwrap();
        assert_eq!(
            fs::read_to_string(workspace.root.join("runtime/codingpad_data.py")).unwrap(),
            data.python
        );
    }

    #[test]
    fn creates_a_workspace_that_exposes_the_codingpad_runtime() {
        let workspace =
            PythonWorkspace::create("from codingpad import dump\ndump(1)", None).unwrap();
        let config = fs::read_to_string(workspace.root.join("pyrightconfig.json")).unwrap();
        assert!(config.contains("runtime"));
        assert!(workspace
            .extra_paths
            .iter()
            .any(|path| path.ends_with("runtime")));
        assert!(workspace.document_uri.ends_with("/script.py"));
    }
}
