//! Local M1 and M2 performance checks (D38, D48, D55). Run with `just bench`.

use codingpad_kernel_core::{lsp::LanguageServers, KernelEvent, Language, Supervisor};
use serde_json::json;
use std::time::{Duration, Instant};
#[cfg(unix)]
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

const TARGET_APP_READY: Duration = Duration::from_millis(1_000);
const TARGET_FIRST_KERNEL: Duration = Duration::from_millis(3_000);
const TARGET_WARM_RERUN: Duration = Duration::from_millis(200);
const TARGET_DUMP_FIRST_PAGE: Duration = Duration::from_millis(1_000);
const TARGET_COMPLETION: Duration = Duration::from_millis(300);
const TARGET_IDLE_MEMORY: u64 = 300 * 1024 * 1024;
const TARGET_CSHARP_FIRST_KERNEL: Duration = Duration::from_millis(3_000);
const TARGET_CSHARP_RERUN: Duration = Duration::from_millis(500);
const TARGET_CSHARP_SEQUENCE: Duration = Duration::from_millis(1_000);
const TARGET_CSHARP_COMPLETION: Duration = Duration::from_millis(300);

/// Roslyn loads the shadow project in the background after it starts.
const ROSLYN_WARM_TIMEOUT: Duration = Duration::from_secs(90);

#[cfg(unix)]
const APP_READY_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(unix)]
const IDLE_SETTLE: Duration = Duration::from_secs(5);

const DUMP_SCRIPT: &str = r#"rows = [{"name": f"row {index}", "score": index} for index in range(100_000)]
print("__codingpad_bench_ready__", flush=True)
dump(rows)
"#;

const CSHARP_SCRIPT: &str = "var numbers = Enumerable.Range(1, 10).ToList();\nnumbers.Sum();";
const CSHARP_EDITED_SCRIPT: &str =
    "var numbers = Enumerable.Range(1, 11).ToList();\nnumbers.Sum();";
const CSHARP_EDITED_RESULT: i64 = 66;
const CSHARP_SEQUENCE_SCRIPT: &str = r#"IEnumerable<object> Rows()
{
    for (var index = 0; index < 100_000; index++)
        yield return new { Name = $"row {index}", Score = index };
}
Rows().Dump();
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

    csharp_kernel_rows(&mut rows).await;
    match csharp_completion_latency().await {
        Ok(measured) => rows.push(Row::time(
            "C# completion",
            measured,
            TARGET_CSHARP_COMPLETION,
        )),
        Err(error) => rows.push(Row::failed("C# completion", "< 300 ms", error)),
    }

    report(&rows);
    if failures.is_empty() && rows.iter().all(|row| row.status != Status::Fail) {
        println!("\nAll measured M1 and M2 targets pass.");
    } else {
        eprintln!("\nSome M1 or M2 targets did not pass or could not be measured.");
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

/// Measure the C# kernel in order: a cold start, the first rerun after an edit,
/// then a dump. Each later metric depends on the kernel the first one started.
async fn csharp_kernel_rows(rows: &mut Vec<Row>) {
    let supervisor = Supervisor::default();
    match csharp_first_run(&supervisor).await {
        Ok(measured) => rows.push(Row::time(
            "first C# kernel start",
            measured,
            TARGET_CSHARP_FIRST_KERNEL,
        )),
        Err(error) => rows.push(Row::failed("first C# kernel start", "< 3000 ms", error)),
    }
    match csharp_edited_rerun(&supervisor).await {
        Ok(measured) => rows.push(Row::time(
            "warm C# rerun after an edit",
            measured,
            TARGET_CSHARP_RERUN,
        )),
        Err(error) => rows.push(Row::failed(
            "warm C# rerun after an edit",
            "< 500 ms",
            error,
        )),
    }
    match csharp_sequence_first_page(&supervisor).await {
        Ok(measured) => rows.push(Row::time(
            "100k-item IEnumerable, first page to host",
            measured,
            TARGET_CSHARP_SEQUENCE,
        )),
        Err(error) => rows.push(Row::failed(
            "100k-item IEnumerable, first page to host",
            "< 1000 ms",
            error,
        )),
    }
    supervisor.shutdown().await;
}

async fn csharp_first_run(supervisor: &Supervisor) -> Result<Duration, String> {
    let started = Instant::now();
    let result = supervisor
        .run_language("bench-csharp", CSHARP_SCRIPT, Language::Csharp, |_| {})
        .await?;
    if !result.ok {
        return Err("the first C# run failed".into());
    }
    Ok(started.elapsed())
}

/// Changed source forces a new compilation, and the result proves it ran.
async fn csharp_edited_rerun(supervisor: &Supervisor) -> Result<Duration, String> {
    let mut value = None;
    let started = Instant::now();
    let result = supervisor
        .run_language(
            "bench-csharp",
            CSHARP_EDITED_SCRIPT,
            Language::Csharp,
            |event: KernelEvent| {
                if event.method == "dump" {
                    value = event.params["node"]["v"].as_i64();
                }
            },
        )
        .await?;
    let elapsed = started.elapsed();
    if !result.ok {
        return Err("the edited C# run failed".into());
    }
    if value != Some(CSHARP_EDITED_RESULT) {
        return Err("the edited C# run did not return the edited result".into());
    }
    Ok(elapsed)
}

/// Timed from pressing Run: a lazy sequence costs nothing to create, so unlike
/// the Python dump the compile is the work a user waits for.
async fn csharp_sequence_first_page(supervisor: &Supervisor) -> Result<Duration, String> {
    let mut first_page = None;
    let started = Instant::now();
    let result = supervisor
        .run_language(
            "bench-csharp",
            CSHARP_SEQUENCE_SCRIPT,
            Language::Csharp,
            |event: KernelEvent| {
                let node = &event.params["node"];
                if first_page.is_none()
                    && event.method == "dump"
                    && node["kind"] == "table"
                    && node["rows"].as_array().is_some_and(|rows| !rows.is_empty())
                {
                    first_page = Some(started.elapsed());
                }
            },
        )
        .await?;
    if !result.ok {
        return Err("the 100k-item C# dump failed".into());
    }
    first_page.ok_or_else(|| "no table dump arrived".into())
}

fn csharp_completion_params(uri: &str, line: u32, character: u32) -> serde_json::Value {
    json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character },
        "context": { "triggerKind": 2, "triggerCharacter": "." },
    })
}

