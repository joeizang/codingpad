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
            .filter(|event| event.method == "dump" && event.params["node"]["kind"] != "error")
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
    let script_dir = scratch[1]["v"].as_str().unwrap();
    let working_dir = scratch[2]["v"].as_str().unwrap();
    if cfg!(windows) {
        assert!(script_dir.eq_ignore_ascii_case(working_dir));
    } else {
        assert_eq!(script_dir, working_dir);
    }
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

#[tokio::test]
async fn typescript_cache_survives_reruns_and_reset_clears_it() {
    let supervisor = Supervisor::default();
    let mut events = Vec::new();
    let probe = supervisor
        .run_language("cache", "1", Language::Typescript, |_| {})
        .await;
    if let Err(message) = probe {
        if message.starts_with("Bun was not found.") && std::env::var_os("CI").is_none() {
            eprintln!("Install Bun to exercise TypeScript execution");
            return;
        }
        panic!("Bun kernel failed: {message}");
    }

    macro_rules! run {
        ($source:expr) => {
            timeout(
                Duration::from_secs(10),
                supervisor.run_language("cache", $source, Language::Typescript, |event| {
                    events.push(event)
                }),
            )
            .await
        };
    }

    let result = run!(
        r#"
import { cache } from "codingpad";
[cache("value", () => 42), cache("value", () => 99)]
"#
    )
    .expect("Bun run timed out")
    .expect("Bun kernel failed");
    assert!(result.ok, "{events:?}");
    let items = &events[0].params["node"]["items"];
    assert_eq!(items[0]["v"], 42);
    assert_eq!(items[1]["v"], 42);

    // A rerun reads the value the previous worker stored in the kernel.
    events.clear();
    let result = run!("import { cache } from 'codingpad';\n[cache('value', () => 7)]")
        .expect("Bun rerun timed out")
        .expect("Bun kernel failed");
    assert!(result.ok, "{events:?}");
    assert_eq!(events[0].params["node"]["items"][0]["v"], 42);

    // Cached undefined is a hit, and failed factories are retried.
    events.clear();
    let result = run!(
        "import { cache } from 'codingpad';\n[cache('none', () => undefined), cache('none', () => 7)]"
    )
    .expect("Bun run timed out")
    .expect("Bun kernel failed");
    assert!(result.ok, "{events:?}");
    assert_eq!(events[0].params["node"]["items"][1]["v"], "undefined");

    events.clear();
    let failed =
        run!("import { cache } from 'codingpad';\ncache('retry', () => { throw new Error('no') })")
            .expect("Bun run timed out")
            .expect("Bun kernel failed");
    assert!(!failed.ok, "{events:?}");
    events.clear();
    let result = run!("import { cache } from 'codingpad';\n[cache('retry', () => 7)]")
        .expect("Bun rerun timed out")
        .expect("Bun kernel failed");
    assert!(result.ok, "{events:?}");
    assert_eq!(events[0].params["node"]["items"][0]["v"], 7);

    // A miss is cloned too, so the first run already behaves like a rerun and
    // never returns a live class instance that later becomes a plain object.
    events.clear();
    let result = run!(
        r#"
import { cache } from "codingpad";
class Point { constructor(x) { this.x = x; } norm() { return this.x; } }
const point = cache("point", () => new Point(1));
[typeof point.norm, point.x, Object.getPrototypeOf(point) === Object.prototype]
"#
    )
    .expect("Bun run timed out")
    .expect("Bun kernel failed");
    assert!(result.ok, "{events:?}");
    let items = &events[0].params["node"]["items"];
    assert_eq!(items[0]["v"], "undefined");
    assert_eq!(items[1]["v"], 1);
    assert_eq!(items[2]["v"], true);

    events.clear();
    let result = run!(
        "import { cache } from 'codingpad';\nconst point = cache('point', () => { throw new Error('factory ran again') });\n[typeof point.norm, point.x]"
    )
    .expect("Bun rerun timed out")
    .expect("Bun kernel failed");
    assert!(result.ok, "{events:?}");
    assert_eq!(events[0].params["node"]["items"][0]["v"], "undefined");
    assert_eq!(events[0].params["node"]["items"][1]["v"], 1);

    // Async factories fail clearly and their rejected promise is handled.
    events.clear();
    let result = run!(
        "import { cache } from 'codingpad';\ncache('async', () => Promise.reject(new Error('late')))"
    )
    .expect("Bun run timed out")
    .expect("Bun kernel failed");
    assert!(!result.ok, "{events:?}");
    let reported = events.iter().any(|event| {
        event.params["text"]
            .as_str()
            .is_some_and(|text| text.contains("must return synchronously"))
            || event.params["node"]["message"]
                .as_str()
                .is_some_and(|text| text.contains("must return synchronously"))
    });
    assert!(reported, "{events:?}");

    // Cooperative Stop keeps the kernel and its cached values.
    let (started, wait_for_start) = oneshot::channel();
    let mut started = Some(started);
    let pending = supervisor.run_language(
        "cache",
        "import { cache } from 'codingpad';\ncache('stopped', () => 'kept'); console.log('waiting'); while (true) {}",
        Language::Typescript,
        move |event| {
            if event.params["text"] == "waiting\n" {
                started.take().unwrap().send(()).unwrap();
            }
        },
    );
    let stop = async {
        wait_for_start.await.unwrap();
        supervisor.stop("cache").await;
    };
    let (result, ()) = timeout(Duration::from_secs(10), async {
        tokio::join!(pending, stop)
    })
    .await
    .expect("Bun Stop timed out");
    let result = result.expect("Bun kernel failed");
    assert!(result.stopped && !result.cache_cleared, "{result:?}");

    events.clear();
    let result = run!("import { cache } from 'codingpad';\n[cache('stopped', () => 'lost')]")
        .expect("Bun rerun timed out")
        .expect("Bun kernel failed");
    assert!(result.ok, "{events:?}");
    assert_eq!(events[0].params["node"]["items"][0]["v"], "kept");

    // Reset discards the warm kernel, so the next run recomputes every value.
    supervisor.reset("cache").await.unwrap();
    events.clear();
    let result = run!("import { cache } from 'codingpad';\n[cache('value', () => 7)]")
        .expect("Bun rerun timed out")
        .expect("Bun kernel failed");
    assert!(result.ok, "{events:?}");
    assert_eq!(events[0].params["node"]["items"][0]["v"], 7);
}

