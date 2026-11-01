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
    collections::{HashMap, VecDeque},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicI64, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tempfile::TempDir;
use tokio::{
    io::{
        AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, Lines,
    },
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{oneshot, Mutex},
    time::timeout,
};
use url::Url;

use crate::{find_uv, RUNTIME};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
const DIAGNOSTICS_METHOD: &str = "textDocument/publishDiagnostics";

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
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

pub struct LanguageServers {
    uv: PathBuf,
    servers: Mutex<HashMap<String, Arc<LspServer>>>,
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
            servers: Mutex::new(HashMap::new()),
        }
    }

    /// Start (or replace) the Python language server for one script.
    pub async fn start_python(
        &self,
        script_id: &str,
        source: &str,
        script_path: Option<&str>,
        emit: impl Fn(LspEvent) + Send + Sync + 'static,
    ) -> Result<LspStartResult, String> {
        if script_id.is_empty() {
            return Err("A script id is required".into());
        }
        let dependencies = inline_dependencies(source);
        let workspace = PythonWorkspace::create(source, script_path)?;
        let server = LspServer::start(&self.uv, &dependencies, workspace, Arc::new(emit)).await?;

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
        Ok(LspStartResult {
            uri: server.document_uri.clone(),
        })
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
        if !method.starts_with("textDocument/") {
            return Err("Only text-document language-server requests are allowed".into());
        }
        self.server(script_id).await?.request(method, params).await
    }

    pub async fn stop(&self, script_id: &str) {
        let server = self.servers.lock().await.remove(script_id);
        if let Some(server) = server {
            server.stop().await;
        }
    }

    pub async fn shutdown(&self) {
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
    child: Mutex<Child>,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    next_id: AtomicU64,
    version: AtomicI64,
    document_uri: String,
    workspace_uri: String,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    _workspace: PythonWorkspace,
}

impl LspServer {
    async fn start(
        uv: &Path,
        dependencies: &[String],
        workspace: PythonWorkspace,
        events: EventSink,
    ) -> Result<Arc<Self>, String> {
        let mut command = server_command(uv, dependencies);
        command
            .current_dir(&workspace.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Ok(paths) = std::env::join_paths(&workspace.extra_paths) {
            command.env("PYTHONPATH", paths);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        let mut child = command.spawn().map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                "uv was not found. Install uv from https://docs.astral.sh/uv/getting-started/installation/ and reopen this tab to enable code intelligence.".to_owned()
            } else {
                format!("Could not start Python code intelligence: {error}")
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
        let stdin = Arc::new(Mutex::new(stdin));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));

        let server = Arc::new(Self {
            child: Mutex::new(child),
            stdin: stdin.clone(),
            pending: pending.clone(),
            next_id: AtomicU64::new(1),
            version: AtomicI64::new(0),
            document_uri: workspace.document_uri.clone(),
            workspace_uri: workspace.root_uri.clone(),
            stderr_tail: stderr_tail.clone(),
            _workspace: workspace,
        });

        let response_config = ResponseConfig {
            workspace_uri: server.workspace_uri.clone(),
            extra_paths: server._workspace.extra_paths.clone(),
        };
        tokio::spawn(read_loop(
            BufReader::new(stdout),
            pending,
            events,
            stdin,
            response_config,
        ));
        tokio::spawn(read_stderr(stderr, stderr_tail));
        Ok(server)
    }

    async fn initialize(&self) -> Result<(), String> {
        let params = json!({
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
                            "insertReplaceSupport": true
                        }
                    },
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "publishDiagnostics": {
                        "relatedInformation": true,
                        "versionSupport": true
                    }
                },
                "workspace": {
                    "workspaceFolders": true,
                    "configuration": true
                }
            }
        });
        self.request_with_timeout("initialize", params, INITIALIZE_TIMEOUT)
            .await?;
        self.notify("initialized", json!({})).await
    }

    async fn did_open(&self, source: &str) -> Result<(), String> {
        self.version.store(1, Ordering::Relaxed);
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": &self.document_uri,
                    "languageId": "python",
                    "version": 1,
                    "text": source
                }
            }),
        )
        .await
    }

    async fn did_change(&self, source: &str) -> Result<(), String> {
        let version = self.version.fetch_add(1, Ordering::Relaxed) + 1;
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": &self.document_uri, "version": version },
                "contentChanges": [{ "text": source }]
            }),
        )
        .await
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        self.request_with_timeout(method, params, REQUEST_TIMEOUT)
            .await
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
        if let Err(error) = self.write(&message).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        match timeout(wait, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(
                "The Python language server stopped before answering. Reopen the tab to restart it."
                    .into(),
            ),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(format!(
                    "The Python language server did not answer {method} in time. Reopen the tab to retry."
                ))
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
        .await
    }

    async fn write(&self, message: &Value) -> Result<(), String> {
        let mut stdin = self.stdin.lock().await;
        write_message(&mut *stdin, message).await
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
        let _ = self
            .request_with_timeout("shutdown", Value::Null, Duration::from_millis(500))
            .await;
        let _ = self.notify("exit", Value::Null).await;
        let mut child = self.child.lock().await;
        if timeout(Duration::from_millis(500), child.wait())
            .await
            .is_err()
        {
            let _ = child.start_kill();
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
    fn create(source: &str, script_path: Option<&str>) -> Result<Self, String> {
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

        let script = root.join("script.py");
        fs::write(&script, source).map_err(|error| {
            format!("Could not stage the script for code intelligence: {error}")
        })?;

        let mut extra_paths = vec![runtime.to_string_lossy().into_owned()];
        if let Some(parent) = script_path.and_then(|path| Path::new(path).parent()) {
            if parent.is_dir() {
                extra_paths.push(parent.to_string_lossy().into_owned());
            }
        }

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
    workspace_uri: String,
    extra_paths: Vec<String>,
}

async fn read_loop(
    mut stdout: BufReader<ChildStdout>,
    pending: Pending,
    events: EventSink,
    stdin: Arc<Mutex<ChildStdin>>,
    config: ResponseConfig,
) {
    loop {
        let message = match read_message(&mut stdout).await {
            Ok(Some(message)) => message,
            Ok(None) | Err(_) => break,
        };
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
                let _ = write_message(&mut *stdin.lock().await, &response).await;
            }
            (Some(method), None) => {
                if method == DIAGNOSTICS_METHOD {
                    events(LspEvent {
                        method: method.to_owned(),
                        params: message["params"].clone(),
                    });
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

    let mut pending = pending.lock().await;
    for (_, sender) in pending.drain() {
        let _ = sender.send(Err(
            "The Python language server stopped. Reopen the tab to restart it.".to_owned(),
        ));
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
                    _ => json!({}),
                }
            })
            .collect(),
    )
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
    command.args(["tool", "run", "--from", "basedpyright"]);
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

fn path_to_uri(path: &Path) -> Result<String, String> {
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
            break;
        }
        body.push_str(content);
        body.push('\n');
    }
    if !inside {
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
    fn answers_configuration_for_python_sections() {
        let config = ResponseConfig {
            workspace_uri: "file:///tmp/workspace".into(),
            extra_paths: vec!["/tmp/runtime".into()],
        };
        let result = configuration_result(
            &json!({"params": {"items": [{"section": "python"}, {"section": "python.analysis"}, {"section": "other"}]}}),
            &config,
        );
        assert_eq!(result[0]["analysis"]["extraPaths"][0], "/tmp/runtime");
        assert_eq!(result[1]["extraPaths"][0], "/tmp/runtime");
        assert_eq!(result[2], json!({}));
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
