//! Builds and runs an editor snapshot in a reusable Cargo shadow project.
mod build_cache;
mod debug;
mod dumps;
mod manifest;
mod panic;
mod source;

pub(crate) use build_cache::BuildCache;
pub(crate) use dumps::Dumps;
pub(crate) use manifest::{frontmatter, Manifest};
pub(crate) use source::FILE as SCRIPT_FILE;

use crate::{shadow::ShadowProject, KernelEvent, RunResult};
use debug::{Snapshot, ValueNode};
use panic::Panic;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use source::Source;
use std::{
    collections::VecDeque,
    ffi::OsString,
    io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines},
    net::{TcpListener, TcpStream},
    process::{Child, Command},
    sync::watch,
    time::Instant,
};

pub(super) const RUNTIME_MANIFEST: &str =
    include_str!("../../../runtimes/rust/codingpad/Cargo.toml");
pub(super) const RUNTIME: &str = include_str!("../../../runtimes/rust/codingpad/src/lib.rs");
pub(super) const CACHE_RUNTIME: &str =
    include_str!("../../../runtimes/rust/codingpad/src/cache.rs");
pub(super) const RICH_RUNTIME: &str = include_str!("../../../runtimes/rust/codingpad/src/rich.rs");
const STOP_GRACE: Duration = Duration::from_secs(2);
const MISSING: &str =
    "Rust was not found. Install the stable Rust toolchain (Cargo and rustc) from https://rustup.rs/ and run the script again.";

pub(super) fn stopped() -> RunResult {
    RunResult {
        ok: false,
        stopped: true,
        cache_cleared: false,
    }
}

pub(super) fn cache_key(script_id: &str) -> String {
    use sha2::{Digest, Sha256};
    // Script IDs may contain paths or use characters unavailable in file names.
    format!("{:x}", Sha256::digest(script_id.as_bytes()))
}

pub(super) fn retire_cache(path: Option<&Path>) -> Result<Option<tempfile::TempDir>, String> {
    let Some(path) = path else { return Ok(None) };
    let error = |error| format!("Could not clear the Rust script cache: {error}");
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(failure) => return Err(error(failure)),
    }
    let retired = tempfile::tempdir_in(path.parent().unwrap()).map_err(error)?;
    match std::fs::rename(path, retired.path().join("cache")) {
        Ok(()) => Ok(Some(retired)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(failure) => Err(error(failure)),
    }
}

pub(super) async fn remove_retired_cache(retired: Option<tempfile::TempDir>) -> Result<(), String> {
    if let Some(retired) = retired {
        tokio::task::spawn_blocking(move || std::fs::remove_dir_all(retired.path()))
            .await
            .map_err(|error| format!("Could not clear the Rust script cache: {error}"))?
            .map_err(|error| format!("Could not clear the Rust script cache: {error}"))?;
    }
    Ok(())
}

fn cargo() -> PathBuf {
    let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(std::env::var_os("CARGO_HOME").map(|path| PathBuf::from(path).join("bin")))
        .chain(dirs::home_dir().map(|home| home.join(".cargo/bin")))
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
        .and_then(|path| std::path::absolute(path).ok())
        .unwrap_or_else(|| PathBuf::from(name))
}

pub(super) struct RunPaths<'a> {
    pub(super) project: &'a ShadowProject,
    pub(super) script: Option<&'a Path>,
    pub(super) values: &'a Path,
}

