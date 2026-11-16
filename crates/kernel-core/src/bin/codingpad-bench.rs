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
const TARGET_IDLE_MEMORY: u64 = 475 * 1024 * 1024;
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
            status: if measured < target {
                Status::Pass
            } else {
                Status::Fail
            },
        }
    }

    fn memory(metric: &'static str, measured: u64, target: u64) -> Self {
        Self {
            metric,
            value: format!("{:.1} MiB", measured as f64 / (1024.0 * 1024.0)),
            target: format!("< {} MiB", target / (1024 * 1024)),
            status: if measured < target {
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
    rss: u64,
}

#[tokio::main]
async fn main() {
    let runs = std::env::var("CODINGPAD_BENCH_RUNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(3);
    let mut passed = true;
    for run in 1..=runs {
        println!("\nBenchmark repetition {run}/{runs} (sequential, warm OS caches)");
        passed &= run_once().await;
    }
    if !passed {
        std::process::exit(1);
    }
}

async fn run_once() -> bool {
    let mut rows = Vec::new();
    let mut failures = Vec::new();

    match app_measurement() {
        Ok(Some(measurement)) => {
            rows.push(Row::time("app usable", measurement.ready, TARGET_APP_READY));
            eprintln!(
                "Idle summed RSS (informational): {:.1} MiB",
                measurement.rss as f64 / 1048576.0
            );
            rows.push(Row::memory(
                "idle physical footprint",
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
                "idle physical footprint",
                "< 475 MiB",
                "not measured on this platform",
            ));
        }
        Err(error) => {
            rows.push(Row::failed("app usable", "< 1000 ms", error.clone()));
            rows.push(Row::failed("idle physical footprint", "< 475 MiB", error));
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
        true
    } else {
        eprintln!("\nSome M1 or M2 targets did not pass or could not be measured.");
        false
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

fn useful_completion(result: &serde_json::Value, expected: &str) -> Result<usize, String> {
    let items = result
        .as_array()
        .or_else(|| result["items"].as_array())
        .ok_or("completion result has no items array")?;
    if !items.iter().any(|item| {
        item["label"].as_str() == Some(expected)
            || item["label"]["label"].as_str() == Some(expected)
    }) {
        return Err(format!(
            "completion did not include {expected:?} ({} items)",
            items.len()
        ));
    }
    Ok(items.len())
}

async fn completion_latency() -> Result<Duration, String> {
    eprintln!("Warming the Python language server (the first run may download basedpyright)...");
    let servers = LanguageServers::default();
    let measured = async {
        servers.begin_session("bench").await;
        // Warm the same imported module and member list that the user will edit.
        let start = servers.start_python("bench-lsp", "import os\nos.path.", "bench", |_| {}).await?;
        let warm = servers.request("bench-lsp", "textDocument/completion", completion_params(&start.uri, 8)).await?;
        useful_completion(&warm, "join")?;
        // Typing 'j' requests real member completion, not call-argument completion.
        // Include didChange/bridge queueing; do not wait for diagnostics or sleep.
        let started = Instant::now();
        servers.did_change("bench-lsp", "import os\nos.path.j").await?;
        let changed = started.elapsed();
        let result = servers.request("bench-lsp", "textDocument/completion", completion_params(&start.uri, 9)).await?;
        let elapsed = started.elapsed();
        let count = useful_completion(&result, "join")?;
        eprintln!("Python completion: change queued {:.3} ms, request/response {:.3} ms, {count} items including join",
            changed.as_secs_f64() * 1000.0, (elapsed - changed).as_secs_f64() * 1000.0);
        Ok(elapsed)
    }.await;
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
    eprintln!("Desktop binary: {}", desktop.display());
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
            report_startup_trace(&marker, launched);
            return Err(error);
        }
    };
    thread::sleep(IDLE_SETTLE);

    let memory = measure_memory(pid, {
        #[cfg(target_os = "macos")]
        {
            webkit_pids()
                .difference(&baseline_helpers)
                .copied()
                .collect()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Vec::new()
        }
    });
    report_startup_trace(&marker, launched);
    let _ = std::fs::remove_file(&marker);
    stop_app(&mut child, pid);
    let (rss, footprint) = memory?;
    Ok(AppMeasurement {
        ready: Duration::from_nanos(ready),
        idle_memory: footprint,
        rss,
    })
}

#[cfg(unix)]
fn report_startup_trace(marker: &Path, launched: u64) {
    let path = format!("{}.trace", marker.display());
    if let Ok(trace) = std::fs::read_to_string(&path) {
        eprintln!("Startup phases (launch unix_ns={launched}):\n{trace}");
    }
    let _ = std::fs::remove_file(path);
}

#[cfg(unix)]
fn stop_app(child: &mut std::process::Child, pid: u32) {
    // LSPs use separate process groups; killing only the app group leaks them
    // into subsequent repetitions and invalidates both memory and timing.
    if let Ok(output) = Command::new("ps")
        .args(["-axo", "pid=,ppid=,pgid=,rss="])
        .output()
    {
        let table = parse_process_table(&String::from_utf8_lossy(&output.stdout));
        for descendant in app_processes(pid, &table) {
            if descendant != pid {
                unsafe {
                    libc::kill(descendant as i32, libc::SIGKILL);
                }
            }
        }
    }
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

/// Include descendants even if a language server creates its own process group.
#[cfg(any(unix, test))]
fn app_processes(pid: u32, table: &[(u32, u32, u32, u64)]) -> std::collections::HashSet<u32> {
    let mut pids = std::collections::HashSet::from([pid]);
    loop {
        let before = pids.len();
        for (child, parent, group, _) in table {
            if *group == pid || pids.contains(parent) {
                pids.insert(*child);
            }
        }
        if pids.len() == before {
            return pids;
        }
    }
}

#[cfg(unix)]
fn measure_memory(pid: u32, helpers: Vec<u32>) -> Result<(u64, u64), String> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,pgid=,rss="])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("could not enumerate app processes".into());
    }
    let table = parse_process_table(&String::from_utf8_lossy(&output.stdout));
    let mut pids = app_processes(pid, &table);
    pids.extend(helpers);
    let mut rss = 0;
    let mut physical = 0;
    let mut pids: Vec<_> = pids.into_iter().collect();
    pids.sort_unstable();
    for pid in pids {
        let resident = table
            .iter()
            .find(|row| row.0 == pid)
            .ok_or_else(|| format!("process {pid} disappeared during memory sampling"))?
            .3
            * 1024;
        #[cfg(target_os = "linux")]
        let (footprint, private) = {
            let smaps = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
                .map_err(|e| format!("cannot read PSS for process {pid}: {e}"))?;
            parse_smaps(&smaps)
                .ok_or_else(|| format!("missing PSS/private fields for process {pid}"))?
        };
        #[cfg(target_os = "macos")]
        let (footprint, private) = (process_memory(pid)?, 0);
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        return Err("physical footprint measurement is unsupported on this platform".into());
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            #[cfg(target_os = "linux")]
            let name = std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .unwrap_or_else(|_| "unknown".into())
                .trim()
                .to_owned();
            #[cfg(not(target_os = "linux"))]
            let name = "process";
            eprintln!(
                "Memory pid={pid} ({name}): RSS={:.1} MiB physical={:.1} MiB private={} MiB",
                resident as f64 / 1048576.0,
                footprint as f64 / 1048576.0,
                if cfg!(target_os = "linux") {
                    format!("{:.1}", private as f64 / 1048576.0)
                } else {
                    "unavailable".into()
                }
            );
            rss += resident;
            physical += footprint;
        }
    }
    if rss == 0 || physical == 0 {
        return Err("could not read the app's memory use".into());
    }
    Ok((rss, physical))
}

#[cfg(any(target_os = "linux", test))]
fn parse_smaps(text: &str) -> Option<(u64, u64)> {
    let field = |key: &str| -> Option<u64> {
        text.lines().find_map(|line| {
            let value = line.strip_prefix(key)?;
            value
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()?
                .checked_mul(1024)
        })
    };
    Some((
        field("Pss:")?,
        field("Private_Clean:")?
            + field("Private_Dirty:")?
            + field("Private_Hugetlb:").unwrap_or(0),
    ))
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

/// Activity Monitor-style footprint. Never silently substitute RSS for it.
#[cfg(target_os = "macos")]
fn process_memory(pid: u32) -> Result<u64, String> {
    let output = Command::new("footprint")
        .arg(pid.to_string())
        .output()
        .map_err(|e| format!("could not run footprint for {pid}: {e}"))?;
    if output.status.success() {
        if let Some(bytes) = parse_footprint(&String::from_utf8_lossy(&output.stdout)) {
            return Ok(bytes);
        }
    }
    Err(format!(
        "could not measure phys_footprint for {pid}; RSS is not a substitute"
    ))
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
        "kb" | "kib" => 1024.0,
        "mb" | "mib" => 1024.0 * 1024.0,
        "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
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
    fn includes_language_servers_in_separate_groups() {
        let table =
            parse_process_table("10 1 10 1024\n11 10 11 2048\n12 11 11 4096\n99 1 99 4096\n");
        assert_eq!(
            app_processes(10, &table),
            std::collections::HashSet::from([10, 11, 12])
        );
    }

    #[test]
    fn requires_physical_and_private_memory_fields() {
        assert_eq!(
            parse_smaps(
                "Pss: 42 kB\nPrivate_Clean: 2 kB\nPrivate_Dirty: 3 kB\nPrivate_Hugetlb: 4 kB\n"
            ),
            Some((42 * 1024, 9 * 1024))
        );
        assert_eq!(parse_smaps("Rss: 100 kB\n"), None);
    }

    #[test]
    fn completion_must_contain_the_expected_member() {
        assert!(useful_completion(&json!(null), "join").is_err());
        assert!(useful_completion(&json!({"items": []}), "join").is_err());
        assert!(useful_completion(&json!([{"label": "other"}]), "join").is_err());
        assert_eq!(
            useful_completion(&json!({"items": [{"label": "join"}]}), "join").unwrap(),
            1
        );
        assert_eq!(
            useful_completion(&json!([{"label": {"label": "join"}}]), "join").unwrap(),
            1
        );
    }

    #[test]
    fn parses_process_table() {
        let table =
            parse_process_table("10 1 10 1024\n11 10 10 2048\n12 11 12 4096\n\nbogus line\n");
        assert_eq!(table.len(), 3);
        assert_eq!(table[1], (11, 10, 10, 2048));
    }

    #[test]
    fn parses_footprint_units() {
        assert_eq!(
            parse_footprint("Auxiliary data:\n    phys_footprint: 118 MiB\n"),
            Some(118 * 1024 * 1024)
        );
        assert_eq!(
            parse_footprint("    phys_footprint: 5904 KB\n"),
            Some(5904 * 1024)
        );
        assert_eq!(parse_footprint("nothing"), None);
    }

    #[test]
    fn target_boundaries_are_strict() {
        assert!(Row::time("ready", TARGET_APP_READY, TARGET_APP_READY).status == Status::Fail);
        assert!(Row::memory("idle", TARGET_IDLE_MEMORY, TARGET_IDLE_MEMORY).status == Status::Fail);
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
