//! Local M1 performance checks (D38, D48). Run with `just bench`.

use codingpad_kernel_core::{lsp::LanguageServers, KernelEvent, Supervisor};
use serde_json::json;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::{
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TARGET_APP_READY: Duration = Duration::from_millis(1_000);
const TARGET_FIRST_KERNEL: Duration = Duration::from_millis(3_000);
const TARGET_WARM_RERUN: Duration = Duration::from_millis(200);
const TARGET_DUMP_FIRST_PAGE: Duration = Duration::from_millis(1_000);
const TARGET_COMPLETION: Duration = Duration::from_millis(300);
const TARGET_IDLE_MEMORY: u64 = 300 * 1024 * 1024;

#[cfg(unix)]
const APP_READY_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(unix)]
const IDLE_SETTLE: Duration = Duration::from_secs(5);

const DUMP_SCRIPT: &str = r#"rows = [{"name": f"row {index}", "score": index} for index in range(100_000)]
print("__codingpad_bench_ready__", flush=True)
dump(rows)
"#;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Pass,
    Fail,
    Skip,
}

struct Row {
    metric: &'static str,
    value: String,
    target: String,
    status: Status,
}

impl Row {
    fn time(metric: &'static str, measured: Duration, target: Duration) -> Self {
        Self {
            metric,
            value: format!("{:.1} ms", measured.as_secs_f64() * 1000.0),
            target: format!("< {} ms", target.as_millis()),
            status: if measured <= target {
                Status::Pass
            } else {
                Status::Fail
            },
        }
    }

    fn memory(metric: &'static str, measured: u64, target: u64) -> Self {
        Self {
            metric,
            value: format!("{:.1} MB", measured as f64 / (1024.0 * 1024.0)),
            target: format!("< {} MB", target / (1024 * 1024)),
            status: if measured <= target {
                Status::Pass
            } else {
                Status::Fail
            },
        }
    }

    fn failed(metric: &'static str, target: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            metric,
            value: reason.into(),
            target: target.into(),
            status: Status::Fail,
        }
    }

    fn skipped(metric: &'static str, target: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            metric,
            value: reason.into(),
            target: target.into(),
            status: Status::Skip,
        }
    }
}

struct AppMeasurement {
    ready: Duration,
    idle_memory: u64,
}

#[tokio::main]
async fn main() {
    let mut rows = Vec::new();
    let mut failures = Vec::new();

    match app_measurement() {
        Ok(Some(measurement)) => {
            rows.push(Row::time("app usable", measurement.ready, TARGET_APP_READY));
            rows.push(Row::memory(
                "idle memory",
                measurement.idle_memory,
                TARGET_IDLE_MEMORY,
            ));
        }
        Ok(None) => {
            rows.push(Row::skipped(
                "app usable",
                "< 1000 ms",
                "not measured on this platform",
            ));
            rows.push(Row::skipped(
                "idle memory",
                "< 300 MB",
                "not measured on this platform",
            ));
        }
        Err(error) => {
            rows.push(Row::failed("app usable", "< 1000 ms", error.clone()));
            rows.push(Row::failed("idle memory", "< 300 MB", error));
        }
    }

    let supervisor = Supervisor::default();
    match first_run(&supervisor).await {
        Ok(measured) => rows.push(Row::time(
            "first Python kernel start",
            measured,
            TARGET_FIRST_KERNEL,
        )),
        Err(error) => {
            rows.push(Row::failed(
                "first Python kernel start",
                "< 3000 ms",
                &error,
            ));
            failures.push(error);
        }
    }

    match warm_run(&supervisor).await {
        Ok(measured) => rows.push(Row::time("warm Python rerun", measured, TARGET_WARM_RERUN)),
        Err(error) => {
            rows.push(Row::failed("warm Python rerun", "< 200 ms", &error));
            failures.push(error);
        }
    }

    match dump_first_page(&supervisor).await {
        Ok(measured) => rows.push(Row::time(
            "100k-row dump, first page to host",
            measured,
            TARGET_DUMP_FIRST_PAGE,
        )),
        Err(error) => {
            rows.push(Row::failed(
                "100k-row dump, first page to host",
                "< 1000 ms",
                &error,
            ));
            failures.push(error);
        }
    }

    match completion_latency().await {
        Ok(measured) => rows.push(Row::time("Python completion", measured, TARGET_COMPLETION)),
        Err(error) => {
            rows.push(Row::failed("Python completion", "< 300 ms", &error));
            failures.push(error);
        }
    }

    supervisor.shutdown().await;
    report(&rows);
    if failures.is_empty() && rows.iter().all(|row| row.status != Status::Fail) {
        println!("\nAll measured M1 targets pass.");
    } else {
        eprintln!("\nSome M1 targets did not pass or could not be measured.");
        std::process::exit(1);
    }
}

async fn first_run(supervisor: &Supervisor) -> Result<Duration, String> {
    let started = Instant::now();
    let result = supervisor
        .run("bench-first", "print('ready')", |_| {})
        .await?;
    if !result.ok {
        return Err("the first kernel run failed".into());
    }
    Ok(started.elapsed())
}