pub(super) async fn run(
    builds: &Arc<BuildCache>,
    source: &str,
    paths: RunPaths<'_>,
    eager: bool,
    stop: &mut watch::Receiver<u64>,
    emit: &mut (impl FnMut(KernelEvent) + Send),
) -> Result<(RunResult, Dumps), String> {
    let RunPaths {
        project,
        script,
        values,
    } = paths;
    let name = script
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("script.rs");
    let mut events = Events {
        emit,
        dumps: Dumps::new(eager),
        views: VecDeque::new(),
        latest: VecDeque::new(),
        panic: None,
        main_unwound: false,
    };
    let manifest = match Manifest::parse(source, script) {
        Ok(manifest) => manifest,
        Err(error) => {
            let text = error.to_string();
            let line = error.line;
            events.send(
                "diagnostics",
                json!({"diagnostics": [
                    {"message": text, "line": line, "column": 1, "endLine": line, "endColumn": 2}
                ]}),
            );
            events.error("Cargo-script frontmatter error", &text, vec![
                json!({"file": name, "line": line, "column": 1, "function": "<manifest>", "text": "", "script": true}),
            ]);
            return Ok((events.finish(false), events.dumps));
        }
    };
    let source =
        Source::with_frontmatter(source, manifest.frontmatter.clone(), &manifest.runtime_name);
    let run_files = tempfile::tempdir_in(project.root())
        .map_err(|error| format!("Could not prepare the Rust run: {error}"))?;
    let cancel = run_files.path().join("cancel");
    let cargo = cargo();
    let root = project.root();
    // Separate binaries for scripts sharing a target directory.
    let binary = format!(
        "codingpad-{:x}",
        Sha256::digest(root.as_os_str().as_encoded_bytes())
    );
    let prepare = async {
        let lease = builds.acquire().await?;
        build_cache::stage(
            project,
            &source.generated,
            &manifest,
            &builds.stage_runtime().await?,
            &binary,
        )?;
        if manifest.needs_packages
            && !prepare_dependencies(&cargo, root, &manifest, &mut events).await?
        {
            return Ok(None);
        }
        let executable = compile(&cargo, root, lease.target(), &source, name, &mut events).await?;
        // The shared target may be cleared while this script runs. A private
        // copy also avoids trying to replace an in-use executable on Windows.
        let Some(executable) = executable else {
            return Ok(None);
        };
        stage_executable(&executable, run_files.path()).map(Some)
    };
    let build = tokio::select! {
        biased;
        _ = stop.changed() => {
            events.send("run_finished", json!({"ok": false}));
            return Ok((stopped(), events.dumps));
        },
        build = prepare => build?,
    };
    let ok = if let Some(executable) = build {
        let exit = execute(
            &executable,
            script,
            run_files.path(),
            &cancel,
            values,
            stop,
            &mut events,
        )
        .await?;
        let ScriptExit::Completed(status) = exit else {
            let mut cache_cleared = false;
            if matches!(exit, ScriptExit::Killed) {
                match retire_cache(Some(values)) {
                    Ok(retired) => {
                        cache_cleared = retired.is_some();
                        if let Err(error) = remove_retired_cache(retired).await {
                            events.stream("stderr", &format!("{error}\n"));
                        }
                    }
                    Err(error) => events.stream("stderr", &format!("{error}\n")),
                }
            }
            events.send("run_finished", json!({"ok": false}));
            return Ok((
                RunResult {
                    cache_cleared,
                    ..stopped()
                },
                events.dumps,
            ));
        };
        let panic = events.panic.take();
        if let Some(panic) = panic.as_ref() {
            if status.success() || !events.main_unwound {
                events.stream("stderr", &panic.stderr());
            }
        }
        if !status.success() {
            if let Some(panic) = panic.filter(|_| events.main_unwound) {
                events.error("Rust panic", &panic.message, panic.frames(&source, name));
            } else {
                events.error(
                    "Rust script failed",
                    &format!("The script exited with {status}."),
                    Vec::new(),
                );
            }
        }
        status.success()
    } else {
        false
    };
    if ok {
        builds.schedule_warmup();
    }
    Ok((events.finish(ok), events.dumps))
}

fn stage_executable(executable: &Path, destination: &Path) -> Result<PathBuf, String> {
    let name = executable
        .file_name()
        .ok_or("Cargo returned an invalid executable path")?;
    let copy = destination.join(name);
    std::fs::copy(executable, &copy)
        .map_err(|error| format!("Could not stage the Rust executable: {error}"))?;
    #[cfg(windows)]
    {
        // MSVC stores source locations in a companion PDB. Keep it beside the
        // private executable so Windows can symbolize the panic stack.
        match std::fs::copy(executable.with_extension("pdb"), copy.with_extension("pdb")) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Could not stage Rust debug symbols: {error}")),
        }
    }
    Ok(copy)
}

