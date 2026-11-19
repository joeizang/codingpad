//! Desktop-independent supervisor for one warm kernel process per script.
pub mod crash;
mod csharp;
pub mod data_context;
pub mod databases;
mod dump;
pub mod export;
pub mod html;
pub mod installer;
pub mod lsp;
mod packages;
mod roslyn;
pub mod samples;
pub mod secrets;
pub mod shadow;
pub mod store;
pub mod text;
pub mod toolchains;
mod ts7;
mod typescript;
pub mod updates;

use packages::Packages;
use secrets::{SecretSnapshot, SecretStore};

/// Names the script lexically passes to `secret(...)`/`Secret(...)`.
/// `Some(names)` means read only those; `None` means a dynamic call or an
/// ambiguous use, so every non-connection secret must be available.
fn referenced_secret_names(source: &str) -> Option<Vec<String>> {
    // Imports (including inline/dynamic imports) can call secret() in unseen modules.
    // A conservative substring check may fetch extra names, but cannot omit them.
    let lower = source.to_ascii_lowercase();
    if lower.contains("import") {
        return None;
    }
    if !lower.contains("secret") {
        return Some(Vec::new());
    }
    let bytes = source.as_bytes();
    let mut names = Vec::new();
    let mut index = 0;
    while index + 6 <= bytes.len() {
        if !bytes[index..index + 6].eq_ignore_ascii_case(b"secret") {
            index += 1;
            continue;
        }
        index += 6;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if bytes.get(index) != Some(&b'(') {
            // `s = secret`, a non-call reference, etc.
            return None;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let quote = bytes
            .get(index)
            .copied()
            .filter(|byte| *byte == b'"' || *byte == b'\'')?;
        index += 1;
        let mut value = Vec::new();
        let mut closed = false;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'\\' {
                // Escapes make the runtime name unknowable; fetch every name.
                return None;
            }
            if byte == quote {
                closed = true;
                index += 1;
                break;
            }
            value.push(byte);
            index += 1;
        }
        if !closed {
            return None;
        }
        let mut lookahead = index;
        while lookahead < bytes.len() && bytes[lookahead].is_ascii_whitespace() {
            lookahead += 1;
        }
        if bytes.get(lookahead) != Some(&b')') {
            // `secret('db-' + env)` or `secret('tok' 'en')`.
            return None;
        }
        let Ok(value) = String::from_utf8(value) else {
            return None;
        };
        if !value.is_empty() && !names.contains(&value) {
            names.push(value);
        }
        index = lookahead + 1;
    }
    if names.is_empty() {
        None
    } else {
        Some(names)
    }
}

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, Command},
    sync::{watch, Mutex, OnceCell},
};

// Write bytes explicitly: Windows piped stdout may use an ANSI code page.
const PYTHON_EXECUTABLE_PROBE: &str =
    "import sys; sys.stdout.buffer.write(sys.executable.encode('utf-8'))";

const KERNEL: &str = include_str!("../../../kernels/python/kernel.py");
pub(crate) const RUNTIME: &str = include_str!("../../../runtimes/python/codingpad/__init__.py");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Python,
    Csharp,
    Typescript,
}

