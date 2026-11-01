//! Desktop-independent supervisor for one warm Python process per script.
pub mod store;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashMap, path::PathBuf, process::Stdio, sync::Arc};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{watch, Mutex},
};

const KERNEL: &str = include_str!("../../../kernels/python/kernel.py");
const RUNTIME: &str = include_str!("../../../runtimes/python/codingpad/__init__.py");

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
    kernels: Mutex<HashMap<String, SharedSlot>>,
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

fn find_uv() -> PathBuf {
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
            kernels: Mutex::new(HashMap::new()),
        }
    }

    pub async fn run(
        &self,
        script_id: &str,
        source: &str,
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
        let mut guard = slot.kernel.lock().await;
        if *closed.borrow() {
            return Err("Script was closed".into());
        }
        if guard.is_none() {
            *guard = Some(Kernel::start(&self.uv).await?);
        }
        let result = tokio::select! {
            result = guard.as_mut().unwrap().run(source, &mut emit, &mut stop) => result,
            _ = closed.changed() => Err("Script was closed".into()),
        };
        if result.as_ref().is_ok_and(|result| result.cache_cleared) {
            *guard = None;
        }
        if result.is_err() {
            // Dropping the child kills it. Next run starts a fresh kernel.
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
    }

    pub async fn expand(
        &self,
        script_id: &str,
        handle: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<ExpandResult, String> {
        let slot = {
            let kernels = self.kernels.lock().await;
            kernels.get(script_id).cloned()
        };
        let unavailable =
            || "That preview is no longer available. Run the script again.".to_owned();
        let slot = slot.ok_or_else(unavailable)?;
        let mut guard = slot.kernel.lock().await;
        let outcome = {
            let kernel = guard.as_mut().ok_or_else(unavailable)?;
            kernel
                .request(
                    "expand",
                    json!({"handle": handle, "cursor": cursor, "limit": limit}),
                )
                .await
        };
        match outcome {
            Ok(node) => serde_json::from_value(node).map_err(|e| e.to_string()),
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
    }
}

struct Kernel {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    _files: TempDir,
}

impl Kernel {
    async fn start(uv: &PathBuf) -> Result<Self, String> {
        let mut discover = Command::new(uv);
        #[cfg(windows)]
        discover.creation_flags(0x08000000); // CREATE_NO_WINDOW, also for uv
        let python = discover
            .args(["python", "find", ">=3.10", "--offline", "--no-config"])
            .output().await.map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    "uv was not found. Install uv from https://docs.astral.sh/uv/getting-started/installation/ and restart CodingPad with uv on PATH.".into()
                } else { format!("Could not start uv: {e}") }
            })?;
        if !python.status.success() {
            return Err(format!("uv could not find an installed Python. Install Python 3.10+ (for example, `uv python install`) and try again. {}", String::from_utf8_lossy(&python.stderr).trim()));
        }
        let executable = String::from_utf8(python.stdout).map_err(|e| e.to_string())?;
        let files = tempfile::tempdir().map_err(|e| e.to_string())?;
        let runtime = files.path().join("runtime/codingpad");
        std::fs::create_dir_all(&runtime).map_err(|e| e.to_string())?;
        std::fs::write(runtime.join("__init__.py"), RUNTIME).map_err(|e| e.to_string())?;
        let kernel = files.path().join("kernel.py");
        std::fs::write(&kernel, KERNEL).map_err(|e| e.to_string())?;
        let mut command = Command::new(executable.trim());
        command
            .args(["-I", "-u", "-X", "utf8"])
            .arg(kernel)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        let mut child = command
            .spawn()
            .map_err(|e| format!("Could not start Python: {e}"))?;
        Ok(Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()).lines(),
            child,
            next_id: 0,
            _files: files,
        })
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
            .map_err(|e| CallError::Transport(format!("Could not send request to Python: {e}")))?;
        self.input
            .flush()
            .await
            .map_err(|e| CallError::Transport(e.to_string()))?;
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
            .map_err(|e| CallError::Transport(format!("Could not read Python output: {e}")))?
            .ok_or_else(|| {
                CallError::Transport(
                    "Python kernel exited before finishing. Run again to start a fresh kernel."
                        .to_owned(),
                )
            })?;
        serde_json::from_str(&line)
            .map_err(|e| CallError::Transport(format!("Invalid kernel message: {e}")))
    }

    async fn run(
        &mut self,
        source: &str,
        emit: &mut (impl FnMut(KernelEvent) + Send),
        stop: &mut watch::Receiver<u64>,
    ) -> Result<RunResult, String> {
        self.next_id += 1;
        let id = self.next_id;
        let mut request = serde_json::to_vec(
            &json!({"jsonrpc": "2.0", "id": id, "method": "run", "params": {"source": source}}),
        )
        .map_err(|e| e.to_string())?;
        request.push(b'\n');
        self.input
            .write_all(&request)
            .await
            .map_err(|e| format!("Could not send script to Python: {e}"))?;
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