async fn warm_run(supervisor: &Supervisor) -> Result<Duration, String> {
    let started = Instant::now();
    let result = supervisor
        .run("bench-first", "value = 1\nvalue", |_| {})
        .await?;
    if !result.ok {
        return Err("the warm kernel run failed".into());
    }
    Ok(started.elapsed())
}

async fn dump_first_page(supervisor: &Supervisor) -> Result<Duration, String> {
    let mut ready = None;
    let mut first_page = None;
    let result = supervisor
        .run("bench-dump", DUMP_SCRIPT, |event: KernelEvent| {
            let text = event.params["text"].as_str().unwrap_or_default();
            if ready.is_none()
                && event.method == "stream"
                && text.contains("__codingpad_bench_ready__")
            {
                ready = Some(Instant::now());
            }
            if first_page.is_none()
                && event.method == "dump"
                && event.params["node"]["kind"] == "table"
            {
                first_page = ready.map(|started| started.elapsed());
            }
        })
        .await?;
    if !result.ok {
        return Err("the 100k-row dump run failed".into());
    }
    first_page.ok_or_else(|| "no table dump arrived".into())
}

fn completion_params(uri: &str, character: u32) -> serde_json::Value {
    json!({
        "textDocument": { "uri": uri },
        "position": { "line": 1, "character": character },
        "context": { "triggerKind": 1 },
    })
}

async fn completion_latency() -> Result<Duration, String> {
    eprintln!("Warming the Python language server (the first run may download basedpyright)...");
    let servers = LanguageServers::default();
    let measured = async {
        servers.begin_session("bench").await;
        let start = servers
            .start_python("bench-lsp", "import os\nos.path.join", "bench", |_| {})
            .await?;
        let _ = servers
            .request(
                "bench-lsp",
                "textDocument/completion",
                completion_params(&start.uri, 12),
            )
            .await?;
        servers
            .did_change("bench-lsp", "import os\nos.path.join(")
            .await?;
        let started = Instant::now();
        let result = servers
            .request(
                "bench-lsp",
                "textDocument/completion",
                completion_params(&start.uri, 13),
            )
            .await?;
        let elapsed = started.elapsed();
        if result.is_null() {
            return Err::<Duration, String>(
                "the language server returned no completion result".into(),
            );
        }
        Ok(elapsed)
    }
    .await;
    servers.shutdown().await;
    measured
}

fn report(rows: &[Row]) {
    let metric_width = rows
        .iter()
        .map(|row| row.metric.len())
        .max()
        .unwrap_or(0)
        .max("metric".len());
    let value_width = rows
        .iter()
        .map(|row| row.value.len())
        .max()
        .unwrap_or(0)
        .max("measured".len());
    let target_width = rows
        .iter()
        .map(|row| row.target.len())
        .max()
        .unwrap_or(0)
        .max("target".len());

    println!(
        "CodingPad M1 performance ({}, {} build)\n",
        std::env::consts::OS,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    println!(
        "{:<metric_width$}  {:<value_width$}  {:<target_width$}  status",
        "metric", "measured", "target"
    );
    for row in rows {
        let status = match row.status {
            Status::Pass => "pass",
            Status::Fail => "FAIL",
            Status::Skip => "skip",
        };
        println!(
            "{:<metric_width$}  {:<value_width$}  {:<target_width$}  {status}",
            row.metric, row.value, row.target
        );
    }
}

fn app_measurement() -> Result<Option<AppMeasurement>, String> {
    #[cfg(unix)]
    {
        measure_app_unix().map(Some)
    }
    #[cfg(not(unix))]
    {
        Ok(None)
    }
}

#[cfg(unix)]
fn measure_app_unix() -> Result<AppMeasurement, String> {
    use std::{os::unix::process::CommandExt, thread};

    let desktop = desktop_binary()?;
    let marker = std::env::temp_dir().join(format!(
        "codingpad-bench-ready-{}-{}",
        std::process::id(),
        now_nanos()
    ));
    let _ = std::fs::remove_file(&marker);
    #[cfg(target_os = "macos")]
    let baseline_helpers = webkit_pids();

    let mut command = Command::new(&desktop);
    command
        .env("CODINGPAD_BENCH_MARKER", &marker)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }

    let launched = now_nanos();
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not launch {}: {error}", desktop.display()))?;
    let pid = child.id();

    let ready = match wait_for_marker(&marker, launched, &mut child) {
        Ok(ready) => ready,
        Err(error) => {
            let _ = std::fs::remove_file(&marker);
            stop_app(&mut child, pid);
            return Err(error);
        }
    };
    thread::sleep(IDLE_SETTLE);

    #[cfg(target_os = "macos")]
    let idle_memory = {
        let current = webkit_pids();
        let helpers: Vec<_> = current.difference(&baseline_helpers).copied().collect();
        std::iter::once(pid)
            .chain(helpers)
            .map(process_memory)
            .sum()
    };
    #[cfg(not(target_os = "macos"))]
    let idle_memory = process_group_rss(pid).unwrap_or(0);
    let _ = std::fs::remove_file(&marker);
    stop_app(&mut child, pid);

    if idle_memory == 0 {
        return Err("could not read the app's memory use".into());
    }
    Ok(AppMeasurement {
        ready: Duration::from_nanos(ready),
        idle_memory,
    })
}