async fn run_dump_script(supervisor: &Supervisor, source: &str) -> Option<Vec<KernelEvent>> {
    let mut events = Vec::new();
    let result = timeout(
        Duration::from_secs(10),
        supervisor.run_language("dump", source, Language::Typescript, |event| {
            events.push(event)
        }),
    )
    .await
    .expect("Bun run timed out");
    match result {
        Err(message)
            if message.starts_with("Bun was not found.") && std::env::var_os("CI").is_none() =>
        {
            None
        }
        result => {
            assert!(result.expect("Bun kernel failed").ok, "{events:?}");
            Some(events)
        }
    }
}

fn dump_nodes(events: &[KernelEvent]) -> impl Iterator<Item = &Value> {
    events
        .iter()
        .filter(|event| event.method == "dump")
        .map(|event| rendered_dump(events, &event.params["node"]))
}

fn rendered_dump<'a>(events: &'a [KernelEvent], node: &'a Value) -> &'a Value {
    events
        .iter()
        .rev()
        .find(|event| event.method == "dump_update" && event.params["nodeId"] == node["id"])
        .map_or(node, |update| &update.params["node"])
}

fn handle(node: &Value) -> &str {
    node["handle"].as_str().unwrap()
}

#[tokio::test]
async fn typescript_expands_lazy_nodes_and_pages_large_arrays() {
    let supervisor = Supervisor::default();
    let Some(events) = run_dump_script(
        &supervisor,
        r#"
import { dump } from "codingpad";
const root = { next: null };
root.next = root;
dump(root);
let nested = { value: 42, get broken() { throw new Error("getter ran"); } };
for (let i = 0; i < 9; ++i) nested = [nested];
dump({ get broken() { throw new Error("getter ran"); } });
dump(nested);
dump(Array.from({ length: 250 }, (_, n) => n));
const records = Array.from({ length: 100_000 }, (_, n) => ({ n, tags: [n], full: "x".repeat(1_001) }));
records.at(-1).late = "included in export";
dump(records);
"#,
    )
    .await
    else {
        return;
    };
    let nodes: Vec<_> = dump_nodes(&events).collect();
    assert_eq!(nodes[0]["fields"][0]["value"]["v"], "[Circular]");
    assert_eq!(nodes[1]["fields"][0]["value"]["v"], "[Accessor]");
    let mut lazy = nodes[2];
    for _ in 0..6 {
        lazy = &lazy["items"][0];
    }
    assert_eq!(lazy["kind"], "lazy");
    let expanded = supervisor
        .expand("dump", handle(lazy), None, None)
        .await
        .unwrap()
        .node;
    assert_eq!(expanded["kind"], "list");

    let list = nodes[3];
    assert_eq!(list["next"], "100");
    let tail = supervisor
        .expand("dump", handle(list), Some("200"), None)
        .await
        .unwrap()
        .node;
    assert_eq!(tail["items"][0]["v"], 200);
    assert_eq!(tail["items"].as_array().unwrap().len(), 50);
    assert!(tail["next"].is_null());

    let table = nodes[4];
    assert_eq!(table["kind"], "table");
    assert_eq!(table["total"], 100_000);
    assert_eq!(table["rows"].as_array().unwrap().len(), 100);
    assert_eq!(table["rows"][99][0]["v"], 99);
    assert_eq!(table["rows"][99][1]["kind"], "lazy");
    let page = supervisor
        .expand("dump", handle(table), Some("99800"), None)
        .await
        .unwrap()
        .node;
    assert_eq!(page["rows"][0][0]["v"], 99_800);
    let exported = supervisor
        .export_page("dump", handle(table), Some("99800"), 1)
        .await
        .unwrap();
    assert_eq!(exported.rows[0][2].as_str().unwrap().len(), 1_001);
    assert_eq!(exported.next.as_deref(), Some("99801"));
    assert_eq!(exported.columns, ["n", "tags", "full", "late"]);

    let new_events = run_dump_script(&supervisor, "[{ x: 1 }]").await.unwrap();
    let new_handle = handle(dump_nodes(&new_events).next().unwrap());
    assert!(supervisor
        .expand("dump", handle(table), None, None)
        .await
        .unwrap_err()
        .contains("Run the script again"));
    supervisor.reset("dump").await.unwrap();
    assert!(supervisor
        .expand("dump", new_handle, None, None)
        .await
        .is_err());
}

