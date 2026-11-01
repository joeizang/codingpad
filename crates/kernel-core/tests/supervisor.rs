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
