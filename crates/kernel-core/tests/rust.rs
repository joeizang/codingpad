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
        "fn crash() {\n    panic!(\"boom\");\n}\n\t\tlet _s = \"ééé\"; crash();",
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

    #[cfg(target_os = "linux")]
    {
        let caller = trace.iter().find(|frame| frame["line"] == 4).unwrap();
        let expected = "\t\tlet _s = \"ééé\"; ".encode_utf16().count() + 1;
        assert_eq!(caller["column"], expected, "{trace:?}");
    }

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
    assert!(events.iter().any(|event| event.params["stream"] == "stderr"
        && event.params["text"]
            .as_str()
            .is_some_and(|text| text.contains("caught"))));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_closes_a_rust_scripts_descendant_processes() {
    use tokio::io::AsyncReadExt;
    use tokio::time::{timeout, Duration, Instant};

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
            codingpad::cache("demo", || 42);
            let _child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("child")
                .spawn()
                .unwrap();
            while !codingpad::is_cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            println!("cancel observed");
            loop { std::thread::park(); }
        }
    "#
    .replace("ADDRESS", &address);
    let data = tempfile::tempdir().unwrap();
    let supervisor = std::sync::Arc::new(
        Supervisor::with_data_dir(data.path().to_owned()).with_session_rust_cache(),
    );
    let running = tokio::spawn({
        let supervisor = supervisor.clone();
        async move { run(&supervisor, &source).await }
    });
    let (mut socket, _) = timeout(Duration::from_secs(30), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut ready = [0];
    socket.read_exact(&mut ready).await.unwrap();
    assert_eq!(ready, *b"1");

    let started = Instant::now();
    timeout(Duration::from_secs(4), supervisor.stop("rust-script"))
        .await
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        (Duration::from_millis(1900)..Duration::from_secs(3)).contains(&elapsed),
        "Stop took {elapsed:?}"
    );
    let (result, events) = running.await.unwrap();
    assert!(events
        .iter()
        .any(|event| { event.method == "stream" && event.params["text"] == "cancel observed\n" }));
    assert!(result.stopped);
    assert!(result.cache_cleared);
    match timeout(Duration::from_secs(5), socket.read(&mut ready))
        .await
        .unwrap()
    {
        Ok(0) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        result => panic!("expected the descendant's socket to close, got {result:?}"),
    }
    let (result, events) = run(&supervisor, r#"codingpad::cache("demo", || 43)"#).await;
    assert!(result.ok);
    assert_eq!(dumps(&events)[0]["v"], 43);
}

#[tokio::test]
async fn recovered_panics_are_visible_and_do_not_mask_a_later_exit() {
    let supervisor = Supervisor::default();
    let (result, events) = run(
        &supervisor,
        r#"
        let _ = std::thread::spawn(|| panic!("worker recovery")).join();
        let _ = std::panic::catch_unwind(|| panic!("main recovery"));
        std::process::exit(7);
    "#,
    )
    .await;
    assert!(!result.ok);
    let stderr = events
        .iter()
        .filter(|event| event.params["stream"] == "stderr")
        .filter_map(|event| event.params["text"].as_str())
        .collect::<String>();
    assert!(stderr.contains("worker recovery"), "{events:?}");
    assert!(stderr.contains("main recovery"), "{events:?}");
    assert_eq!(dumps(&events).len(), 1);
    assert_eq!(dumps(&events)[0]["type"], "Rust script failed");
}