#[tokio::test]
async fn typescript_pulls_iterables_one_page_at_a_time() {
    let supervisor = Supervisor::default();
    let directory = tempfile::tempdir().unwrap();
    let cleanup = serde_json::to_string(&directory.path().join("closed")).unwrap();
    let source = format!(
        r#"
import {{ dump }} from "codingpad";
import {{ writeFileSync }} from "node:fs";
let reads = 0;
async function* endless() {{
  try {{
    while (true) {{
      await Promise.resolve();
      yield {{ n: ++reads }};
    }}
  }} finally {{
    writeFileSync({cleanup}, String(reads));
  }}
}}
function* finite() {{
  for (let i = 0; i < 100; ++i) yield i;
  throw new Error("iterator failed");
}}
dump(endless());
dump(finite());
"#
    );
    let Some(events) = run_dump_script(&supervisor, &source).await else {
        return;
    };
    let nodes: Vec<_> = dump_nodes(&events).collect();
    let endless = nodes
        .iter()
        .find(|node| node["columns"][0]["name"] == "n")
        .unwrap();
    let finite = nodes
        .iter()
        .find(|node| node["columns"][0]["name"] == "Value")
        .unwrap();
    assert_eq!(endless["rows"].as_array().unwrap().len(), 100);
    assert_eq!(endless["rows"][99][0]["v"], 100);
    assert_eq!(endless["totalKnown"], false);
    assert_eq!(endless["total"], 101);
    for (cursor, last) in [("100", 200), ("0", 100), ("100", 200), ("200", 300)] {
        let page = supervisor
            .expand("dump", handle(endless), Some(cursor), None)
            .await
            .unwrap()
            .node;
        assert_eq!(page["rows"][99][0]["v"], last);
        assert_eq!(page["totalKnown"], false);
    }
    assert!(supervisor
        .expand("dump", handle(endless), Some("400"), None)
        .await
        .unwrap_err()
        .contains("previous page"));
    let last = supervisor
        .expand("dump", handle(finite), Some("100"), None)
        .await
        .unwrap()
        .node;
    assert_eq!(last["totalKnown"], true);
    assert_eq!(last["total"], 101);
    assert_eq!(last["rows"][0][0]["kind"], "error");
    assert_eq!(last["rows"][0][0]["message"], "iterator failed");
    assert_eq!(last["rows"][0][0]["traceback"][0]["script"], true);

    let events = run_dump_script(&supervisor, &format!("await Bun.file({cleanup}).text()"))
        .await
        .unwrap();
    assert_eq!(dump_nodes(&events).next().unwrap()["v"], "300");
}

