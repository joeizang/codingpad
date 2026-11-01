use codingpad_kernel_core::{KernelEvent, Supervisor};
use serde_json::Value;
use std::time::Duration;
use tokio::time::timeout;

async fn run(supervisor: &Supervisor, id: &str, source: &str) -> Vec<KernelEvent> {
    let mut events = Vec::new();
    let result = timeout(
        Duration::from_secs(15),
        supervisor.run(id, source, |e| events.push(e)),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(result.ok);
    assert_eq!(events.last().unwrap().method, "run_finished");
    events
}

fn value(events: &[KernelEvent]) -> &Value {
    &events.iter().find(|e| e.method == "dump").unwrap().params["node"]["v"]
}

#[tokio::test]
async fn acceptance_and_streaming() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "one",
        "print('hello')\ndump({'name': 'Ada'})\n6 * 7",
    )
    .await;
    assert_eq!(events.iter().filter(|e| e.method == "dump").count(), 2);
    assert!(events.iter().any(|e| e.params["text"]
        .as_str()
        .is_some_and(|text| text.contains("hello"))));
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut tx = Some(tx);
    let handle = tokio::spawn(async move {
        supervisor
            .run(
                "one",
                "import time\nprint('before sleep')\ntime.sleep(1)",
                |e| {
                    if e.params["text"]
                        .as_str()
                        .is_some_and(|text| text.contains("before sleep"))
                    {
                        let _ = tx.take().unwrap().send(());
                    }
                },
            )
            .await
            .unwrap()
    });
    timeout(Duration::from_secs(3), rx).await.unwrap().unwrap();
    assert!(
        !handle.is_finished(),
        "Output must arrive before execution finishes"
    );
    assert!(handle.await.unwrap().ok);
}

#[tokio::test]
async fn warm_per_script_clean_globals_and_close() {
    let supervisor = Supervisor::default();
    let first = run(&supervisor, "one", "import os\nx = 7\nos.getpid()").await;
    let second = run(
        &supervisor,
        "one",
        "import os\nassert 'x' not in globals()\nos.getpid()",
    )
    .await;
    let other = run(&supervisor, "two", "import os\nos.getpid()").await;
    assert_eq!(value(&first), value(&second));
    assert_ne!(value(&first), value(&other));
    supervisor.close("one").await;
    assert_ne!(
        value(&first),
        value(&run(&supervisor, "one", "import os\nos.getpid()").await)
    );
}

#[tokio::test]
async fn script_errors_recover_and_process_exit_restarts() {
    let supervisor = Supervisor::default();
    assert!(!supervisor.run("one", "1 / 0", |_| {}).await.unwrap().ok);
    run(&supervisor, "one", "42").await;
    assert!(supervisor
        .run("one", "import os\nos._exit(1)", |_| {})
        .await
        .is_err());
    run(&supervisor, "one", "42").await;
}

#[tokio::test]
async fn missing_uv_is_actionable() {
    let supervisor = Supervisor::with_uv("/codingpad/nonexistent/uv".into());
    let error = supervisor.run("one", "42", |_| {}).await.unwrap_err();
    assert!(error.contains("uv was not found"));
    assert!(error.contains("PATH"));
}

#[tokio::test]
async fn shutdown_terminates_a_running_kernel() {
    let supervisor = std::sync::Arc::new(Supervisor::default());
    let running = supervisor.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut tx = Some(tx);
    let task = tokio::spawn(async move {
        running
            .run(
                "one",
                "import time\nprint('started')\ntime.sleep(60)",
                |event| {
                    if event.params["text"]
                        .as_str()
                        .is_some_and(|text| text.contains("started"))
                    {
                        let _ = tx.take().unwrap().send(());
                    }
                },
            )
            .await
    });
    timeout(Duration::from_secs(15), rx).await.unwrap().unwrap();
    timeout(Duration::from_secs(3), supervisor.shutdown())
        .await
        .unwrap();
    assert!(task.await.unwrap().is_err());
}

#[tokio::test]
async fn surrogate_text_and_dump_labels_are_valid_for_serde() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "one",
        r#"print('\ud800')
dump({'\udcff': '\udfff'})
import os
os.getpid()"#,
    )
    .await;
    assert!(events.iter().any(|event| event.params["text"]
        .as_str()
        .is_some_and(|text| text.contains('?'))));
    assert!(events
        .iter()
        .any(|event| event.params["node"]["fields"][0]["name"] == "?"));
    run(&supervisor, "one", "42").await;
}