#[tokio::test]
async fn dump_preserves_mixed_variants_and_keeps_debug_markers_private() {
    let supervisor = Supervisor::default();
    let (result, events) = run(
        &supervisor,
        r#"
        use codingpad::{dump, html, live};
        use std::fmt;
        struct Custom(&'static str);
        impl fmt::Debug for Custom {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.0)
            }
        }
        dump(vec![Ok::<u8, u8>(1), Err(1)]);
        dump(['\0', '\'', '\\', '\u{1b}']);
        let status = live(Custom("custom formatting"));
        println!("{status:?}");
        println!("{:?}", html("<b>ready</b>"));
        status.set(Custom("still private"));
        dump(&status);
        status.set(Custom("now updated"));
    "#,
    )
    .await;
    assert!(result.ok, "{events:?}");
    let values = dumps(&events);
    assert_eq!(values[0]["kind"], "list");
    assert_eq!(values[0]["items"][0]["type"], "Ok");
    assert_eq!(values[0]["items"][1]["type"], "Err");
    let characters = values[1]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["v"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(characters, ["\0", "'", "\\", "\u{1b}"]);
    let stdout = events
        .iter()
        .filter(|event| event.params["stream"] == "stdout")
        .filter_map(|event| event.params["text"].as_str())
        .collect::<String>();
    assert_eq!(stdout, "custom formatting\n<b>ready</b>\n");
    assert_eq!(values[2]["kind"], "live");
    assert_eq!(values[2]["node"]["v"], "still private");
    let updates = events
        .iter()
        .filter(|event| event.method == "dump_update")
        .collect::<Vec<_>>();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].params["node"]["v"], "now updated");
}