impl Language {
    fn supports_connections(self) -> bool {
        matches!(self, Self::Python | Self::Csharp)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KernelEvent {
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RunResult {
    pub ok: bool,
    #[serde(default)]
    pub stopped: bool,
    #[serde(default, rename = "cacheCleared")]
    pub cache_cleared: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExpandResult {
    pub node: Value,
}

/// Generated models and connection details for one run. The connection may
/// contain a vault password and is never written to disk.
#[derive(Clone, Debug)]
pub struct DataContextRun {
    pub source: String,
    pub connection: crate::data_context::ConnectionInfo,
}

/// The connection named by a script's `codingpad:` header, if any.
pub fn attached_connection(source: &str) -> Option<String> {
    store::parse_header(source)?
        .settings
        .get("connection")
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

/// How long schema discovery may take before a run starts without models.
const DATA_CONTEXT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Build the typed models and connection details for a script's header
/// connection. Kernels receive the result as their `db` client; a failure is
/// reported as a message and the script still runs without models.
pub async fn attached_data_context(
    sql: &databases::SqlManager,
    source: &str,
    language: Language,
) -> (Option<DataContextRun>, Option<String>) {
    if !language.supports_connections() {
        return (None, None);
    }
    let Some(name) = attached_connection(source) else {
        return (None, None);
    };
    let generated = tokio::time::timeout(DATA_CONTEXT_TIMEOUT, sql.data_context(&name)).await;
    match generated {
        Ok(Ok(generated)) => {
            let source = match language {
                Language::Python => generated.python,
                Language::Csharp => generated.csharp,
                Language::Typescript => return (None, None),
            };
            (
                Some(DataContextRun {
                    source,
                    connection: generated.connection,
                }),
                None,
            )
        }
        Ok(Err(error)) => (None, Some(format!("Data context unavailable: {error}"))),
        Err(_) => (
            None,
            Some(format!(
                "Data context unavailable: schema discovery timed out after {} seconds.",
                DATA_CONTEXT_TIMEOUT.as_secs()
            )),
        ),
    }
}

struct Slot {
    kernel: Mutex<Option<Kernel>>,
    closed: watch::Sender<bool>,
    stop: watch::Sender<u64>,
}

impl Slot {
    fn new() -> Self {
        Self {
            kernel: Mutex::new(None),
            closed: watch::channel(false).0,
            stop: watch::channel(0).0,
        }
    }
}

type SharedSlot = Arc<Slot>;

pub struct Supervisor {
    uv: PathBuf,
    dotnet: Option<PathBuf>,
    bun: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    kernels: Mutex<HashMap<String, SharedSlot>>,
    shadow_projects: Arc<shadow::ShadowProjects>,
    csharp: OnceCell<csharp::Installation>,
    secrets: Option<Arc<SecretStore>>,
    connections: Option<Arc<databases::ConnectionStore>>,
    eager_dumps: bool,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::with_uv(find_uv())
    }
}

// Desktop launchers (especially Finder) often provide only the system PATH.
fn uv_search_dirs(
    path: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    macos: bool,
) -> Vec<PathBuf> {
    let mut directories: Vec<_> = path
        .as_ref()
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    if let Some(home) = home {
        for directory in [".local/bin", ".cargo/bin"] {
            directories.push(PathBuf::from(&home).join(directory));
        }
    }
    if macos {
        directories.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
    }
    directories
}

pub(crate) fn find_uv() -> PathBuf {
    let name = if cfg!(windows) { "uv.exe" } else { "uv" };
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    uv_search_dirs(std::env::var_os("PATH"), home, cfg!(target_os = "macos"))
        .into_iter()
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

impl Supervisor {
    pub fn with_uv(uv: PathBuf) -> Self {
        Self {
            uv,
            dotnet: None,
            bun: None,
            data_dir: None,
            kernels: Mutex::new(HashMap::new()),
            shadow_projects: Arc::default(),
            csharp: OnceCell::new(),
            secrets: None,
            connections: None,
            eager_dumps: false,
        }
    }

    /// Pin C# execution to one `dotnet` host, as the installer acceptance test
    /// does when the user's own SDK must not be used.
    pub fn with_dotnet(dotnet: PathBuf) -> Self {
        Self {
            uv: find_uv(),
            dotnet: Some(dotnet),
            bun: None,
            data_dir: None,
            kernels: Mutex::new(HashMap::new()),
            shadow_projects: Arc::default(),
            csharp: OnceCell::new(),
            secrets: None,
            connections: None,
            eager_dumps: false,
        }
    }

    /// Pin TypeScript execution to one Bun, as the installer acceptance test
    /// does when the user's own Bun must not be used.
    pub fn with_bun(bun: PathBuf) -> Self {
        Self {
            shadow_projects: Arc::new(shadow::ShadowProjects::with_bun(bun.clone())),
            bun: Some(bun),
            ..Self::default()
        }
    }

    /// Resolve user tools first, then the app-private installation on each startup.
    pub fn with_data_dir(data_dir: PathBuf) -> Self {
        Self {
            data_dir: Some(data_dir.clone()),
            shadow_projects: Arc::new(shadow::ShadowProjects::with_data_dir(data_dir)),
            ..Self::default()
        }
    }

    /// Share the app's secret store with every kernel this supervisor starts.
    pub fn with_data_dir_and_secrets(data_dir: PathBuf, secrets: Arc<SecretStore>) -> Self {
        Self {
            secrets: Some(secrets),
            ..Self::with_data_dir(data_dir)
        }
    }

    /// Use the same live connection store as the desktop connection editor.
    pub fn with_connections(mut self, connections: Arc<databases::ConnectionStore>) -> Self {
        self.connections = Some(connections);
        self
    }

    pub fn shadow_projects(&self) -> Arc<shadow::ShadowProjects> {
        self.shadow_projects.clone()
    }

    /// Ask kernels to materialise lazy values inside each dump instead of
    /// sending expandable placeholders. Text hosts such as `cpad run` cannot
    /// click a preview, so the desktop keeps the default deferred behaviour.
    pub fn with_eager_dumps(mut self) -> Self {
        self.eager_dumps = true;
        self
    }

    fn uv_path(&self) -> PathBuf {
        self.data_dir
            .as_deref()
            .map(toolchains::resolve_uv)
            .unwrap_or_else(|| self.uv.clone())
    }

    fn bun_path(&self) -> PathBuf {
        toolchains::bun_for(self.bun.as_deref(), self.data_dir.as_deref())
    }

    /// A private Bun that cannot start is removed so the next Run reports the
    /// missing toolchain and offers the one-click install again.
    fn discard_broken_bun(&self, bun: &Path) {
        if self.bun.is_some() {
            return;
        }
        if self
            .data_dir
            .as_deref()
            .is_some_and(|data_dir| toolchains::private_bun(data_dir).as_path() == bun)
        {
            let _ = std::fs::remove_file(bun);
        }
    }

    pub async fn run(
        &self,
        script_id: &str,
        source: &str,
        mut emit: impl FnMut(KernelEvent) + Send,
    ) -> Result<RunResult, String> {
        self.run_language(script_id, source, Language::Python, &mut emit)
            .await
    }

    pub async fn run_language(
        &self,
        script_id: &str,
        source: &str,
        language: Language,
        mut emit: impl FnMut(KernelEvent) + Send,
    ) -> Result<RunResult, String> {
        self.run_language_at(script_id, source, language, None, &mut emit)
            .await
    }

    /// Run a saved script with file-relative NuGet configuration or imports.
    pub async fn run_language_at(
        &self,
        script_id: &str,
        source: &str,
        language: Language,
        path: Option<&str>,
        emit: impl FnMut(KernelEvent) + Send,
    ) -> Result<RunResult, String> {
        self.run_language_at_with_data(script_id, source, language, path, None, emit)
            .await
    }

    /// Run a script with generated data-context models available to it.
    pub async fn run_language_at_with_data(
        &self,
        script_id: &str,
        source: &str,
        language: Language,
        path: Option<&str>,
        data: Option<DataContextRun>,
        mut emit: impl FnMut(KernelEvent) + Send,
    ) -> Result<RunResult, String> {
        if script_id.is_empty() {
            return Err("A script id is required".into());
        }
        let slot = self
            .kernels
            .lock()
            .await
            .entry(script_id.into())
            .or_insert_with(|| Arc::new(Slot::new()))
            .clone();
        let mut closed = slot.closed.subscribe();
        let mut stop = slot.stop.subscribe();
        // Vault prompts block. Resolve values off the async thread and let Stop
        // or Close cancel the wait before the kernel slot lock is taken.
        let secrets = match (&self.secrets, language) {
            (Some(store), language) if language.supports_connections() => {
                let store = Arc::clone(store);
                let names = referenced_secret_names(source);
                let snapshot =
                    tokio::task::spawn_blocking(move || store.snapshot_named(names.as_deref()));
                tokio::select! {
                    result = snapshot => result
                        .map_err(|error| format!("Could not read secrets from the vault: {error}"))?,
                    _ = closed.changed() => return Err("Script was closed".into()),
                    _ = stop.changed() => return Ok(RunResult { ok: false, stopped: true, cache_cleared: false }),
                }
            }
            _ => SecretSnapshot::default(),
        };
        let connection = if language.supports_connections() {
            store::parse_header(source)
                .and_then(|header| header.settings.get("connection").cloned())
                .filter(|name| !name.trim().is_empty())
        } else {
            None
        };
        let connection = match connection {
            Some(name) => {
                let connection = self
                    .connections
                    .as_ref()
                    .and_then(|store| store.by_name(&name))
                    .ok_or_else(|| {
                        format!("Connection \"{name}\" was not found. Add it in Connections…")
                    })?;
                let secret = connection
                    .secret
                    .clone()
                    .filter(|name| !name.trim().is_empty());
                let password = if connection.kind != databases::DatabaseKind::Sqlite {
                    if let Some(secret) = secret {
                        let store = self
                            .secrets
                            .clone()
                            .ok_or("The password vault is unavailable")?;
                        let read = tokio::task::spawn_blocking(move || store.get(&secret));
                        Some(tokio::select! {
                            result = read => result.map_err(|error| format!("The password read failed: {error}"))??,
                            _ = closed.changed() => return Err("Script was closed".into()),
                            _ = stop.changed() => return Ok(RunResult { ok: false, stopped: true, cache_cleared: false }),
                        })
                    } else {
                        None
                    }
                } else {
                    None
                };
                Some(json!({"settings": connection, "password": password}))
            }
            None => None,
        };
        let mut guard = slot.kernel.lock().await;
        if *closed.borrow() {
            return Err("Script was closed".into());
        }
        let script = if language == Language::Typescript {
            typescript::parse(source)?
        } else {
            None
        };
        let (typescript_imports, final_expression) = script.map_or((None, None), |script| {
            (Some(script.imports), script.final_expression)
        });
        let mut packages = if language == Language::Python {
            Packages::parse(source)?
        } else {
            Packages::parse("")?
        };
        if language == Language::Python {
            if let Some(kind) = connection
                .as_ref()
                .and_then(|value| value["settings"]["kind"].as_str())
            {
                let driver = match kind {
                    "postgres" => Some("psycopg[binary]>=3.2,<4"),
                    "sqlserver" => Some("mssql-python>=1,<2"),
                    _ => None,
                };
                if let Some(driver) = driver {
                    packages.dependencies.push(driver.to_owned());
                    packages.dependencies.sort();
                    packages.dependencies.dedup();
                    packages.declared = true;
                }
            }
            if let Some(data) = &data {
                packages.declared = true;
                let driver = match data.connection.kind.as_str() {
                    "postgres" => Some("psycopg[binary]>=3.2,<4"),
                    "sqlserver" => Some("mssql-python>=1,<2"),
                    _ => None,
                };
                let dependency_name = |dependency: &str| {
                    dependency
                        .split(['[', '<', '>', '=', ' ', '!', '~'])
                        .next()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                };
                for dependency in ["sqlalchemy>=2.0,<2.2"].into_iter().chain(driver) {
                    let name = dependency_name(dependency);
                    if !packages
                        .dependencies
                        .iter()
                        .any(|item| dependency_name(item) == name)
                    {
                        packages.dependencies.push(dependency.to_owned());
                    }
                }
                packages.dependencies.sort();
                packages.dependencies.dedup();
            }
        }
        let typescript_project = if language == Language::Typescript {
            let prepare = self.shadow_projects.prepare_typescript(script_id, source, typescript_imports, false, |text| {
                emit(KernelEvent {
                    method: "stream".into(),
                    params: json!({"runId": 0, "stream": "stderr", "text": format!("{text}\n")}),
                });
            });
            Some(tokio::select! {
                biased;
                _ = closed.changed() => return Err("Script was closed".into()),
                _ = stop.changed() => return Ok(RunResult { ok: false, stopped: true, cache_cleared: false }),
                result = prepare => result?,
            })
        } else {
            None
        };
        if guard.as_ref().is_some_and(|kernel| {
            kernel.packages != packages
                || kernel.language != language
                || typescript_project.as_ref().is_some_and(|project| {
                    kernel
                        .typescript_project
                        .as_ref()
                        .is_some_and(|current| !Arc::ptr_eq(current, project))
                })
        }) {
            *guard = None;
            emit(KernelEvent {
                method: "stream".into(),
                params: json!({"runId": 0, "stream": "stderr", "text": "Language or dependencies changed; restarting the kernel and clearing cache.\n"}),
            });
        }
        // Stop and Close can cancel preparation before the kernel starts.
        if guard.is_none() {
            let startup = tokio::select! {
                result = async {
                    match language {
                        Language::Csharp => {
                            let (command, files) = csharp::command(
                                &self.csharp,
                                self.data_dir.as_deref(),
                                self.dotnet.as_deref(),
                            ).await?;
                            Kernel::spawn(command, files, packages, language)
                        }
                        Language::Python => Kernel::start(&self.uv_path(), packages, &mut emit).await,
                        Language::Typescript => {
                            let bun = self.bun_path();
                            let started = Kernel::start_typescript(
                                packages,
                                typescript_project.unwrap(),
                                &bun,
                            ).await;
                            if started.is_err() {
                                self.discard_broken_bun(&bun);
                            }
                            started
                        }
                    }
                } => result,
                _ = closed.changed() => return Err("Script was closed".into()),
                _ = stop.changed() => return Ok(RunResult { ok: false, stopped: true, cache_cleared: true }),
            };
            *guard = Some(startup?);
        }
        let kernel = guard.as_mut().unwrap();
        let data = data.map(|data| json!({"source": data.source, "connection": data.connection}));
        let params = json!({"source": source, "path": path, "secrets": secrets.values, "secret_errors": secrets.errors, "connection": connection, "data": data, "eager": self.eager_dumps, "finalExpression": final_expression});
        let result = tokio::select! {
            result = kernel.run(params, &mut emit, &mut stop) => result,
            _ = closed.changed() => Err("Script was closed".into()),
        };
        if result.as_ref().is_ok_and(|result| result.cache_cleared) || result.is_err() {
            *guard = None;
        }
        result
    }

    pub async fn stop(&self, script_id: &str) {
        let slot = self.kernels.lock().await.get(script_id).cloned();
        if let Some(slot) = slot {
            // New subscribers see the current version as already consumed.
            // Always notify runs subscribed before Stop, even before they lock.
            slot.stop.send_modify(|version| *version += 1);
            drop(slot.kernel.lock().await);
        }
    }

    pub async fn reset(&self, script_id: &str) -> Result<(), String> {
        self.stop(script_id).await;
        let slot = self.kernels.lock().await.get(script_id).cloned();
        if let Some(slot) = slot {
            let mut guard = slot.kernel.lock().await;
            if *slot.closed.borrow() {
                return Err("Script was closed".into());
            }
            *guard = None;
        }
        Ok(())
    }

    pub async fn close(&self, script_id: &str) {
        if let Some(slot) = self.kernels.lock().await.remove(script_id) {
            slot.closed.send_replace(true);
            *slot.kernel.lock().await = None;
        }
        self.shadow_projects.close(script_id).await;
    }

    pub async fn expand(
        &self,
        script_id: &str,
        handle: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<ExpandResult, String> {
        self.expand_within(
            script_id,
            handle,
            cursor,
            limit,
            std::time::Duration::from_secs(10),
        )
        .await
    }

    /// Expand with a caller-chosen timeout. Text hosts use a shorter limit when
    /// they have an overall post-run budget.
    pub async fn expand_within(
        &self,
        script_id: &str,
        handle: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
        preview_timeout: std::time::Duration,
    ) -> Result<ExpandResult, String> {
        let node = self
            .call_kernel(
                script_id,
                "expand",
                json!({"handle": handle, "cursor": cursor, "limit": limit}),
                preview_timeout,
            )
            .await?;
        serde_json::from_value(node).map_err(|e| e.to_string())
    }

    /// One page of plain, untruncated cells from a table dump, for export.
    pub async fn export_page(
        &self,
        script_id: &str,
        handle: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<export::ExportPage, String> {
        let page = self
            .call_kernel(
                script_id,
                "export_rows",
                json!({"handle": handle, "cursor": cursor, "limit": limit}),
                // A page runs script code (getters, iterators), as a preview does.
                std::time::Duration::from_secs(30),
            )
            .await?;
        serde_json::from_value(page).map_err(|e| e.to_string())
    }

    async fn call_kernel(
        &self,
        script_id: &str,
        method: &str,
        params: Value,
        preview_timeout: std::time::Duration,
    ) -> Result<Value, String> {
        let slot = {
            let kernels = self.kernels.lock().await;
            kernels.get(script_id).cloned()
        };
        let unavailable =
            || "That preview is no longer available. Run the script again.".to_owned();
        let slot = slot.ok_or_else(unavailable)?;
        // Subscribe before taking the lock so Stop also cancels queued previews.
        let mut closed = slot.closed.subscribe();
        let mut stop = slot.stop.subscribe();
        let mut guard = tokio::select! {
            biased;
            _ = closed.changed() => return Err(unavailable()),
            _ = stop.changed() => return Err(unavailable()),
            guard = slot.kernel.lock() => guard,
        };
        if *closed.borrow() {
            return Err(unavailable());
        }
        let outcome = {
            let kernel = guard.as_mut().ok_or_else(unavailable)?;
            // Getters and iterators can execute arbitrary script code. On
            // cancellation/timeout, discard the process and its partial response.
            tokio::select! {
                biased;
                _ = closed.changed() => Err(CallError::Transport(unavailable())),
                _ = stop.changed() => Err(CallError::Transport("Preview stopped; the kernel and cache were cleared. Run the script again.".into())),
                _ = tokio::time::sleep(preview_timeout) => Err(CallError::Transport("Preview timed out; the kernel and cache were cleared. Run the script again.".into())),
                result = kernel.request(method, params) => result,
            }
        };
        match outcome {
            Ok(value) => Ok(value),
            Err(CallError::Kernel(message)) => Err(message),
            Err(CallError::Transport(message)) => {
                // The kernel died or answered with garbage. Drop it so the next
                // run starts a fresh process instead of failing once.
                *guard = None;
                Err(message)
            }
        }
    }

    pub async fn shutdown(&self) {
        let slots: Vec<_> = self
            .kernels
            .lock()
            .await
            .drain()
            .map(|(_, slot)| slot)
            .collect();
        for slot in &slots {
            slot.closed.send_replace(true);
        }
        for slot in slots {
            *slot.kernel.lock().await = None;
        }
        self.shadow_projects.shutdown().await;
    }
}

struct Kernel {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<Box<dyn AsyncRead + Unpin + Send>>>,
    next_id: u64,
    _files: Arc<TempDir>,
    packages: Packages,
    language: Language,
    // Retain the package directory while this kernel uses it. The manager
    // holds the current project independently of the kernel.
    typescript_project: Option<Arc<typescript::Project>>,
}

impl Kernel {
    fn spawn(
        command: Command,
        files: Arc<TempDir>,
        packages: Packages,
        language: Language,
    ) -> Result<Self, String> {
        let mut child = Self::spawn_child(command, language, Stdio::piped())?;
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        Ok(Self::from_child(
            child, input, output, files, packages, language,
        ))
    }

    async fn start_typescript(
        packages: Packages,
        project: Arc<typescript::Project>,
        bun: &Path,
    ) -> Result<Self, String> {
        let (command, files) = typescript::command(bun, &project)?;
        let language = Language::Typescript;
        let mut child = Self::spawn_child(command, language, Stdio::inherit())?;
        let mut input = child.stdin.take().unwrap();
        let output = typescript::connect(&mut child, &mut input).await?;
        let mut kernel = Self::from_child(child, input, output, files, packages, language);
        kernel.typescript_project = Some(project);
        Ok(kernel)
    }

    fn spawn_child(
        mut command: Command,
        language: Language,
        stdout: Stdio,
    ) -> Result<Child, String> {
        command
            .stdin(Stdio::piped())
            .stdout(stdout)
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        // Let the host handle terminal Ctrl-C and send Stop over the protocol.
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(0x08000000 | 0x00000200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
        command.spawn().map_err(|error| match language {
            Language::Python => format!("Could not start Python: {error}"),
            Language::Csharp => format!("Could not start C# kernel: {error}"),
            Language::Typescript => typescript::bun_not_found(&error)
                .unwrap_or_else(|| format!("Could not start the Bun kernel: {error}")),
        })
    }

    fn from_child(
        child: Child,
        input: ChildStdin,
        output: impl AsyncRead + Unpin + Send + 'static,
        files: Arc<TempDir>,
        packages: Packages,
        language: Language,
    ) -> Self {
        Self {
            input,
            output: BufReader::new(Box::new(output) as Box<dyn AsyncRead + Unpin + Send>).lines(),
            child,
            next_id: 0,
            _files: files,
            packages,
            language,
            typescript_project: None,
        }
    }

    async fn start(
        uv: &PathBuf,
        packages: Packages,
        emit: &mut (impl FnMut(KernelEvent) + Send),
    ) -> Result<Self, String> {
        let mut discover = Command::new(uv);
        #[cfg(unix)]
        discover.process_group(0);
        #[cfg(windows)]
        discover.creation_flags(0x08000000 | 0x00000200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
        discover.kill_on_drop(true);
        let python = discover
            .args(["python", "find", ">=3.10", "--offline", "--no-config"])
            .output().await.map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    "uv was not found. Choose Install uv and run to install a private copy for CodingPad, or install uv on PATH yourself and run again.".into()
                } else { format!("Could not start uv: {e}") }
            })?;
        if !python.status.success() {
            return Err(format!("uv could not find an installed Python. Install Python 3.10+ (for example, `uv python install`) and try again. {}", String::from_utf8_lossy(&python.stderr).trim()));
        }
        let mut executable = String::from_utf8(python.stdout).map_err(|e| e.to_string())?;
        if packages.declared {
            emit(KernelEvent {
                method: "stream".into(),
                params: json!({"runId": 0, "stream": "stderr", "text": "Preparing Python packages (uv cache)…\n"}),
            });
            executable = prepare_packages(uv, &packages, emit).await?;
        }
        let files = tempfile::tempdir().map_err(|e| e.to_string())?;
        let runtime = files.path().join("runtime/codingpad");
        std::fs::create_dir_all(&runtime).map_err(|e| e.to_string())?;
        std::fs::write(runtime.join("__init__.py"), RUNTIME).map_err(|e| e.to_string())?;
        let kernel = files.path().join("kernel.py");
        std::fs::write(&kernel, KERNEL).map_err(|e| e.to_string())?;
        let mut command = Command::new(executable.trim());
        command.args(["-I", "-u", "-X", "utf8"]).arg(kernel);
        Self::spawn(command, Arc::new(files), packages, Language::Python)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, CallError> {
        self.next_id += 1;
        let id = self.next_id;
        let mut request = serde_json::to_vec(
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        )
        .map_err(|e| CallError::Transport(e.to_string()))?;
        request.push(b'\n');
        self.input
            .write_all(&request)
            .await
            .map_err(|e| CallError::Transport(format!("Could not send request to kernel: {e}")))?;
        self.input
            .flush()
            .await
            .map_err(|e| CallError::Transport(format!("Could not flush request to kernel: {e}")))?;
        let message = self.next_message().await?;
        if message.get("id").is_some() {
            return response_result(&message, id);
        }
        Err(CallError::Transport(
            "Unexpected kernel notification while waiting for a preview".into(),
        ))
    }

    async fn next_message(&mut self) -> Result<Value, CallError> {
        let line = self
            .output
            .next_line()
            .await
            .map_err(|e| CallError::Transport(format!("Could not read kernel output: {e}")))?
            .ok_or_else(|| {
                CallError::Transport(
                    "Kernel exited before finishing. Run again to start a fresh kernel.".to_owned(),
                )
            })?;
        serde_json::from_str(&line)
            .map_err(|e| CallError::Transport(format!("Invalid kernel message: {e}")))
    }

    async fn run(
        &mut self,
        params: Value,
        emit: &mut (impl FnMut(KernelEvent) + Send),
        stop: &mut watch::Receiver<u64>,
    ) -> Result<RunResult, String> {
        self.next_id += 1;
        let id = self.next_id;
        let mut request = serde_json::to_vec(
            &json!({"jsonrpc": "2.0", "id": id, "method": "run", "params": params}),
        )
        .map_err(|e| e.to_string())?;
        request.push(b'\n');
        self.input
            .write_all(&request)
            .await
            .map_err(|e| format!("Could not send script to kernel: {e}"))?;
        self.input.flush().await.map_err(|e| e.to_string())?;
        let mut stopped = false;
        let mut deadline = None;
        loop {
            let message = tokio::select! {
                message = self.next_message() => message.map_err(CallError::message)?,
                _ = stop.changed(), if !stopped => {
                    stopped = true;
                    let message = json!({"jsonrpc": "2.0", "method": "cancel", "params": {"runId": id}});
                    self.input.write_all(format!("{message}\n").as_bytes()).await.map_err(|e| e.to_string())?;
                    self.input.flush().await.map_err(|e| e.to_string())?;
                    deadline = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(2));
                    continue;
                }
                _ = async { tokio::time::sleep_until(deadline.unwrap()).await }, if deadline.is_some() => {
                    emit(KernelEvent { method: "run_finished".into(), params: json!({"runId": id, "ok": false}) });
                    return Ok(RunResult { ok: false, stopped: true, cache_cleared: true });
                }
            };
            if message["jsonrpc"] != "2.0" {
                return Err("Invalid kernel protocol version".into());
            }
            if message.get("id").is_some() {
                if message["id"] != id {
                    return Err("Unexpected kernel response id".into());
                }
                if message["error"]["code"] == -32603 {
                    let text = message["error"]["message"]
                        .as_str()
                        .unwrap_or("Kernel run failed");
                    emit(KernelEvent {
                        method: "stream".into(),
                        params: json!({"runId": id, "stream": "stderr", "text": format!("{text}\n")}),
                    });
                    emit(KernelEvent {
                        method: "run_finished".into(),
                        params: json!({"runId": id, "ok": false}),
                    });
                    return Ok(RunResult {
                        ok: false,
                        stopped,
                        cache_cleared: false,
                    });
                }
                let value = response_result(&message, id).map_err(CallError::message)?;
                let mut result: RunResult =
                    serde_json::from_value(value).map_err(|e| e.to_string())?;
                result.stopped = stopped && !result.ok;
                return Ok(result);
            }
            if message["params"]["runId"] != id {
                return Err("Unexpected kernel notification run id".into());
            }
            emit(
                serde_json::from_value(message)
                    .map_err(|e| format!("Invalid kernel notification: {e}"))?,
            );
        }
    }
}

// A stable launcher per dependency set lets uv manage persistent script
// environments, including locking concurrent installs. Launch Python directly
// afterwards so Stop/Close own the actual kernel process on every platform.
async fn prepare_packages(
    uv: &PathBuf,
    packages: &Packages,
    emit: &mut (impl FnMut(KernelEvent) + Send),
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    let mut metadata = packages.clone();
    metadata.requires_python = Some(
        packages
            .requires_python
            .as_ref()
            .map(|spec| format!(">=3.10,{spec}"))
            .unwrap_or_else(|| ">=3.10".into()),
    );
    let metadata = toml::to_string(&metadata).map_err(|e| e.to_string())?;
    let launcher = format!(
        "# /// script\n{}# ///\n{PYTHON_EXECUTABLE_PROBE}\n",
        metadata
            .lines()
            .map(|line| format!("# {line}\n"))
            .collect::<String>()
    );
    let cache = dirs::cache_dir()
        .ok_or("Could not locate the user cache directory")?
        .join("codingpad/python-packages-v1");
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let path = cache.join(format!("{:x}.py", Sha256::digest(launcher.as_bytes())));
    if !path.exists() {
        let mut file = tempfile::NamedTempFile::new_in(&cache).map_err(|e| e.to_string())?;
        file.write_all(launcher.as_bytes())
            .map_err(|e| e.to_string())?;
        match file.persist_noclobber(&path) {
            Ok(_) => (),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.to_string()),
        }
    }
    let mut command = Command::new(uv);
    command
        .args([
            "run",
            "--no-project",
            "--no-config",
            "--no-env-file",
            "--no-python-downloads",
            "--color",
            "never",
            "--script",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x08000000 | 0x00000200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not prepare Python packages: {e}"))?;
    let stderr = child.stderr.take().unwrap();
    let progress = stream_package_progress(stderr, emit);
    let (output, ()) = tokio::try_join!(
        async { child.wait_with_output().await.map_err(|e| e.to_string()) },
        progress
    )?;
    if !output.status.success() {
        let driver = packages
            .dependencies
            .iter()
            .any(|name| name.starts_with("psycopg[") || name.starts_with("mssql-python"));
        let detail = if driver {
            " The attached connection needs a database driver, which may require network access on first use."
        } else {
            ""
        };
        return Err(format!("Python package preparation failed. See uv output in Results; check the dependencies, requires-python and network connection. Python 3.10+ must already be installed.{detail}"));
    }
    let executable = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    if !std::path::Path::new(executable.trim()).is_file() {
        return Err("uv did not return a usable Python interpreter".into());
    }
    Ok(executable)
}

async fn stream_package_progress(
    reader: impl tokio::io::AsyncRead + Unpin,
    emit: &mut (impl FnMut(KernelEvent) + Send),
) -> Result<(), String> {
    // Buffer complete lines before decoding: preserve split UTF-8 sequences,
    // while tolerating invalid bytes relayed from package build backends.
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    while reader
        .read_until(b'\n', &mut line)
        .await
        .map_err(|e| e.to_string())?
        != 0
    {
        emit(KernelEvent {
            method: "stream".into(),
            params: json!({"runId": 0, "stream": "stderr", "text": String::from_utf8_lossy(&line)}),
        });
        line.clear();
    }
    Ok(())
}

enum CallError {
    /// A JSON-RPC error reported by a live kernel; keep the process.
    Kernel(String),
    /// I/O, EOF or malformed protocol; the process cannot be trusted.
    Transport(String),
}

impl CallError {
    fn message(self) -> String {
        match self {
            Self::Kernel(message) | Self::Transport(message) => message,
        }
    }
}

fn response_result(message: &Value, id: u64) -> Result<Value, CallError> {
    if message["jsonrpc"] != "2.0" {
        return Err(CallError::Transport(
            "Invalid kernel protocol version".into(),
        ));
    }
    if message["id"] != id {
        return Err(CallError::Transport("Unexpected kernel response id".into()));
    }
    if let Some(error) = message.get("error") {
        let text = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("The kernel could not complete the request");
        return Err(CallError::Kernel(text.to_owned()));
    }
    Ok(message["result"].clone())
}

impl Drop for Kernel {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_bun_reports_how_to_run_typescript() {
        let files = Arc::new(tempfile::tempdir().unwrap());
        let command = Command::new(files.path().join("missing-bun"));
        let result = Kernel::spawn(
            command,
            files,
            Packages::parse("").unwrap(),
            Language::Typescript,
        );
        assert!(
            matches!(result, Err(message) if message.starts_with("Bun was not found. Choose Install Bun"))
        );
    }

    #[tokio::test]
    async fn a_private_bun_that_cannot_start_is_discarded() {
        let data = tempfile::tempdir().unwrap();
        let private = toolchains::private_bun(data.path());
        std::fs::create_dir_all(private.parent().unwrap()).unwrap();
        std::fs::write(&private, "broken").unwrap();
        let supervisor = Supervisor::with_data_dir(data.path().to_path_buf());
        supervisor.discard_broken_bun(&private);
        assert!(!private.exists(), "the next run can offer a reinstall");
        // An explicit pin, as tests and embedders set, belongs to the caller.
        let pinned = data.path().join("pinned-bun");
        std::fs::write(&pinned, "broken").unwrap();
        let supervisor = Supervisor::with_bun(pinned.clone());
        supervisor.discard_broken_bun(&pinned);
        assert!(pinned.exists());
    }

    #[test]
    fn secret_names_are_read_lexically_or_not_at_all() {
        for source in [
            "x = 1; import hlp",
            "if True: import hlp",
            "try: import hlp",
            "__import__('hlp').run()",
            "import\thlp",
            "secret('a')\nx = 1; import hlp",
            "importlib.import_module('hlp').run()",
        ] {
            assert_eq!(referenced_secret_names(source), None, "{source}");
        }
        assert_eq!(referenced_secret_names("print(1)"), Some(vec![]));
        assert_eq!(
            referenced_secret_names("from codingpad import secret\nsecret('token')"),
            None,
            "imports can hide calls, so every secret is read"
        );
        assert_eq!(
            referenced_secret_names("Secret(\"a\"); Secret( 'b' ); Secret(\"a\")"),
            Some(vec!["a".into(), "b".into()])
        );
        assert_eq!(referenced_secret_names("secret(name)"), None);
        assert_eq!(referenced_secret_names("secret('db-' + env)"), None);
        assert_eq!(
            referenced_secret_names("secret('token'); s = secret; s('other')"),
            None
        );
        assert_eq!(referenced_secret_names("secret('tok' 'en')"), None);
        assert_eq!(
            referenced_secret_names("secret('clé')"),
            Some(vec!["clé".into()])
        );
        assert_eq!(
            referenced_secret_names("import hlp\nhlp.run()"),
            None,
            "a module may call secret()"
        );
    }

    #[tokio::test]
    async fn stop_during_a_vault_read_does_not_claim_the_cache_was_cleared() {
        use crate::secrets::Vault;
        use std::time::Duration;

        struct SlowVault;
        impl Vault for SlowVault {
            fn get(&self, _: &str) -> Result<Option<String>, String> {
                std::thread::sleep(Duration::from_millis(300));
                Ok(None)
            }
            fn set(&self, _: &str, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn delete(&self, _: &str) -> Result<(), String> {
                Ok(())
            }
        }

        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("secrets.json"), "[\"token\"]").unwrap();
        let store = Arc::new(SecretStore::with_vault(
            directory.path(),
            Arc::new(SlowVault),
        ));
        let supervisor =
            Supervisor::with_data_dir_and_secrets(directory.path().to_path_buf(), store);
        let mut run = std::pin::pin!(supervisor.run("slow-secret", "secret('token')", |_| {}));
        let result = tokio::select! {
            result = &mut run => result,
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                supervisor.stop("slow-secret").await;
                run.await
            }
        };
        let result = result.unwrap();
        assert!(result.stopped);
        assert!(
            !result.cache_cleared,
            "no kernel existed, so no cache was cleared"
        );
    }

    #[tokio::test]
    async fn interpreter_probe_writes_utf8_with_an_ascii_stdout_encoding() {
        let expected = "C:/José/用户/python.exe";
        let output = Command::new(find_uv())
            .args(["run", "--offline", "--no-project", "--no-config", "--no-python-downloads", "python", "-I", "-c",
                "import sys; sys.executable = sys.argv[1]; sys.stdout.reconfigure(encoding='ascii'); exec(sys.argv[2])",
                expected, PYTHON_EXECUTABLE_PROBE])
            .output().await.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    }

    #[tokio::test]
    async fn package_progress_preserves_unicode_split_across_reads() {
        // A one-byte pipe forces every multibyte character across read boundaries.
        let (mut writer, reader) = tokio::io::duplex(1);
        let expected = "error: × failure\n╰─▶ dependency missing\n";
        let mut events = Vec::new();
        let mut emit = |event| events.push(event);
        let write = async {
            writer.write_all(expected.as_bytes()).await.unwrap();
            writer.shutdown().await.unwrap();
        };
        let (result, ()) = tokio::join!(stream_package_progress(reader, &mut emit), write);
        result.unwrap();
        let text: String = events
            .iter()
            .map(|event| event.params["text"].as_str().unwrap())
            .collect();
        assert_eq!(text, expected);
    }

    #[tokio::test]
    async fn package_progress_tolerates_invalid_bytes_and_preserves_final_line() {
        let input = b"build backend: \xff\nactual uv error: \xfe";
        let mut events = Vec::new();
        stream_package_progress(&input[..], &mut |event| events.push(event))
            .await
            .unwrap();
        let text: String = events
            .iter()
            .map(|event| event.params["text"].as_str().unwrap())
            .collect();
        assert_eq!(text, "build backend: \u{fffd}\nactual uv error: \u{fffd}");
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn stop_notifies_a_run_subscribed_before_taking_kernel_lock() {
        let supervisor = Supervisor::default();
        let slot = Arc::new(Slot::new());
        supervisor
            .kernels
            .lock()
            .await
            .insert("one".into(), slot.clone());
        // Reproduce the window between run's subscription and lock acquisition.
        let pending_run = slot.stop.subscribe();
        supervisor.stop("one").await;
        assert!(pending_run.has_changed().unwrap());
        let next_run = slot.stop.subscribe();
        assert!(!next_run.has_changed().unwrap());
    }

    #[tokio::test]
    async fn stop_and_reset_succeed_when_the_toolchain_becomes_unavailable() {
        let mut supervisor = Supervisor::default();
        assert!(supervisor.run("one", "42", |_| {}).await.unwrap().ok);
        supervisor.uv = PathBuf::from("/codingpad-missing-toolchain/uv");
        let (ready, started) = tokio::sync::oneshot::channel();
        let mut ready = Some(ready);
        let run = supervisor.run("one", "import signal, time\nsignal.signal(signal.SIGINT, signal.SIG_IGN)\nprint('ready', flush=True)\ntime.sleep(60)", |event| {
            if event.method == "stream" && event.params["text"].as_str().is_some_and(|text| text.contains("ready")) {
                if let Some(ready) = ready.take() { let _ = ready.send(()); }
            }
        });
        let stop = async {
            started.await.unwrap();
            supervisor.stop("one").await;
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(6), async {
            tokio::join!(run, stop)
        })
        .await
        .unwrap();
        let result = result.unwrap();
        assert!(result.stopped && result.cache_cleared);
        supervisor.reset("one").await.unwrap();
        assert!(supervisor
            .run("one", "42", |_| {})
            .await
            .unwrap_err()
            .contains("uv was not found"));
    }

    #[tokio::test]
    async fn timed_out_preview_discards_the_kernel_and_the_next_run_recovers() {
        let supervisor = Supervisor::default();
        let mut events = Vec::new();
        supervisor.run_language("timeout", "new Item().Dump(); Environment.ProcessId.Dump(); class Item { public int Value { get { Thread.Sleep(Timeout.Infinite); return 0; } } }", Language::Csharp, |event| events.push(event)).await.unwrap();
        let nodes: Vec<_> = events
            .iter()
            .filter(|event| event.method == "dump")
            .map(|event| &event.params["node"])
            .collect();
        let handle = nodes[0]["fields"][0]["value"]["handle"].as_str().unwrap();
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            supervisor.expand_within(
                "timeout",
                handle,
                None,
                None,
                std::time::Duration::from_millis(100),
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.contains("Preview timed out"));
        assert!(error.contains("Run the script again"));
        let mut recovered = Vec::new();
        supervisor
            .run_language(
                "timeout",
                "Environment.ProcessId.Dump();",
                Language::Csharp,
                |event| recovered.push(event),
            )
            .await
            .unwrap();
        let pid = &recovered
            .iter()
            .find(|event| event.method == "dump")
            .unwrap()
            .params["node"]["v"];
        assert_ne!(pid, &nodes[1]["v"]);
        supervisor.shutdown().await;
    }

    #[test]
    fn finder_path_includes_both_homebrew_prefixes_after_user_tools() {
        let path = std::env::join_paths(["/usr/bin", "/bin"]).unwrap();
        let paths = uv_search_dirs(Some(path), Some("/Users/test".into()), true);
        assert_eq!(paths[0], PathBuf::from("/usr/bin"));
        assert!(paths.contains(&PathBuf::from("/Users/test/.local/bin")));
        assert!(paths.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(paths.contains(&PathBuf::from("/usr/local/bin")));
        assert!(!uv_search_dirs(None, None, false).contains(&PathBuf::from("/opt/homebrew/bin")));
    }
}