#[tokio::test]
async fn delayed_output_between_runs_does_not_corrupt_or_restart_kernel() {
    let supervisor = Supervisor::default();
    let folder = tempfile::tempdir().unwrap();
    let marker = folder.path().join("late-output-finished");
    let path = serde_json::to_string(&marker.to_string_lossy()).unwrap();
    let source = format!("import os, threading\ndef late():\n    print('late python')\n    os.write(1, b'late native\\n')\n    open({path}, 'w').close()\nthreading.Timer(0.05, late).start()\nos.getpid()");
    let first = run(&supervisor, "one", &source).await;
    timeout(Duration::from_secs(5), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let second = run(&supervisor, "one", "import os\nos.getpid()").await;
    assert_eq!(value(&first), value(&second));
}

#[tokio::test]
async fn native_children_cannot_consume_protocol_input() {
    let supervisor = Supervisor::default();
    run(&supervisor, "one", "import subprocess, sys\nresult = subprocess.run([sys.executable, '-c', 'import sys; assert sys.stdin.read() == chr(0)[:0]'], check=True, timeout=2)").await;
    run(&supervisor, "one", "42").await;
}

#[tokio::test]
async fn table_pages_expand_and_expire_between_runs() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "one",
        "rows = [{'name': f'row {i}', 'score': i} for i in range(250)]\ndump(rows)",
    )
    .await;
    let table = events
        .iter()
        .find(|event| event.method == "dump")
        .unwrap()
        .params["node"]
        .clone();
    assert_eq!(table["kind"], "table");
    assert_eq!(table["total"], 250);
    assert_eq!(table["rows"].as_array().unwrap().len(), 100);
    let handle = table["handle"].as_str().unwrap().to_owned();

    let page = supervisor
        .expand("one", &handle, Some("200"), Some(100))
        .await
        .unwrap();
    assert_eq!(page.node["rows"].as_array().unwrap().len(), 50);
    assert_eq!(page.node["rows"][0][1]["v"], 200);
    assert!(page.node["next"].is_null());

    run(&supervisor, "one", "42").await;
    let expired = supervisor
        .expand("one", &handle, None, None)
        .await
        .unwrap_err();
    assert!(expired.contains("no longer available"));
}

#[tokio::test]
async fn lazy_values_expand_on_request() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "one",
        "x = 1\nfor _ in range(8): x = (x,)\ndump(x)",
    )
    .await;
    let node = events
        .iter()
        .find(|event| event.method == "dump")
        .unwrap()
        .params["node"]
        .clone();

    fn find_lazy(node: &Value) -> Option<String> {
        if node["kind"] == "lazy" {
            return node["handle"].as_str().map(str::to_owned);
        }
        for field in node["fields"].as_array().into_iter().flatten() {
            if let Some(handle) = find_lazy(&field["value"]) {
                return Some(handle);
            }
        }
        for item in node["items"].as_array().into_iter().flatten() {
            if let Some(handle) = find_lazy(item) {
                return Some(handle);
            }
        }
        None
    }

    let handle = find_lazy(&node).unwrap();
    let expanded = supervisor.expand("one", &handle, None, None).await.unwrap();
    assert_ne!(expanded.node["kind"], "lazy");
}

#[tokio::test]
async fn expanding_an_unknown_script_is_actionable() {
    let supervisor = Supervisor::default();
    let error = supervisor
        .expand("missing", "h1", None, None)
        .await
        .unwrap_err();
    assert!(error.contains("no longer available"));
}