async fn compile(
    cargo: &Path,
    root: &Path,
    target: &Path,
    source: &Source<'_>,
    name: &str,
    events: &mut Events<'_, impl FnMut(KernelEvent)>,
) -> Result<Option<PathBuf>, String> {
    let CargoOutput {
        status,
        stdout,
        stderr,
    } = build_cargo(cargo, root, target).await?;

    let root_source = root.join(source::FILE);
    let canonical_source = std::fs::canonicalize(&root_source).ok();
    let mut executable = None;
    let mut diagnostics = Vec::new();
    let mut had_error = false;
    for line in stdout.split(|&byte| byte == b'\n') {
        let Ok(message) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let is_script = message["target"]["src_path"]
            .as_str()
            .map(Path::new)
            .is_some_and(|path| {
                path == root_source
                    || canonical_source.as_ref().is_some_and(|source| {
                        path.ends_with(source::FILE)
                            && std::fs::canonicalize(path).is_ok_and(|path| path == *source)
                    })
            });
        if message["reason"] == "compiler-artifact" && is_script {
            if let Some(path) = message["executable"].as_str() {
                executable = Some(root.join(path));
            }
        } else if message["reason"] == "compiler-message" {
            let diagnostic = &message["message"];
            if diagnostic["level"] == "error" {
                had_error = true;
                let text = diagnostic["message"]
                    .as_str()
                    .unwrap_or("Compilation failed");
                let text = diagnostic["code"]["code"]
                    .as_str()
                    .map_or_else(|| text.to_owned(), |code| format!("{code}: {text}"));
                let span = diagnostic["spans"].as_array().and_then(|spans| {
                    spans
                        .iter()
                        .filter(|span| span["is_primary"] == true)
                        .find_map(|span| {
                            is_script.then(|| diagnostic_range(span, source)).flatten()
                        })
                });
                let frames = if let Some((line, column, end_line, end_column)) = span {
                    diagnostics.push(json!({
                        "message": text, "line": line, "column": column,
                        "endLine": end_line, "endColumn": end_column
                    }));
                    vec![
                        json!({"file": name, "line": line, "column": column, "function": "<compile>", "text": "", "script": true}),
                    ]
                } else {
                    diagnostic["spans"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|span| span["is_primary"] == true)
                        .map(|span| {
                            json!({
                                "file": span["file_name"], "line": span["line_start"],
                                "column": span["column_start"], "function": "<compile>",
                                "text": "", "script": false
                            })
                        })
                        .collect()
                };
                events.error("Compile error", &text, frames);
            }
        }
    }
    events.send("diagnostics", json!({"diagnostics": diagnostics}));
    if status.success() {
        executable
            .map(Some)
            .ok_or_else(|| "Cargo did not produce a runnable Rust executable.".into())
    } else {
        let stderr = String::from_utf8_lossy(&stderr);
        if stderr.contains("no default is configured")
            || stderr.contains("could not execute process `rustc")
        {
            return Err(format!("{MISSING}\n{}", stderr.trim()));
        }
        if !stderr.is_empty() {
            events.stream("stderr", &stderr);
        }
        if !had_error {
            events.error(
                "Build error",
                match stderr.trim() {
                    "" => "Cargo could not build the Rust script.",
                    text => text.strip_prefix("error: ").unwrap_or(text),
                },
                Vec::new(),
            );
        }
        Ok(None)
    }
}

async fn build_cargo(cargo: &Path, root: &Path, target: &Path) -> Result<CargoOutput, String> {
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .args([
            "build",
            "--offline",
            "--message-format=json",
            "--quiet",
            "--target-dir",
        ])
        .arg(target)
        .env("CARGO_BUILD_BUILD_DIR", target);
    cargo_output(command).await
}