#[tokio::test]
async fn rust_cache_survives_fresh_processes_and_can_be_reset_before_a_run() {
    let data = tempfile::tempdir().unwrap();
    let source = |value| format!(r#"codingpad::cache("example", || {value})"#);
    let supervisor = Supervisor::with_data_dir(data.path().to_owned());
    let (result, events) = run(&supervisor, &source(41)).await;
    assert!(result.ok);
    assert_eq!(dumps(&events)[0]["v"], 41);

    let supervisor = Supervisor::with_data_dir(data.path().to_owned());
    let (result, events) = run(&supervisor, &source(42)).await;
    assert!(result.ok);
    assert_eq!(dumps(&events)[0]["v"], 41);
    drop(supervisor);

    let supervisor = Supervisor::with_data_dir(data.path().to_owned());
    supervisor.reset("rust-script").await.unwrap();
    let (result, events) = run(&supervisor, &source(42)).await;
    assert!(result.ok);
    assert_eq!(dumps(&events)[0]["v"], 42);

    supervisor.close("rust-script").await;
    let (result, events) = run(&supervisor, &source(43)).await;
    assert!(result.ok);
    assert_eq!(dumps(&events)[0]["v"], 43);
}

#[tokio::test]
async fn desktop_rust_cache_reuses_values_in_one_open_tab() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("saved.rs");
    let supervisor = Supervisor::with_data_dir(data.path().to_owned()).with_session_rust_cache();
    let run_tab = |id, initial, expected| {
        let supervisor = &supervisor;
        let path = &path;
        async move {
            let source = format!(r#"codingpad::cache("answer", || {initial})"#);
            let mut events = Vec::new();
            let result = supervisor
                .run_language_at(id, &source, Language::Rust, path.to_str(), |event| {
                    events.push(event)
                })
                .await
                .unwrap();
            assert!(result.ok, "{events:?}");
            assert_eq!(dumps(&events)[0]["v"], expected);
        }
    };
    run_tab("file-first-tab", 41, 41).await;
    run_tab("file-first-tab", 42, 41).await;
    supervisor.reset("file-first-tab").await.unwrap();
    run_tab("file-first-tab", 43, 43).await;
    supervisor.close("file-first-tab").await;
    run_tab("file-reopened-tab", 42, 42).await;
    assert!(!data.path().join("rust-cache").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cooperative_stop_keeps_the_cache_and_does_not_mask_an_independent_exit() {
    use std::sync::Arc;
    use tokio::{
        sync::Notify,
        time::{timeout, Duration},
    };

    let supervisor = Arc::new(Supervisor::default());
    // The second fixture observes the file directly so its exit represents a
    // program completing on its own without acknowledging cancellation.
    let independent = r#"
        std::path::PathBuf::from(std::env::var_os("CODINGPAD_RUST_CANCEL").unwrap()).is_file()
    "#;
    for (condition, stopped) in [("codingpad::is_cancelled()", true), (independent, false)] {
        let source = format!(
            r#"
            codingpad::cache("answer", || 42);
            println!("ready");
            while !({condition}) {{
                std::thread::sleep(std::time::Duration::from_millis(10));
            }}
        "#
        );
        let ready = Arc::new(Notify::new());
        let running = tokio::spawn({
            let supervisor = supervisor.clone();
            let ready = ready.clone();
            async move {
                supervisor
                    .run_language("rust-script", &source, Language::Rust, move |event| {
                        if event.method == "stream" && event.params["text"] == "ready\n" {
                            ready.notify_one();
                        }
                    })
                    .await
                    .unwrap()
            }
        });
        timeout(Duration::from_secs(60), ready.notified())
            .await
            .unwrap();
        supervisor.stop("rust-script").await;
        let result = running.await.unwrap();
        assert_eq!(result.stopped, stopped);
        assert_eq!(result.ok, !stopped);
        assert!(!result.cache_cleared);
        let (result, events) = run(&supervisor, r#"codingpad::cache("answer", || 43)"#).await;
        assert!(result.ok);
        assert_eq!(dumps(&events)[0]["v"], 42);
    }
}

#[tokio::test]
async fn cargo_frontmatter_resolves_crates_and_preserves_editor_positions() {
    let supervisor = Supervisor::default();
    let source = concat!(
        "#!/usr/bin/env -S cargo +nightly -Zscript\r\n",
        "----cargo\r\n",
        "# café\r\n",
        "[package]\r\nedition = \"2021\"\r\n",
        "[dependencies]\r\nserde_json = \"1\"\r\n",
        "----\r\n",
        "fn main() { println!(\"{}\", serde_json::json!(42)); }\r\n",
    );
    let (result, events) = run(&supervisor, source).await;
    assert!(result.ok, "{events:?}");
    assert!(events
        .iter()
        .any(|event| event.method == "stream" && event.params["text"] == "42\n"));

    let bad = source.replace("serde_json::json!(42)", "missing()");
    let (result, events) = run(&supervisor, &bad).await;
    assert!(!result.ok);
    let diagnostics = &events
        .iter()
        .find(|event| event.method == "diagnostics")
        .unwrap()
        .params["diagnostics"];
    assert_eq!(diagnostics[0]["line"], 9);
    assert_eq!(diagnostics[0]["column"], 28);
}

#[tokio::test]
async fn relative_frontmatter_dependencies_resolve_from_the_saved_script() {
    let base = tempfile::tempdir().unwrap();
    let dependency = base.path().join("local-crate");
    std::fs::create_dir_all(dependency.join("src")).unwrap();
    std::fs::write(
        dependency.join("Cargo.toml"),
        "[package]\nname = \"local-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"",
    )
    .unwrap();
    std::fs::write(
        dependency.join("src/lib.rs"),
        "pub fn answer() -> i32 { 42 }",
    )
    .unwrap();
    let supervisor = Supervisor::default();
    let source = "---cargo\n[dependencies]\nlocal-crate = { path = \"local-crate\" }\n---\nlocal_crate::answer()";
    let script = base.path().join("saved.rs");
    let mut events = Vec::new();
    let result = supervisor
        .run_language_at(
            "relative",
            source,
            Language::Rust,
            script.to_str(),
            |event| events.push(event),
        )
        .await
        .unwrap();
    assert!(result.ok, "{events:?}");
    assert_eq!(dumps(&events)[0]["v"], 42);

    let source = source.replace("path = \"local-crate\"", "path = \"missing-crate\"");
    let mut events = Vec::new();
    let result = supervisor
        .run_language_at(
            "relative",
            &source,
            Language::Rust,
            script.to_str(),
            |event| events.push(event),
        )
        .await
        .unwrap();
    assert!(!result.ok);
    let error = dumps(&events)[0];
    assert_eq!(error["type"], "Build error");
    assert!(error["message"].as_str().unwrap().contains("missing-crate"));
}
