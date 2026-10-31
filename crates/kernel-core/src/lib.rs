//! Desktop-independent supervisor for one warm Python process per script.
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
}

struct Slot {
    kernel: Mutex<Option<Kernel>>,
    closed: watch::Sender<bool>,
}

impl Slot {
    fn new() -> Self {
        Self {
            kernel: Mutex::new(None),
            closed: watch::channel(false).0,
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

// Desktop launchers may omit ~/.local/bin from PATH, even when uv is installed.
fn find_uv() -> PathBuf {
    let name = if cfg!(windows) { "uv.exe" } else { "uv" };
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    if let Some(home) = home {
        for directory in [".local/bin", ".cargo/bin"] {
            let candidate = PathBuf::from(&home).join(directory).join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from(name)
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
        let mut guard = slot.kernel.lock().await;
        if *closed.borrow() {
            return Err("Script was closed".into());
        }
        if guard.is_none() {
            *guard = Some(Kernel::start(&self.uv).await?);
        }
        let result = tokio::select! {
            result = guard.as_mut().unwrap().run(source, &mut emit) => result,
            _ = closed.changed() => Err("Script was closed".into()),
        };
        if result.is_err() {
            // Dropping the child kills it. Next run starts a fresh kernel.
            *guard = None;
        }
        result
    }

    pub async fn close(&self, script_id: &str) {
        if let Some(slot) = self.kernels.lock().await.remove(script_id) {
            slot.closed.send_replace(true);
            *slot.kernel.lock().await = None;
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
        let python = Command::new(uv)
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

    async fn run(
        &mut self,
        source: &str,
        emit: &mut (impl FnMut(KernelEvent) + Send),
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
        loop {
            let line = self
                .output
                .next_line()
                .await
                .map_err(|e| format!("Could not read Python output: {e}"))?
                .ok_or_else(|| {
                    "Python kernel exited before finishing. Run again to start a fresh kernel."
                        .to_owned()
                })?;
            let message: Value =
                serde_json::from_str(&line).map_err(|e| format!("Invalid kernel message: {e}"))?;
            if message["jsonrpc"] != "2.0" {
                return Err("Invalid kernel protocol version".into());
            }
            if message.get("id").is_some() {
                if message["id"] != id {
                    return Err("Unexpected kernel response id".into());
                }
                if let Some(error) = message.get("error") {
                    return Err(format!("Kernel request failed: {error}"));
                }
                return serde_json::from_value(message["result"].clone())
                    .map_err(|e| e.to_string());
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

impl Drop for Kernel {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}
