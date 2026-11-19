use codingpad_kernel_core::{KernelEvent, Language, Supervisor};
use serde_json::Value;
use std::time::Duration;
use tokio::time::timeout;

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

    let supervisor = Supervisor::default();
    let mut events = Vec::<KernelEvent>::new();
    let source = r#"
import { dump } from "codingpad";
import { count } from "./values";
const wrongType: number = "still runs";
console.log("hello %s 🌍", wrongType);
console.warn("warning");
process.stdout.write("direct output");
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
            script.to_str(),
            |event| events.push(event),
        ),
    )
    .await
    .expect("Bun run timed out");
    let result = match result {
        Err(message) if message.starts_with("Bun was not found.") => {
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