async fn prepare_dependencies(
    cargo: &Path,
    root: &Path,
    manifest: &Manifest,
    events: &mut Events<'_, impl FnMut(KernelEvent)>,
) -> Result<bool, String> {
    let fetch = |offline| {
        let mut command = Command::new(cargo);
        command.current_dir(root).args(["fetch", "--quiet"]);
        if offline {
            command.arg("--offline");
        }
        cargo_output(command)
    };
    let offline = fetch(true).await?;
    if offline.status.success() {
        return Ok(true);
    }

    let online = fetch(false).await?;
    if !online.status.success() {
        let offline_error = String::from_utf8_lossy(&offline.stderr);
        let online_error = String::from_utf8_lossy(&online.stderr);
        let mut message = "Could not resolve Rust dependencies.".to_owned();
        let offline_error = offline_error.trim();
        let online_error = online_error.trim();
        if !offline_error.is_empty() {
            message.push_str(&format!("\nCached packages:\n{offline_error}"));
        }
        if !online_error.is_empty() && online_error != offline_error {
            message.push_str(&format!("\nCargo fetch:\n{online_error}"));
        }
        let line = manifest.dependency_error_line(offline_error);
        events.send(
            "diagnostics",
            json!({"diagnostics": [
                {"message": message, "line": line, "column": 1, "endLine": line, "endColumn": 2}
            ]}),
        );
        events.error("Build error", &message, Vec::new());
    }
    Ok(online.status.success())
}

struct CargoOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn cargo_output(mut command: Command) -> Result<CargoOutput, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = spawn(&mut command).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            MISSING.to_owned()
        } else {
            format!("Could not start Cargo: {error}")
        }
    })?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_pipe = child.process.stdout.take().unwrap();
    let mut stderr_pipe = child.process.stderr.take().unwrap();
    let (status, _, _) = tokio::try_join!(
        child.process.wait(),
        stdout_pipe.read_to_end(&mut stdout),
        stderr_pipe.read_to_end(&mut stderr),
    )
    .map_err(|error| format!("Could not read Cargo output: {error}"))?;

    child.finished = true;
    Ok(CargoOutput {
        status,
        stdout,
        stderr,
    })
}

fn diagnostic_range(span: &Value, source: &Source<'_>) -> Option<(usize, usize, usize, usize)> {
    if span["file_name"].as_str()?.replace('\\', "/") == source::FILE {
        let start = span["byte_start"].as_u64()?;
        let end = span["byte_end"].as_u64()?;
        if let Some((line, column)) = source.position(start as usize) {
            let (end_line, end_column) = source.position(end as usize).unwrap_or((line, column));
            return Some((line, column, end_line, end_column));
        }
    }
    let caller = span.get("expansion")?.get("span")?;
    diagnostic_range(caller, source)
}

// Keep the process group or job even after the direct child has been reaped.
struct RunningChild {
    process: Child,
    finished: bool,
    pid: u32,
    #[cfg(windows)]
    job: Option<std::os::windows::io::OwnedHandle>,
}

impl RunningChild {
    fn kill_tree(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            if unsafe { libc::kill(-(self.pid as i32), libc::SIGKILL) } == -1 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
        }
        #[cfg(windows)]
        {
            if let Some(job) = &self.job {
                use std::os::windows::io::AsRawHandle;
                use windows_sys::Win32::System::JobObjects::TerminateJobObject;
                if unsafe { TerminateJobObject(job.as_raw_handle(), 1) } == 0 {
                    return Err(io::Error::last_os_error());
                }
            } else if self.process.id().is_some() {
                let kill = || terminate_windows_tree(self.pid);
                if tokio::runtime::Handle::try_current().is_ok_and(|handle| {
                    handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
                }) {
                    tokio::task::block_in_place(kill)?;
                } else {
                    kill()?;
                }
            }
        }
        self.finished = true;
        Ok(())
    }
}

impl Drop for RunningChild {
    fn drop(&mut self) {
        if !self.finished && self.kill_tree().is_err() {
            let _ = self.process.start_kill();
        }
    }
}

