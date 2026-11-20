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
async fn inspects_rust_types_and_expands_deep_values_after_exit() {
    let supervisor = Supervisor::default();
    let (result, events) = run(
        &supervisor,
        r#"
        use codingpad::dump;
        #[derive(Debug)]
        enum State { Ready, Busy(u8), Failed { code: u8 } }
        #[derive(Debug)]
        struct Job { name: &'static str, state: State, attempts: Option<Vec<u8>> }
        dump(Job { name: "line\0end", state: State::Busy(2), attempts: Some(vec![3, 4]) });
        dump(State::Ready);
        dump(State::Failed { code: 5 });
        dump(None::<u8>);
        dump(std::collections::BTreeMap::from([("total", 42)]));
        dump(std::collections::BTreeSet::from([10, 20]));
        dump(vec![vec![vec![vec![vec![vec![vec![13]]]]]]]);
    "#,
    )
    .await;
    assert!(result.ok, "{events:?}");
    let values = dumps(&events);
    let fields = values[0]["fields"].as_array().unwrap();
    assert_eq!(values[0]["type"], "Job");
    assert_eq!(fields[0]["value"]["v"], "line\0end");
    assert_eq!(fields[1]["value"]["type"], "Busy");
    assert_eq!(fields[1]["value"]["fields"][0]["value"]["v"], 2);
    assert_eq!(fields[2]["value"]["type"], "Some");
    assert_eq!(fields[2]["value"]["fields"][0]["value"]["items"][1]["v"], 4);
    assert_eq!(values[1]["v"], "Ready");
    assert_eq!(values[2]["fields"][0]["value"]["v"], 5);
    assert_eq!(values[3]["kind"], "null");
    assert_eq!(values[4]["fields"][0]["name"], "total");
    assert_eq!(values[5]["items"][1]["v"], 20);
    let mut nested = values[6];
    for _ in 0..6 {
        nested = &nested["items"][0];
    }
    assert_eq!(nested["kind"], "lazy");
    let handle = nested["handle"].as_str().unwrap();
    let page = supervisor
        .expand("rust-script", handle, None, None)
        .await
        .unwrap();
    assert_eq!(page.node["items"][0]["v"], 13);

    assert!(run(&supervisor, "42").await.0.ok);
    assert!(supervisor
        .expand("rust-script", handle, None, None)
        .await
        .is_err());
}

#[tokio::test]
async fn pages_exports_and_resets_table_snapshots() {
    let supervisor = Supervisor::default();
    let (result, events) = run(&supervisor, r#"
        #[derive(Debug)]
        struct Row { number: u32, note: String, values: Vec<u32> }
        (0..230).map(|number| Row { number, note: "x".repeat(1100), values: vec![number] }).collect::<Vec<_>>()
    "#).await;
    assert!(result.ok, "{events:?}");
    let values = dumps(&events);
    let table = &values[0];
    assert_eq!(table["kind"], "table");
    assert_eq!(table["rows"].as_array().unwrap().len(), 100);
    assert_eq!(table["total"], 230);
    assert_eq!(table["rows"][0][1]["truncated"], true);
    assert_eq!(table["rows"][0][2]["kind"], "lazy");
    let handle = table["handle"].as_str().unwrap();
    let cursor = table["next"].as_str().unwrap();
    let page = supervisor
        .expand("rust-script", handle, Some(cursor), Some(55))
        .await
        .unwrap()
        .node;
    assert_eq!(page["rows"][0][0]["v"], 100);
    assert_eq!(page["next"], "155");
    let exported = supervisor
        .export_page("rust-script", handle, Some("228"), 10)
        .await
        .unwrap();
    assert_eq!(exported.columns, ["number", "note", "values"]);
    assert_eq!(exported.rows.len(), 2);
    assert_eq!(exported.rows[0][1].as_str().unwrap().len(), 1100);
    assert_eq!(exported.rows[1][0], 229);
    assert_eq!(exported.rows[1][2], "[229]");
    assert!(exported.next.is_none());
    supervisor.reset("rust-script").await.unwrap();
    assert!(supervisor
        .expand("rust-script", handle, None, None)
        .await
        .is_err());
}

#[tokio::test]
async fn emits_images_html_and_live_updates_with_stable_ids() {
    let supervisor = Supervisor::default();
    let (result, events) = run(
        &supervisor,
        r##"
        use codingpad::{dump, html, image, image_bytes, live};
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"></svg>"#;
        std::fs::write("pixel.svg", svg).unwrap();
        dump(image("pixel.svg").unwrap());
        dump(image_bytes(svg).unwrap());
        dump(html("<strong>Ready</strong>"));
        let status = live(vec![vec!["starting"]; 105]);
        dump(&status);
        dump(&status);
        status.set(vec![vec!["ready"]; 105]);
    "##,
    )
    .await;
    assert!(result.ok, "{events:?}");
    let values = dumps(&events);
    assert_eq!(values[0]["kind"], "image");
    assert_eq!(values[0]["name"], "pixel.svg");
    assert!(values[0]["src"]
        .as_str()
        .unwrap()
        .starts_with("data:image/svg+xml;base64,PHN2Zy"));
    assert_eq!(values[1]["src"], values[0]["src"]);
    assert_eq!(values[2]["html"], "<strong>Ready</strong>");
    let updates = events
        .iter()
        .filter(|event| event.method == "dump_update")
        .collect::<Vec<_>>();
    assert_eq!(updates.len(), 2, "{events:?}");
    for (initial, update) in values[3..].iter().zip(updates) {
        assert_eq!(initial["kind"], "live");
        assert_eq!(initial["node"]["id"], update.params["nodeId"]);
        assert_ne!(update.params["node"]["id"], update.params["nodeId"]);
        assert_eq!(update.params["node"]["items"][0]["items"][0]["v"], "ready");
        let old_handle = initial["node"]["handle"].as_str().unwrap();
        assert!(supervisor
            .expand("rust-script", old_handle, None, None)
            .await
            .is_err());
        let new_handle = update.params["node"]["handle"].as_str().unwrap();
        assert_eq!(
            supervisor
                .expand("rust-script", new_handle, Some("100"), None)
                .await
                .unwrap()
                .node["items"][0]["items"][0]["v"],
            "ready"
        );
    }
}

#[tokio::test]
async fn maps_panics_in_snippets_and_native_main_to_script_frames() {
    let supervisor = Supervisor::default();
    let (result, events) = run(
        &supervisor,
        "fn crash() {\n    panic!(\"boom\");\n}\ncrash();",
    )
    .await;
    assert!(!result.ok, "{events:?}");
    let values = dumps(&events);
    assert_eq!(values[0]["type"], "Rust panic");
    assert_eq!(values[0]["message"], "boom");
    let trace = values[0]["traceback"].as_array().unwrap();
    assert_eq!(trace[0]["line"], 2);
    assert_eq!(trace[0]["column"], 5);
    assert_eq!(trace[0]["script"], true);
    assert!(
        trace
            .iter()
            .any(|frame| frame["line"] == 4 && frame["script"] == true),
        "{trace:?}"
    );

    let source = "fn main() {\tlet _ = \"漢😀\"; panic!(\"main\"); }";
    let (result, events) = run(&supervisor, source).await;
    assert!(!result.ok, "{events:?}");
    let values = dumps(&events);
    assert_eq!(values[0]["traceback"][0]["line"], 1);
    let column = source[..source.find("panic!").unwrap()]
        .encode_utf16()
        .count()
        + 1;
    assert_eq!(values[0]["traceback"][0]["column"], column);
    assert_eq!(values[0]["traceback"][0]["script"], true);

    let (result, events) = run(
        &supervisor,
        "let _ = std::panic::catch_unwind(|| panic!(\"caught\"));\n42",
    )
    .await;
    assert!(result.ok, "{events:?}");
    assert_eq!(dumps(&events).len(), 1);
    assert_eq!(dumps(&events)[0]["v"], 42);
}

#[tokio::test]
async fn pages_large_tables_without_materializing_all_rows_in_results() {
    let supervisor = Supervisor::default();
    let (result, events) = run(
        &supervisor,
        "#[derive(Debug)] struct Row { n: u32 }\n(0..100_000).map(|n| Row { n }).collect::<Vec<_>>()",
    ).await;
    assert!(result.ok);
    let values = dumps(&events);
    let table = values[0];
    assert_eq!(table["kind"], "table");
    assert_eq!(table["total"], 100_000);
    assert_eq!(table["rows"].as_array().unwrap().len(), 100);
    let handle = table["handle"].as_str().unwrap();
    let last = supervisor
        .expand("rust-script", handle, Some("99999"), None)
        .await
        .unwrap()
        .node;
    assert_eq!(last["rows"][0][0]["v"], 99_999);
    assert_eq!(last["next"], Value::Null);
}

#[tokio::test]
async fn a_live_update_can_arrive_before_the_initial_dump() {
    let supervisor = Supervisor::default();
    let (result, events) = run(
        &supervisor,
        r#"
        use codingpad::{dump, live, Live};
        use std::fmt::{self, Debug, Formatter};
        use std::sync::mpsc::{channel, Sender, Receiver};

        struct Delayed<'a>(&'a Live<u32>, Sender<()>, Receiver<()>);
        impl Debug for Delayed<'_> {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)?;
                self.1.send(()).unwrap();
                self.2.recv().unwrap();
                Ok(())
            }
        }
        let status = live(1);
        let (ready, can_update) = channel();
        let (updated, can_dump) = channel();
        std::thread::scope(|scope| {
            let progress = &status;
            scope.spawn(move || {
                can_update.recv().unwrap();
                progress.set(2);
                updated.send(()).unwrap();
            });
            dump(Delayed(&status, ready, can_dump));
        });
    "#,
    )
    .await;
    assert!(result.ok, "{events:?}");
    assert_eq!(dumps(&events)[0]["node"]["v"], 2);
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