#[cfg(unix)]
fn stop_app(child: &mut std::process::Child, pid: u32) {
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn wait_for_marker(
    path: &Path,
    spawned: u64,
    child: &mut std::process::Child,
) -> Result<u64, String> {
    use std::thread;

    let deadline = Instant::now() + APP_READY_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Err(format!("the app exited before it was ready ({status})"));
        }
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(ready) = text.trim().parse::<u64>() {
                return Ok(ready.saturating_sub(spawned));
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err("the app did not report that it was ready".into())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn process_group_rss(pgid: u32) -> Option<u64> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,pgid=,rss="])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    Some(group_rss(pgid, &parse_process_table(&text)))
}

#[cfg(target_os = "macos")]
fn webkit_pids() -> std::collections::HashSet<u32> {
    Command::new("ps")
        .args(["-axo", "pid=,comm="])
        .output()
        .ok()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| {
                    let (pid, command) = line.trim().split_once(char::is_whitespace)?;
                    let command = command.trim();
                    let is_webkit_helper = command.starts_with(
                        "/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices/com.apple.WebKit.",
                    ) && command.contains(".xpc/");
                    is_webkit_helper.then(|| pid.parse().ok()).flatten()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Activity Monitor-style footprint, falling back to RSS.
#[cfg(target_os = "macos")]
fn process_memory(pid: u32) -> u64 {
    Command::new("footprint")
        .arg(pid.to_string())
        .output()
        .ok()
        .and_then(|output| parse_footprint(&String::from_utf8_lossy(&output.stdout)))
        .or_else(|| {
            Command::new("ps")
                .args(["-o", "rss=", "-p", &pid.to_string()])
                .output()
                .ok()
                .and_then(|output| {
                    String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .parse::<u64>()
                        .ok()
                        .map(|kilobytes| kilobytes * 1024)
                })
        })
        .unwrap_or(0)
}

#[allow(dead_code)]
fn parse_footprint(output: &str) -> Option<u64> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("phys_footprint:"))?;
    let mut fields = line["phys_footprint:".len()..].split_whitespace();
    let amount: f64 = fields.next()?.parse().ok()?;
    let unit = fields.next()?.to_ascii_lowercase();
    let factor = match unit.as_str() {
        "b" | "bytes" => 1.0,
        "kb" => 1024.0,
        "mb" => 1024.0 * 1024.0,
        "gb" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((amount * factor) as u64)
}

#[allow(dead_code)]
fn parse_process_table(output: &str) -> Vec<(u32, u32, u32, u64)> {
    output
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() != 4 {
                return None;
            }
            Some((
                fields[0].parse().ok()?,
                fields[1].parse().ok()?,
                fields[2].parse().ok()?,
                fields[3].parse().ok()?,
            ))
        })
        .collect()
}

#[allow(dead_code)]
fn group_rss(pgid: u32, table: &[(u32, u32, u32, u64)]) -> u64 {
    table
        .iter()
        .filter(|(_, _, group, _)| *group == pgid)
        .map(|(_, _, _, rss)| rss.saturating_mul(1024))
        .sum()
}

fn desktop_binary() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("CODINGPAD_DESKTOP") {
        return Ok(PathBuf::from(path));
    }
    let name = if cfg!(windows) {
        "codingpad-desktop.exe"
    } else {
        "codingpad-desktop"
    };
    let path = std::env::current_exe()
        .map_err(|error| format!("could not find the bench executable: {error}"))?
        .parent()
        .ok_or("could not find the bench folder")?
        .join(name);
    if !path.is_file() {
        return Err(format!(
            "{} is missing; run `just bench`, which builds it first",
            path.display()
        ));
    }
    Ok(path)
}

fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_sums_a_process_group() {
        let table =
            parse_process_table("10 1 10 1024\n11 10 10 2048\n12 11 12 4096\n\nbogus line\n");
        assert_eq!(table.len(), 3);
        assert_eq!(group_rss(10, &table), 3 * 1024 * 1024);
        assert_eq!(group_rss(99, &table), 0);
    }

    #[test]
    fn parses_footprint_units() {
        assert_eq!(
            parse_footprint("Auxiliary data:\n    phys_footprint: 118 MB\n"),
            Some(118 * 1024 * 1024)
        );
        assert_eq!(
            parse_footprint("    phys_footprint: 5904 KB\n"),
            Some(5904 * 1024)
        );
        assert_eq!(parse_footprint("nothing"), None);
    }

    #[test]
    fn formats_a_slow_measurement_as_failed() {
        let row = Row::time("warm", Duration::from_millis(250), TARGET_WARM_RERUN);
        assert!(row.status == Status::Fail);
        assert_eq!(row.target, "< 200 ms");
    }
}