#[cfg(windows)]
fn terminate_windows_tree(pid: u32) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    let status = std::process::Command::new("taskkill")
        .creation_flags(0x08000000)
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("taskkill exited with {status}")))
    }
}

#[cfg(windows)]
fn assign_job(process: &Child) -> Option<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    let child = process.raw_handle()?;
    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
        return None;
    }
    let job = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let configured = unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    };
    if configured == 0 || unsafe { AssignProcessToJobObject(job.as_raw_handle(), child) } == 0 {
        return None;
    }
    Some(job)
}

// Callers choose whether to pipe output for Results or discard it for warmup.
fn spawn(command: &mut Command) -> io::Result<RunningChild> {
    command.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x08000000 | 0x00000200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
    let process = command.spawn()?;
    let pid = process
        .id()
        .ok_or_else(|| io::Error::other("Could not get the child process ID"))?;
    Ok(RunningChild {
        pid,
        #[cfg(windows)]
        job: assign_job(&process),
        process,
        finished: false,
    })
}

fn token() -> Result<([u8; 32], OsString), String> {
    let mut random = [0; 16];
    getrandom::fill(&mut random)
        .map_err(|error| format!("Could not initialize the Rust output channel: {error}"))?;
    let text: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut token = [0; 32];
    token.copy_from_slice(text.as_bytes());
    Ok((token, text.into()))
}

enum ScriptExit {
    Completed(ExitStatus),
    Cancelled,
    Killed,
}

async fn execute(
    executable: &Path,
    script: Option<&Path>,
    root: &Path,
    cancel: &Path,
    cache: &Path,
    stop: &mut watch::Receiver<u64>,
    events: &mut Events<'_, impl FnMut(KernelEvent)>,
) -> Result<ScriptExit, String> {
    let (token, secret) = token()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|error| error.to_string())?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    let mut command = Command::new(executable);
    let cwd = script
        .and_then(|path| std::path::absolute(path).ok())
        .and_then(|path| path.parent().map(Path::to_path_buf));
    command
        .current_dir(cwd.as_deref().unwrap_or(root))
        .env("CODINGPAD_RUST_CANCEL", cancel)
        .env("CODINGPAD_RUST_CACHE", cache)
        .env("CODINGPAD_RUST_OUTPUT", address.to_string())
        .env("CODINGPAD_RUST_TOKEN", secret)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    let ready = {
        // Let the generated entry point wait until the job is attached before
        // it can start a subprocess.
        let ready = cancel.with_file_name("start");
        command.env("CODINGPAD_RUST_START", &ready);
        ready
    };
    let mut child = spawn_script(&mut command)
        .await
        .map_err(|error| format!("Could not start the Rust script: {error}"))?;
    #[cfg(windows)]
    std::fs::write(ready, [])
        .map_err(|error| format!("Could not start the Rust script: {error}"))?;
    read_output(&mut child, &listener, &token, events, stop, cancel)
        .await
        .map_err(|error| format!("Could not run the Rust script: {error}"))
}

async fn spawn_script(command: &mut Command) -> io::Result<RunningChild> {
    #[cfg(unix)]
    for _ in 0..5 {
        match spawn(command) {
            // A concurrent fork can briefly inherit the open file description
            // from the executable copy before closing it on exec.
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            result => return result,
        }
    }
    spawn(command)
}