#[tokio::test]
async fn typescript_renders_images_html_and_live_updates() {
    let supervisor = Supervisor::default();
    let Some(events) = run_dump_script(
        &supervisor,
        r#"
import { dump, html, image, live } from "codingpad";
dump(html("<b>ready</b>"));
dump(image(Uint8Array.of(137, 80, 78, 71, 13, 10, 26, 10)));
const status = live("starting");
dump(status);
await Promise.resolve();
status.value = "ready";
"#,
    )
    .await
    else {
        return;
    };
    let nodes: Vec<_> = dump_nodes(&events).collect();
    assert_eq!(nodes[0]["kind"], "html");
    assert_eq!(nodes[0]["html"], "<b>ready</b>");
    assert_eq!(nodes[1]["kind"], "image");
    assert_eq!(nodes[1]["src"], "data:image/png;base64,iVBORw0KGgo=");
    assert_eq!(nodes[2]["kind"], "live");
    assert_eq!(nodes[2]["node"]["v"], "starting");
    let update = events
        .iter()
        .find(|event| event.method == "dump_update")
        .unwrap();
    assert_eq!(update.params["nodeId"], nodes[2]["node"]["id"]);
    assert_eq!(update.params["node"]["v"], "ready");
    assert_eq!(events.last().unwrap().method, "run_finished");
}

#[tokio::test]
async fn typescript_errors_point_to_the_editor_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("saved #1.ts");
    let script = path.to_str().unwrap();
    let supervisor = Supervisor::default();
    for (source, line, column, message) in [
        ("// first\nconst answer = ;", 2, 16, "Unexpected"),
        (
            "const emoji = '💡';\n(() => { throw new Error('boom', { cause: new Error('inner') }); })()",
            2,
            20,
            "boom",
        ),
        (
            "const first = true;\n(() => {\n  throw new Error('multiline');\n})()",
            3,
            13,
            "multiline",
        ),
    ] {
        let mut events = Vec::new();
        let result = supervisor
            .run_language_at("dump", source, Language::Typescript, Some(script), |event| {
                events.push(event)
            })
            .await;
        match result {
            Err(message) if message.starts_with("Bun was not found.") && std::env::var_os("CI").is_none() => return,
            result => assert!(!result.unwrap().ok, "{events:?}"),
        }
        let node = dump_nodes(&events).next().unwrap();
        assert_eq!(node["kind"], "error");
        assert!(node["message"].as_str().unwrap().contains(message));
        let frame = &node["traceback"][0];
        assert_eq!(frame["script"], true, "{node}");
        assert_eq!(frame["line"], line, "{node}");
        assert_eq!(frame["column"], column, "{node}");
        assert_eq!(frame["text"], source.lines().nth(line - 1).unwrap());
        if message == "boom" {
            assert_eq!(node["causes"][0]["message"], "inner");
        }
    }
}

#[tokio::test]
async fn typescript_iterable_dumps_keep_results_in_call_order() {
    let supervisor = Supervisor::default();
    let Some(events) = run_dump_script(
        &supervisor,
        r#"
import { dump } from "codingpad";
function* syncRows() { yield 1; }
async function* asyncRows() {
  await Bun.sleep(10);
  yield 2;
}
dump(syncRows());
dump("after sync");
console.log("stdout");
dump(asyncRows());
dump("after async");
console.warn("stderr");
dump(syncRows());
"#,
    )
    .await
    else {
        return;
    };
    let results: Vec<_> = events
        .iter()
        .filter_map(|event| match event.method.as_str() {
            "dump" => {
                let node = rendered_dump(&events, &event.params["node"]);
                if node["kind"] == "table" {
                    Some(node["rows"][0][0]["v"].to_string())
                } else {
                    Some(node["v"].as_str().unwrap().to_owned())
                }
            }
            "stream" => Some(event.params["text"].as_str().unwrap().trim_end().to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        [
            "1",
            "after sync",
            "stdout",
            "2",
            "after async",
            "stderr",
            "1"
        ]
    );
}