#[tokio::test]
async fn expand_drops_a_kernel_that_died() {
    let supervisor = Supervisor::default();
    run(
        &supervisor,
        "one",
        "import os, threading\nthreading.Timer(0.05, lambda: os._exit(1)).start()\n42",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let error = supervisor
        .expand("one", "h1", None, None)
        .await
        .unwrap_err();
    assert!(
        error.contains("exited")
            || error.contains("Could not")
            || error.contains("Invalid kernel message"),
        "{error}"
    );
    // A failed expand must not leave the dead process behind for the next run.
    run(&supervisor, "one", "42").await;
}

#[tokio::test]
async fn an_expired_handle_keeps_the_warm_kernel() {
    let supervisor = Supervisor::default();
    let first = run(&supervisor, "one", "import os\nos.getpid()").await;
    let error = supervisor
        .expand("one", "missing", None, None)
        .await
        .unwrap_err();
    assert!(error.contains("no longer available"));
    let second = run(&supervisor, "one", "import os\nos.getpid()").await;
    assert_eq!(value(&first), value(&second));
}

#[tokio::test]
async fn cleanup_failure_reports_run_error_without_restarting_kernel() {
    let supervisor = Supervisor::default();
    let before = run(&supervisor, "one", "import os\nos.getpid()").await;
    let mut events = Vec::new();
    let result = timeout(
        Duration::from_secs(5),
        supervisor.run(
            "one",
            r#"import sys
kernel = sys.stdout.drain.__globals__
notify = kernel["notify"]
def fail_finished(method, params):
    if method == "run_finished":
        kernel["notify"] = notify
        raise KeyboardInterrupt("cleanup failure")
    notify(method, params)
kernel["notify"] = fail_finished
"#,
            |event| events.push(event),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!result.ok);
    assert!(events.iter().any(|event| event.method == "stream"
        && event.params["stream"] == "stderr"
        && event.params["text"]
            .as_str()
            .is_some_and(|text| text.contains("KeyboardInterrupt"))));
    assert_eq!(events.last().unwrap().method, "run_finished");
    assert_eq!(events.last().unwrap().params["ok"], false);
    let after = run(&supervisor, "one", "import os\nos.getpid()").await;
    assert_eq!(value(&before), value(&after));
}

async fn start_until_ready(
    supervisor: std::sync::Arc<Supervisor>,
    source: &'static str,
) -> tokio::task::JoinHandle<codingpad_kernel_core::RunResult> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut tx = Some(tx);
    let task = tokio::spawn(async move {
        supervisor
            .run("one", source, |event| {
                if event.params["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("ready"))
                {
                    if let Some(tx) = tx.take() {
                        let _ = tx.send(());
                    }
                }
            })
            .await
            .unwrap()
    });
    timeout(Duration::from_secs(5), rx).await.unwrap().unwrap();
    task
}

#[tokio::test]
async fn cooperative_stop_preserves_cache_and_warm_process() {
    let supervisor = std::sync::Arc::new(Supervisor::default());
    let before = run(
        &supervisor,
        "one",
        "import os\ncache('pid', os.getpid)\nos.getpid()",
    )
    .await;
    let task = start_until_ready(
        supervisor.clone(),
        "print('ready', flush=True)\nwhile True: pass",
    )
    .await;
    timeout(Duration::from_secs(5), supervisor.stop("one"))
        .await
        .unwrap();
    let result = task.await.unwrap();
    assert!(result.stopped);
    assert!(!result.ok);
    assert!(!result.cache_cleared);
    let after = run(&supervisor, "one", "cache('pid', lambda: 'lost')").await;
    assert_eq!(value(&before), value(&after));
    let pid = run(&supervisor, "one", "import os\nos.getpid()").await;
    assert_eq!(value(&before), value(&pid));
    // Idle Stop must not cancel the next run.
    supervisor.stop("one").await;
    run(&supervisor, "one", "42").await;
}

#[tokio::test]
async fn unresponsive_stop_restarts_kernel_and_clears_cache() {
    let supervisor = std::sync::Arc::new(Supervisor::default());
    let before = run(
        &supervisor,
        "one",
        "import os\ncache('saved', lambda: 42)\nos.getpid()",
    )
    .await;
    // Disabling tracing also exercises the escape hatch for native blocking code.
    let task = start_until_ready(
        supervisor.clone(),
        "import sys, time\nsys.settrace(None)\nprint('ready', flush=True)\ntime.sleep(60)",
    )
    .await;
    timeout(Duration::from_secs(6), supervisor.stop("one"))
        .await
        .unwrap();
    let result = task.await.unwrap();
    assert!(result.stopped);
    assert!(result.cache_cleared);
    let after = run(&supervisor, "one", "import os\nos.getpid()").await;
    assert_ne!(value(&before), value(&after));
    let cached = run(&supervisor, "one", "cache('saved', lambda: 'cleared')").await;
    assert_eq!(value(&cached), "cleared");
}

#[tokio::test]
async fn reset_clears_cache_and_handles_without_affecting_other_scripts() {
    let supervisor = Supervisor::default();
    run(&supervisor, "one", "cache('key', lambda: 42)").await;
    run(&supervisor, "two", "cache('key', lambda: 99)").await;
    let table = run(&supervisor, "one", "[{'n': i} for i in range(200)]").await;
    let handle = table
        .iter()
        .find(|event| event.method == "dump")
        .unwrap()
        .params["node"]["handle"]
        .as_str()
        .unwrap()
        .to_owned();
    supervisor.reset("one").await.unwrap();
    assert!(supervisor.expand("one", &handle, None, None).await.is_err());
    let one = run(&supervisor, "one", "cache('key', lambda: 'new')").await;
    let two = run(&supervisor, "two", "cache('key', lambda: 'new')").await;
    assert_eq!(value(&one), "new");
    assert_eq!(value(&two), 99);
}