async fn read_output(
    child: &mut RunningChild,
    listener: &TcpListener,
    token: &[u8; 32],
    events: &mut Events<'_, impl FnMut(KernelEvent)>,
    stop: &mut watch::Receiver<u64>,
    cancel: &Path,
) -> io::Result<ScriptExit> {
    let mut stdout = BufReader::new(child.process.stdout.take().unwrap());
    let mut stderr = BufReader::new(child.process.stderr.take().unwrap());
    let mut runtime: Option<Lines<BufReader<TcpStream>>> = None;
    let mut connected = false;
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut stdout_line = Vec::new();
    let mut stderr_line = Vec::new();
    let mut exit = None;
    let mut deadline = Instant::now();
    let mut stopping = false;
    let connect = crate::accept_kernel(listener, token);
    tokio::pin!(connect);
    loop {
        tokio::select! {
            biased;
            // Observe exit even if subprocesses inherit a pipe and keep writing.
            result = child.process.wait(), if exit.is_none() => {
                exit = Some(result?);
                if stopping {
                    child.kill_tree()?;
                }
                deadline = Instant::now() + Duration::from_millis(500);
            }
            _ = stop.changed(), if exit.is_none() && !stopping => {
                std::fs::write(cancel, [])?;
                stopping = true;
                deadline = Instant::now() + STOP_GRACE;
            }
            _ = tokio::time::sleep_until(deadline), if exit.is_none() && stopping => {
                child.kill_tree()?;
                child.process.wait().await?;
                return Ok(ScriptExit::Killed);
            }
            _ = tokio::time::sleep_until(deadline), if exit.is_some() => break,
            socket = &mut connect, if !connected => {
                let socket = socket?;
                socket.set_nodelay(true)?;
                runtime = Some(BufReader::new(socket).lines());
                connected = true;
            }
            count = stdout.read_until(b'\n', &mut stdout_line), if stdout_open => {
                stdout_open = count? != 0;
                if stdout_open { events.stream("stdout", &String::from_utf8_lossy(&stdout_line)); }
                stdout_line.clear();
            }
            count = stderr.read_until(b'\n', &mut stderr_line), if stderr_open => {
                stderr_open = count? != 0;
                if stderr_open { events.stream("stderr", &String::from_utf8_lossy(&stderr_line)); }
                stderr_line.clear();
            }
            line = async { runtime.as_mut().unwrap().next_line().await }, if runtime.is_some() => {
                match line {
                    Ok(Some(line)) => {
                        events.runtime(&line)?;
                        // The child may exit as soon as dump() returns. Acknowledge
                        // the complete event before allowing that to happen.
                        let channel = runtime.as_mut().unwrap().get_mut();
                        if let Err(error) = channel.write_all(b"1").await {
                            if !peer_closed(&error) { return Err(error); }
                            runtime = None;
                        }
                    }
                    Ok(None) => runtime = None,
                    Err(error) if peer_closed(&error) => runtime = None,
                    Err(error) => return Err(error),
                }
            }
        }
        if exit.is_some() && !stdout_open && !stderr_open && runtime.is_none() {
            break;
        }
    }
    child.finished = true;
    Ok(if stopping && cancel.with_extension("observed").is_file() {
        ScriptExit::Cancelled
    } else {
        ScriptExit::Completed(exit.unwrap())
    })
}

fn peer_closed(error: &io::Error) -> bool {
    // Windows can reset a socket held by a program that has just exited.
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
    )
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RuntimeEvent {
    Debug(DebugValue),
    LiveUpdate {
        key: String,
        revision: String,
        #[serde(flatten)]
        value: DebugValue,
    },
    Panic(Panic),
    MainUnwound,
}

#[derive(Deserialize)]
struct DebugValue {
    #[serde(rename = "type")]
    name: String,
    text: String,
    truncated: String,
}

impl DebugValue {
    fn snapshot(self) -> (Snapshot, usize) {
        let size = self.text.len();
        let snapshot = Snapshot::new(self.text, self.name, self.truncated == "true");
        let bytes = snapshot.retained_bytes(size);
        (snapshot, bytes)
    }
}

struct LiveView {
    node: String,
    current: LiveSnapshot,
}

#[derive(Clone, Copy)]
struct LiveSnapshot {
    key: u64,
    revision: u64,
    group: u64,
    index: debug::Index,
}

struct Events<'a, F> {
    emit: &'a mut F,
    dumps: Dumps,
    views: VecDeque<LiveView>,
    latest: VecDeque<LiveSnapshot>,
    panic: Option<Panic>,
    main_unwound: bool,
}

