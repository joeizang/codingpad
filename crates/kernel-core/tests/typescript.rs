use codingpad_kernel_core::{KernelEvent, Language, Supervisor};
use serde_json::Value;
use std::time::Duration;
use tokio::{sync::oneshot, time::timeout};

#[tokio::test]
async fn typescript_run_streams_await_and_dumps_the_editor_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("scratch #1.ts");
    std::fs::write(&script, "throw new Error('saved copy ran')").unwrap();
    std::fs::write(
        directory.path().join("values.ts"),
        "export const count = 20;",
    )
    .unwrap();

    let saved_path = script.to_str().unwrap();
    // Windows may normalize the drive letter while resolving file: URLs.
    let drive_lowercase;
    let saved_path = if cfg!(windows) && saved_path.as_bytes().get(1) == Some(&b':') {
        drive_lowercase = saved_path[..1].to_ascii_lowercase() + &saved_path[1..];
        drive_lowercase.as_str()
    } else {
        saved_path
    };
    let supervisor = Supervisor::default();
    let mut events = Vec::<KernelEvent>::new();
    let source = r#"
import { dump } from "codingpad";
import { count } from "./values";
const wrongType: number = "still runs";
console.log("hello %s 🌍", wrongType);
console.warn("warning");
process.stdout.write("direct output");
import { writeSync } from "node:fs";
writeSync(1, "raw fd 1\n");
writeSync(2, "raw fd 2\n");
dump({ name: "Ada", values: [count, null] });
const answer = async () => 42;
await answer() // final expression
"#;
    let result = timeout(
        Duration::from_secs(10),
        supervisor.run_language_at(
            "scratch",
            source,
            Language::Typescript,
            Some(saved_path),
            |event| events.push(event),
        ),
    )
    .await
    .expect("Bun run timed out");
    let result = match result {
        Err(message)
            if message.starts_with("Bun was not found.") && std::env::var_os("CI").is_none() =>
        {
            assert!(message.contains("Install Bun"));
            eprintln!("Install Bun to exercise TypeScript execution");
            return;
        }
        result => result.expect("Bun kernel failed"),
    };
    assert!(result.ok, "{events:?}");
    let summary: Vec<_> = events.iter().map(|event| event.method.as_str()).collect();
    assert_eq!(
        summary,
        ["stream", "stream", "stream", "dump", "dump", "run_finished"]
    );
    assert_eq!(events[0].params["text"], "hello still runs 🌍\n");
    assert_eq!(events[0].params["stream"], "stdout");
    assert_eq!(events[1].params["stream"], "stderr");
    assert_eq!(events[2].params["text"], "direct output");
    let explicit = &events[3].params["node"];
    assert_eq!(explicit["fields"][0]["value"]["v"], "Ada");
    assert_eq!(explicit["fields"][1]["value"]["items"][0]["v"], 20);
    assert_eq!(events[4].params["node"]["v"], 42);
    assert_eq!(
        std::fs::read_to_string(&script).unwrap(),
        "throw new Error('saved copy ran')"
    );

    for (source, expected) in [
        ("const x = ;", None),
        ("throw new Error('boom');", None),
        (
            "import { dump as show } from 'codingpad';\nshow('explicit only');",
            Some(Value::from("explicit only")),
        ),
        ("let n = 41;\n(++n)", Some(Value::from(42))),
        ("'a final string'", Some(Value::from("a final string"))),
    ] {
        events.clear();
        let result = timeout(
            Duration::from_secs(10),
            supervisor.run_language("scratch", source, Language::Typescript, |event| {
                events.push(event)
            }),
        )
        .await
        .expect("Bun rerun timed out")
        .expect("Bun kernel failed");
        assert_eq!(result.ok, expected.is_some(), "{source} {events:?}");
        let dumps: Vec<_> = events
            .iter()
            .filter(|event| event.method == "dump")
            .map(|event| event.params["node"]["v"].clone())
            .collect();
        assert_eq!(dumps, expected.into_iter().collect::<Vec<_>>(), "{source}");
    }
}

#[tokio::test]
async fn typescript_stop_preserves_kernel_and_clears_script_state() {
    let supervisor = Supervisor::default();
    let mut events = Vec::new();
    let prior = supervisor
        .run_language(
            "scratch",
            "globalThis.prior = true; [process.pid, import.meta.dir, process.cwd(), await Bun.file('./run.ts').exists(), await Bun.file('./runtime.ts').exists()]",
            Language::Typescript,
            |event| events.push(event),
        )
        .await;
    let prior = match prior {
        Err(message)
            if message.starts_with("Bun was not found.") && std::env::var_os("CI").is_none() =>
        {
            return;
        }
        result => result.expect("Bun kernel failed"),
    };
    assert!(prior.ok, "{events:?}");
    let scratch = &events[0].params["node"]["items"];
    let pid = scratch[0]["v"].clone();
    assert_eq!(scratch[1]["v"], scratch[2]["v"]);
    assert_eq!(scratch[3]["v"], false);
    assert_eq!(scratch[4]["v"], false);
    assert!(std::path::Path::new(scratch[2]["v"].as_str().unwrap()).ends_with("scratch"));
    assert_ne!(
        scratch[2]["v"].as_str().unwrap(),
        std::env::current_dir().unwrap().to_str().unwrap()
    );

    for pending in ["while (true) {}", "await new Promise(() => {});"] {
        let (started, wait_for_start) = oneshot::channel();
        let mut started = Some(started);
        let source = format!("console.log('waiting'); {pending}");
        let mut output = Vec::new();
        let run = supervisor.run_language("scratch", &source, Language::Typescript, |event| {
            if event.params["text"] == "waiting\n" {
                started.take().unwrap().send(()).unwrap();
            }
            output.push(event);
        });
        let stop = async {
            wait_for_start.await.unwrap();
            supervisor.stop("scratch").await;
        };
        let (result, ()) = timeout(Duration::from_secs(10), async { tokio::join!(run, stop) })
            .await
            .expect("Bun Stop timed out");
        let result = result.expect("Bun kernel failed");
        assert!(result.stopped && !result.cache_cleared, "{result:?}");
        assert_eq!(output.first().unwrap().params["text"], "waiting\n");
        assert_eq!(output.last().unwrap().method, "run_finished");
    }

    events.clear();
    let result = supervisor
        .run_language(
            "scratch",
            "[process.pid, globalThis.prior === undefined, new Date(0), new Map([[1, 2]]), new Set([3]), Promise.resolve(4), new (class Example { value = 5 })()]",
            Language::Typescript,
            |event| events.push(event),
        )
        .await
        .unwrap();
    assert!(result.ok, "{events:?}");
    let items = &events[0].params["node"]["items"];
    assert_eq!(items[0]["v"], pid);
    assert_eq!(items[1]["v"], true);
    for (offset, label) in [(2, "1970"), (3, "Map"), (4, "Set"), (5, "Promise")] {
        assert!(
            items[offset]["v"].as_str().unwrap().contains(label),
            "{items}"
        );
    }
    assert_eq!(items[6]["type"], "Example");
    assert_eq!(items[6]["fields"][0]["value"]["v"], 5);
}
