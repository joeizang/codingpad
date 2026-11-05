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
    assert!(error.contains("Install uv and run"));
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
    // Ignoring SIGINT exercises the fallback for uncooperative/native code.
    let task = start_until_ready(
        supervisor.clone(),
        "import signal, time\nsignal.signal(signal.SIGINT, signal.SIG_IGN)\nprint('ready', flush=True)\ntime.sleep(60)",
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

#[tokio::test]
async fn stop_interrupts_imported_python_code_without_clearing_cache() {
    let supervisor = std::sync::Arc::new(Supervisor::default());
    let task = start_until_ready(
        supervisor.clone(),
        r#"library = {}
exec(compile('def spin():\n    while True: pass', 'library.py', 'exec'), library)
print('ready', flush=True)
library['spin']()
"#,
    )
    .await;
    timeout(Duration::from_secs(5), supervisor.stop("one"))
        .await
        .unwrap();
    let result = task.await.unwrap();
    assert!(result.stopped);
    assert!(!result.cache_cleared);
}

#[cfg(unix)]
#[tokio::test]
async fn stop_interrupts_sleep_without_restarting_kernel() {
    let supervisor = std::sync::Arc::new(Supervisor::default());
    let task = start_until_ready(
        supervisor.clone(),
        "import time\nprint('ready', flush=True)\ntime.sleep(60)",
    )
    .await;
    timeout(Duration::from_secs(5), supervisor.stop("one"))
        .await
        .unwrap();
    let result = task.await.unwrap();
    assert!(result.stopped);
    assert!(!result.cache_cleared);
}

#[tokio::test]
async fn successful_run_after_stop_request_is_not_reported_as_stopped() {
    let supervisor = std::sync::Arc::new(Supervisor::default());
    let task = start_until_ready(
        supervisor.clone(),
        r#"import signal, time
signal.signal(signal.SIGINT, lambda *args: None)
print('ready', flush=True)
time.sleep(0.2)
42
"#,
    )
    .await;
    timeout(Duration::from_secs(5), supervisor.stop("one"))
        .await
        .unwrap();
    let result = task.await.unwrap();
    assert!(result.ok);
    assert!(!result.stopped);
    assert!(!result.cache_cleared);
}

// Build a tiny real wheel locally: these package tests never depend on PyPI.
fn package_fixture(folder: &std::path::Path, version: u8) -> String {
    let output = std::process::Command::new("uv")
        .args(["run", "--offline", "--no-project", "--no-config", "--no-python-downloads", "python", "-c", r#"
import pathlib, sys, zipfile
folder, version = pathlib.Path(sys.argv[1]), sys.argv[2]
name = 'cpad_package_fixture'
wheel = folder / f'{name}-{version}-py3-none-any.whl'
info = f'{name}-{version}.dist-info'
with zipfile.ZipFile(wheel, 'w') as z:
    z.writestr(f'{name}.py', f'VALUE = {version}\n')
    z.writestr(f'{info}/METADATA', f'Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n')
    z.writestr(f'{info}/WHEEL', 'Wheel-Version: 1.0\nGenerator: CodingPad test\nRoot-Is-Purelib: true\nTag: py3-none-any\n')
    z.writestr(f'{info}/RECORD', '')
print(f'{name} @ {wheel.as_uri()}')
"#])
        .arg(folder).arg(version.to_string()).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn package_source(requirement: &str, code: &str) -> String {
    format!(
        "# /// script\n# dependencies = [{}]\n# ///\n{code}",
        serde_json::to_string(requirement).unwrap()
    )
}

#[tokio::test]
async fn inline_packages_install_reuse_and_change_without_leaking_imports() {
    let folder = tempfile::tempdir().unwrap();
    let first = package_fixture(folder.path(), 1);
    let second = package_fixture(folder.path(), 2);
    let supervisor = Supervisor::default();
    let code = "import cpad_package_fixture as fixture\nassert fixture.VALUE == 1\ncache('saved', lambda: 42)\nimport os\nimport sys\nos.path.realpath(sys.executable)";
    let source = package_source(&first, code);
    let events = run(&supervisor, "one", &source).await;
    assert!(events.iter().any(|event| event.params["text"]
        .as_str()
        .is_some_and(|text| text.contains("Preparing Python packages"))));
    let again = run(
        &supervisor,
        "one",
        &package_source(&first, "cache('saved', lambda: 'lost')"),
    )
    .await;
    assert_eq!(value(&again), 42);
    // A different tab and Reset reuse the environment, with independent kernels.
    let other = run(&supervisor, "two", &source).await;
    assert_eq!(value(&events), value(&other));
    supervisor.reset("one").await.unwrap();
    let reset = run(&supervisor, "one", &source).await;
    assert_eq!(value(&events), value(&reset));
    let changed = run(&supervisor, "one", &package_source(&second, "import cpad_package_fixture as fixture\nassert fixture.VALUE == 2\ncache('saved', lambda: 'cleared')")).await;
    assert_eq!(value(&changed), "cleared");
    let removed = run(&supervisor, "one", "import importlib.util\nassert importlib.util.find_spec('cpad_package_fixture') is None\n42").await;
    assert_eq!(value(&removed), 42);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn package_errors_stream_details_and_recover() {
    let supervisor = Supervisor::default();
    let mut events = Vec::new();
    let error = supervisor
        .run(
            "one",
            &package_source(
                "invalid requirement >>>",
                "raise AssertionError('must not execute')",
            ),
            |e| events.push(e),
        )
        .await
        .unwrap_err();
    assert!(error.contains("package preparation failed"));
    assert!(events.iter().any(|event| event.params["text"]
        .as_str()
        .is_some_and(|text| text.contains("error:"))));
    let source = "# /// script\n# requires-python = '>=99'\n# ///\n42";
    assert!(supervisor
        .run("one", source, |_| {})
        .await
        .unwrap_err()
        .contains("package preparation failed"));
    run(&supervisor, "one", "42").await;
}

#[cfg(unix)]
#[tokio::test]
async fn stop_and_close_cancel_package_preparation() {
    use std::os::unix::fs::PermissionsExt;
    let folder = tempfile::tempdir().unwrap();
    let wrapper = folder.path().join("uv-wrapper");
    std::fs::write(&wrapper, "#!/bin/sh\nif [ \"$1\" = python ]; then exec uv \"$@\"; fi\nprintf 'install-ready\\n' >&2\nexec sleep 60\n").unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let supervisor = Supervisor::with_uv(wrapper);
    for close in [false, true] {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut tx = Some(tx);
        let source = package_source("requests", "42");
        let running = supervisor.run("one", &source, |event| {
            if event.params["text"]
                .as_str()
                .is_some_and(|text| text.contains("install-ready"))
            {
                if let Some(tx) = tx.take() {
                    let _ = tx.send(());
                }
            }
        });
        let control = async {
            rx.await.unwrap();
            if close {
                supervisor.close("one").await;
            } else {
                supervisor.stop("one").await;
            }
        };
        let (result, ()) = timeout(Duration::from_secs(5), async {
            tokio::join!(running, control)
        })
        .await
        .unwrap();
        if close {
            assert!(result.unwrap_err().contains("closed"));
        } else {
            assert!(result.unwrap().stopped);
        }
        run(&supervisor, "one", "42").await;
    }
}

#[tokio::test]
async fn empty_metadata_uses_an_isolated_environment() {
    let supervisor = Supervisor::default();
    run(&supervisor, "empty", "# /// script\n# dependencies = []\n# ///\nimport sys\nassert sys.prefix != sys.base_prefix\n42").await;
}

#[tokio::test]
#[ignore = "optional PyPI acceptance check; needs network or a populated uv cache"]
async fn requests_first_run_and_cached_environment_after_restart() {
    let source = package_source(
        "requests>=2,<3",
        "import requests, sys\nassert requests.__version__.startswith('2.')\nsys.prefix",
    );
    let supervisor = Supervisor::default();
    let first = run(&supervisor, "requests", &source).await;
    supervisor.shutdown().await;
    let supervisor = Supervisor::default();
    let second = run(&supervisor, "requests", &source).await;
    assert_eq!(value(&first), value(&second));
}

#[tokio::test]
async fn python_uses_the_attached_sqlite_connection() {
    use codingpad_kernel_core::{
        databases::{Connection, ConnectionStore, DatabaseKind, TlsMode},
        secrets::SecretStore,
    };
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.sqlite");
    let database = rusqlite::Connection::open(&path).unwrap();
    database
        .execute_batch("CREATE TABLE items(name TEXT); INSERT INTO items VALUES ('Ada');")
        .unwrap();
    let connections = Arc::new(ConnectionStore::new(dir.path()));
    connections
        .save(Connection {
            id: String::new(),
            name: "sample".into(),
            kind: DatabaseKind::Sqlite,
            path: Some(path.to_string_lossy().into()),
            host: None,
            port: None,
            database: None,
            user: None,
            secret: None,
            tls: TlsMode::Prefer,
            trust_server_certificate: false,
        })
        .unwrap();
    let supervisor = Supervisor::with_data_dir_and_secrets(
        dir.path().into(),
        Arc::new(SecretStore::new(dir.path())),
    )
    .with_connections(connections);
    let events = run(
        &supervisor,
        "db-test",
        "# codingpad: connection=sample\ndb.query('select name from items')",
    )
    .await;
    let table = &events
        .iter()
        .find(|event| event.method == "dump")
        .unwrap()
        .params["node"];
    assert_eq!(table["kind"], "table");
    assert_eq!(table["rows"][0][0]["v"], "Ada");
}

#[tokio::test]
async fn csharp_uses_the_attached_sqlite_connection() {
    use codingpad_kernel_core::{
        databases::{Connection, ConnectionStore, DatabaseKind, TlsMode},
        secrets::SecretStore,
        Language,
    };
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.sqlite");
    let database = rusqlite::Connection::open(&path).unwrap();
    database
        .execute_batch("CREATE TABLE items(name TEXT); INSERT INTO items VALUES ('Ada');")
        .unwrap();
    let connections = Arc::new(ConnectionStore::new(dir.path()));
    connections
        .save(Connection {
            id: String::new(),
            name: "sample".into(),
            kind: DatabaseKind::Sqlite,
            path: Some(path.to_string_lossy().into()),
            host: None,
            port: None,
            database: None,
            user: None,
            secret: None,
            tls: TlsMode::Prefer,
            trust_server_certificate: false,
        })
        .unwrap();
    let supervisor = Supervisor::with_data_dir_and_secrets(
        dir.path().into(),
        Arc::new(SecretStore::new(dir.path())),
    )
    .with_connections(connections);
    let mut events = Vec::new();
    let result = timeout(
        Duration::from_secs(30),
        supervisor.run_language(
            "db-test",
            "// codingpad: connection=sample\ndb.Query(\"select name from items\").Dump();",
            Language::Csharp,
            |e| events.push(e),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(result.ok, "{events:?}");
    let table = &events
        .iter()
        .find(|event| event.method == "dump")
        .unwrap()
        .params["node"];
    assert_eq!(table["kind"], "table");
    assert_eq!(table["rows"][0][0]["v"], "Ada");
}

/// Run against explicitly configured local test servers; no credentials are stored.
#[tokio::test]
async fn language_clients_query_attached_server_databases() {
    use codingpad_kernel_core::{
        databases::{Connection, ConnectionStore, DatabaseKind, TlsMode},
        secrets::SecretStore,
        Language,
    };
    use std::sync::Arc;
    for (kind, prefix) in [
        (DatabaseKind::Postgres, "CODINGPAD_TEST_PG"),
        (DatabaseKind::SqlServer, "CODINGPAD_TEST_MSSQL"),
    ] {
        let (Ok(port), Ok(database), Ok(user), Ok(password)) = (
            std::env::var(format!("{prefix}_PORT")),
            std::env::var(format!("{prefix}_DATABASE")),
            std::env::var(format!("{prefix}_USER")),
            std::env::var(format!("{prefix}_PASSWORD")),
        ) else {
            continue;
        };
        let dir = tempfile::tempdir().unwrap();
        let connections = Arc::new(ConnectionStore::new(dir.path()));
        let secrets = Arc::new(SecretStore::new(dir.path()));
        secrets.set_session("connection:test", &password).unwrap();
        connections
            .save(Connection {
                id: String::new(),
                name: "test".into(),
                kind,
                path: None,
                host: Some("127.0.0.1".into()),
                port: Some(port.parse().unwrap()),
                database: Some(database),
                user: Some(user),
                secret: Some("connection:test".into()),
                tls: if kind == DatabaseKind::Postgres {
                    TlsMode::Disable
                } else {
                    TlsMode::Require
                },
                trust_server_certificate: true,
            })
            .unwrap();
        let supervisor = Supervisor::with_data_dir_and_secrets(dir.path().into(), secrets)
            .with_connections(connections);
        for (language, script) in [
            (
                Language::Python,
                "# codingpad: connection=test\ndb.query('select 42 as answer')",
            ),
            (
                Language::Csharp,
                "// codingpad: connection=test\ndb.Query(\"select 42 as answer\").Dump();",
            ),
        ] {
            let mut events = Vec::new();
            let result = timeout(
                Duration::from_secs(120),
                supervisor.run_language("live", script, language, |event| events.push(event)),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(result.ok, "{kind:?} {language:?}: {events:?}");
            let table = &events
                .iter()
                .find(|event| event.method == "dump")
                .unwrap()
                .params["node"];
            assert_eq!(table["kind"], "table");
            assert_eq!(table["rows"][0][0]["v"], 42);
        }
    }
}