impl<F: FnMut(KernelEvent)> Events<'_, F> {
    fn finish(&mut self, ok: bool) -> RunResult {
        self.send("run_finished", json!({"ok": ok}));
        RunResult {
            ok,
            stopped: false,
            cache_cleared: false,
        }
    }

    fn send(&mut self, method: &str, mut params: Value) {
        params["runId"] = json!(self.dumps.run_id());
        (self.emit)(KernelEvent {
            method: method.into(),
            params,
        });
    }

    fn stream(&mut self, stream: &str, text: &str) {
        self.send("stream", json!({"stream": stream, "text": text}));
    }

    fn runtime(&mut self, line: &str) -> io::Result<()> {
        match serde_json::from_str(line)? {
            RuntimeEvent::Debug(value) => self.dump(value),
            RuntimeEvent::LiveUpdate {
                key,
                revision,
                value,
            } => {
                if let (Ok(key), Ok(revision)) = (key.parse(), revision.parse()) {
                    self.update(key, revision, value);
                }
            }
            RuntimeEvent::Panic(panic) => {
                if let Some(recovered) = self.panic.replace(panic) {
                    self.stream("stderr", &recovered.stderr());
                }
            }
            RuntimeEvent::MainUnwound => self.main_unwound = true,
        }
        Ok(())
    }

    fn dump(&mut self, value: DebugValue) {
        let (snapshot, bytes) = value.snapshot();
        let root = snapshot.root;
        let live = match snapshot.nodes[root].value {
            ValueNode::Live {
                key,
                revision,
                value,
            } => Some((key, revision, value)),
            _ => None,
        };
        let group = self.dumps.insert(snapshot, bytes);
        let node = if let Some((key, revision, index)) = live {
            let id = self.dumps.id();
            let mut current = LiveSnapshot {
                key,
                revision,
                index,
                group,
            };
            // The initial Debug snapshot can arrive after a concurrent set().
            // Render the newer value when its update already reached Results.
            let newer = self
                .latest
                .iter()
                .find(|entry| entry.key == key && entry.revision > revision);
            let updated = newer.and_then(|entry| {
                let node = self
                    .dumps
                    .current(entry.group, entry.index, Some(id.clone()))?;
                current = *entry;
                self.dumps.remove(group);
                Some(node)
            });
            let child =
                updated.unwrap_or_else(|| self.dumps.initial(group, index, Some(id.clone())));
            if self.views.len() == 500 {
                self.views.pop_front();
            }
            self.views.push_back(LiveView { node: id, current });
            json!({"id": self.dumps.id(), "kind": "live", "node": child})
        } else {
            self.dumps.initial(group, root, None)
        };
        self.send("dump", json!({"node": node}));
    }

    fn update(&mut self, key: u64, revision: u64, value: DebugValue) {
        if let Some(index) = self.latest.iter().position(|entry| entry.key == key) {
            if self.latest[index].revision >= revision {
                return;
            }
            let old = self.latest.remove(index).unwrap();
            self.dumps.remove(old.group);
        }
        let (snapshot, bytes) = value.snapshot();
        let index = snapshot.root;
        let group = self.dumps.insert(snapshot, bytes);
        let update = LiveSnapshot {
            key,
            revision,
            group,
            index,
        };
        if self.latest.len() == 500 {
            self.latest.pop_front();
        }
        self.latest.push_back(update);

        for position in 0..self.views.len() {
            let view = &mut self.views[position];
            if view.current.key != key || view.current.revision >= revision {
                continue;
            }
            self.dumps.remove(view.current.group);
            view.current = update;
            let target = view.node.clone();
            // The target stays stable, while the replacement gets a fresh
            // identity so collection and table viewers reset their paging.
            let node = self.dumps.initial(group, index, None);
            self.send("dump_update", json!({"nodeId": target, "node": node}));
        }
    }

    fn error(&mut self, kind: &str, message: &str, frames: Vec<Value>) {
        let id = self.dumps.id();
        self.send("dump", json!({"node": {
            "id": id, "kind": "error", "type": kind, "message": message, "traceback": frames, "causes": []
        }}));
    }
}