fn has_completion(result: &serde_json::Value, prefix: &str) -> bool {
    result["items"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["label"]
                .as_str()
                .is_some_and(|label| label.starts_with(prefix))
        })
    })
}

async fn csharp_completion_latency() -> Result<Duration, String> {
    eprintln!("Warming the Roslyn language server...");
    let servers = LanguageServers::default();
    let measured = async {
        servers.begin_session("bench").await;
        let source = "var value = 42;\nvalue.";
        let start = servers
            .start_csharp("bench-csharp-lsp", source, None, "bench", |_| {})
            .await?;
        let warm = Instant::now();
        loop {
            let result = servers
                .request(
                    "bench-csharp-lsp",
                    "textDocument/completion",
                    csharp_completion_params(&start.uri, 1, 6),
                )
                .await?;
            if has_completion(&result, "Dump") {
                break;
            }
            if warm.elapsed() > ROSLYN_WARM_TIMEOUT {
                return Err("Roslyn did not load the shadow project in time".to_owned());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        // Time the first completion after an edit, as a user would see it.
        servers
            .did_change(
                "bench-csharp-lsp",
                "var other = 42;\nvar total = other + 1;\ntotal.",
            )
            .await?;
        let started = Instant::now();
        let result = servers
            .request(
                "bench-csharp-lsp",
                "textDocument/completion",
                csharp_completion_params(&start.uri, 2, 6),
            )
            .await?;
        let elapsed = started.elapsed();
        if !has_completion(&result, "Dump") {
            return Err("Roslyn returned no completions for the edited script".to_owned());
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
        "CodingPad M1 and M2 performance ({}, {} build)\n",
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

#[cfg(unix)]
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

#[cfg(unix)]
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

    #[test]
    fn finds_a_completion_by_label_prefix() {
        let result = json!({ "items": [{ "label": "ToString" }, { "label": "Dump<>" }] });
        assert!(has_completion(&result, "Dump"));
        assert!(!has_completion(&result, "Missing"));
        assert!(!has_completion(&serde_json::Value::Null, "Dump"));
    }
}
