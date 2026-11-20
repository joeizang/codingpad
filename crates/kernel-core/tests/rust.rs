use codingpad_kernel_core::{KernelEvent, Language, RunResult, Supervisor};
use serde_json::Value;

async fn run(supervisor: &Supervisor, source: &str) -> (RunResult, Vec<KernelEvent>) {
    let mut events = Vec::new();
    let result = supervisor
        .run_language("rust-script", source, Language::Rust, |event| {
            events.push(event)
        })
        .await
        .unwrap();
    (result, events)
}

fn dumps(events: &[KernelEvent]) -> Vec<&Value> {
    events
        .iter()
        .filter(|event| event.method == "dump")
        .map(|event| &event.params["node"])
        .collect()
}

#[tokio::test]
async fn captures_stdout_and_dumps_only_a_plain_tail() {
    let supervisor = Supervisor::default();
    let (result, events) = run(
        &supervisor,
        r#"
        use codingpad::dump;
        println!("hello from Cargo");
        dump("explicit");
        let values = [10, 20, 12];
        values.iter().sum::<i32>()
    "#,
    )
    .await;
    assert!(result.ok, "{events:?}");
    assert!(events
        .iter()
        .any(|event| event.params["text"] == "hello from Cargo\n"));
    let values = dumps(&events);
    assert_eq!(values.len(), 2);
    assert_eq!(values[0]["v"], "explicit");
    assert_eq!(values[1]["v"], 42);

    let (result, events) = run(&supervisor, "use codingpad::dump; dump(7); 42;").await;
    assert!(result.ok, "{events:?}");
    assert_eq!(dumps(&events).len(), 1);
    assert_eq!(dumps(&events)[0]["v"], 7);
}

#[tokio::test]
async fn maps_compile_diagnostics_and_never_executes_a_failed_build() {
    let supervisor = Supervisor::default();
    let (result, events) = run(
        &supervisor,
        "println!(\"must not run\");\nlet café = \"☕\";\ncafé + 1",
    )
    .await;
    assert!(!result.ok);
    assert!(events
        .iter()
        .all(|event| event.params["stream"] != "stdout"));
    let markers = &events
        .iter()
        .find(|event| event.method == "diagnostics")
        .unwrap()
        .params["diagnostics"];
    assert_eq!(markers[0]["line"], 3, "{events:?}");
    assert_eq!(markers[0]["column"], 6);
    assert_eq!(dumps(&events)[0]["traceback"][0]["line"], 3);
}

#[tokio::test]
async fn runs_explicit_main_against_the_saved_scripts_directory() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native.rs");
    std::fs::write(&path, "disk contents").unwrap();
    std::fs::write(directory.path().join("input.txt"), "relative file").unwrap();
    let mut events = Vec::new();
    let result = Supervisor::default()
        .run_language_at(
            "rust-script",
            r#"fn main() { codingpad::dump(std::fs::read_to_string("input.txt").unwrap()); }"#,
            Language::Rust,
            path.to_str(),
            |event| events.push(event),
        )
        .await
        .unwrap();
    assert!(result.ok, "{events:?}");
    assert_eq!(dumps(&events)[0]["v"], "relative file");
    assert_eq!(std::fs::read_to_string(path).unwrap(), "disk contents");
}

#[tokio::test]
async fn runs_top_level_scripts_with_a_bom_and_shebang() {
    let supervisor = Supervisor::default();
    for source in [
        "\u{feff}42",
        "#!/usr/bin/env rust-script\n42",
        "\u{feff}#!/usr/bin/env rust-script\r\n#![allow(unused)]\r\n42",
    ] {
        let (result, events) = run(&supervisor, source).await;
        assert!(result.ok, "{events:?}");
        assert_eq!(dumps(&events)[0]["v"], 42);
    }
}

#[tokio::test]
async fn stop_closes_a_rust_scripts_descendant_processes() {
    use tokio::io::AsyncReadExt;
    use tokio::time::{timeout, Duration};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let source = r#"
        fn main() {
            if std::env::args().any(|arg| arg == "child") {
                let mut socket = std::net::TcpStream::connect("ADDRESS").unwrap();
                std::io::Write::write_all(&mut socket, b"1").unwrap();
                let _ = std::io::Read::read(&mut socket, &mut [0]);
                return;
            }
            let _child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("child")
                .spawn()
                .unwrap();
            std::thread::park();
        }
    "#
    .replace("ADDRESS", &address);
    let supervisor = std::sync::Arc::new(Supervisor::default());
    let running = tokio::spawn({
        let supervisor = supervisor.clone();
        async move { run(&supervisor, &source).await.0 }
    });
    let (mut socket, _) = timeout(Duration::from_secs(30), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut ready = [0];
    socket.read_exact(&mut ready).await.unwrap();
    assert_eq!(ready, *b"1");

    supervisor.stop("rust-script").await;
    assert!(running.await.unwrap().stopped);
    match timeout(Duration::from_secs(5), socket.read(&mut ready))
        .await
        .unwrap()
    {
        Ok(0) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        result => panic!("expected the descendant's socket to close, got {result:?}"),
    }
}
