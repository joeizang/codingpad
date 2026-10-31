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
    assert!(events.iter().any(|e| e.params["text"] == "hello"));
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut tx = Some(tx);
    let handle = tokio::spawn(async move {
        supervisor
            .run(
                "one",
                "import time\nprint('before sleep')\ntime.sleep(1)",
                |e| {
                    if e.params["text"] == "before sleep" {
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
                    if event.params["text"] == "started" {
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
