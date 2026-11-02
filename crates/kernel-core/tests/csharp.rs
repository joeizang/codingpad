use codingpad_kernel_core::{KernelEvent, Language, Supervisor};
use serde_json::Value;
use std::time::Duration;
use tokio::time::timeout;

async fn run(supervisor: &Supervisor, id: &str, source: &str, ok: bool) -> Vec<KernelEvent> {
    let mut events = Vec::new();
    let result = timeout(
        Duration::from_secs(30),
        supervisor.run_language(id, source, Language::Csharp, |event| events.push(event)),
    )
    .await
    .expect("C# run timed out")
    .expect("C# kernel failed");
    assert_eq!(result.ok, ok, "{events:?}");
    assert_eq!(events.last().unwrap().method, "run_finished");
    events
}

fn dumps(events: &[KernelEvent]) -> Vec<&Value> {
    events
        .iter()
        .filter(|event| event.method == "dump")
        .map(|event| &event.params["node"])
        .collect()
}

#[tokio::test]
async fn csharp_acceptance_streams_await_and_auto_dump() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "one",
        r#"
Console.WriteLine("hello C# 🌍");
Console.Error.WriteLine("stderr");
"explicit".Dump();
await Task.Delay(5);
Enumerable.Range(1, 10).Sum();
"#,
        true,
    )
    .await;
    assert!(events.iter().any(|e| e.params["text"]
        .as_str()
        .is_some_and(|text| text.contains("hello C# 🌍"))));
    assert!(events.iter().any(|e| e.params["stream"] == "stderr"));
    let values = dumps(&events);
    assert_eq!(values.len(), 2);
    assert_eq!(values[0]["v"], "explicit");
    assert_eq!(values[1]["v"], 55);
    let events = run(&supervisor, "one", "await Task.FromResult(42);", true).await;
    assert_eq!(dumps(&events)[0]["v"], 42);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_errors_locations_and_warm_recovery() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "one",
        "Console.WriteLine(\"must not run\");\nmissing.Call();",
        false,
    )
    .await;
    assert!(!events.iter().any(|e| e.method == "stream"));
    let diagnostic = &events
        .iter()
        .find(|e| e.method == "diagnostics")
        .unwrap()
        .params["diagnostics"][0];
    assert_eq!(diagnostic["line"], 2);
    assert!(diagnostic["message"].as_str().unwrap().contains("CS0103"));
    assert_eq!(dumps(&events)[0]["traceback"][0]["line"], 2);
    let events = run(
        &supervisor,
        "one",
        "\nthrow new InvalidOperationException(\"boom\");",
        false,
    )
    .await;
    assert_eq!(dumps(&events)[0]["message"], "boom");
    assert!(dumps(&events)[0]["traceback"]
        .as_array()
        .unwrap()
        .iter()
        .all(|frame| frame["file"] == "script.cs" && frame["script"] == true));
    assert_eq!(dumps(&events)[0]["traceback"][0]["line"], 2);
    let events = run(&supervisor, "one", "42.Dump();", true).await;
    assert_eq!(dumps(&events).len(), 1);
    assert_eq!(dumps(&events)[0]["v"], 42);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_only_valid_value_returning_calls_are_auto_dumped() {
    let supervisor = Supervisor::default();
    for source in [
        "Console.WriteLine(42);",
        "var x = 42;",
        "await Task.Delay(1);",
        "void Print() {} Print();",
        "\"abc\".AsSpan();",
        "new int[] { 1, 2 }.AsSpan();",
        "static Value Next() => new(); Next(); ref struct Value {}",
    ] {
        assert!(
            dumps(&run(&supervisor, "one", source, true).await).is_empty(),
            "{source}"
        );
    }
    let events = run(
        &supervisor,
        "one",
        "dynamic d = 5; Console.WriteLine(d + 1);",
        true,
    )
    .await;
    assert!(dumps(&events).is_empty());
    let stdout: String = events
        .iter()
        .filter(|event| event.method == "stream" && event.params["stream"] == "stdout")
        .filter_map(|event| event.params["text"].as_str())
        .collect();
    assert_eq!(stdout.trim(), "6");
    assert_eq!(
        dumps(&run(&supervisor, "one", "42;", false).await)[0]["kind"],
        "error"
    );
    let events = run(
        &supervisor,
        "one",
        "int i = 0; int Next() => ++i; Next();",
        true,
    )
    .await;
    assert_eq!(dumps(&events)[0]["v"], 1);
    // A type declaration after the last top-level statement must not hide it.
    let events = run(
        &supervisor,
        "one",
        "Helper.Value(); class Helper { public static int Value() => 7; }",
        true,
    )
    .await;
    assert_eq!(dumps(&events)[0]["v"], 7);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_warm_process_fresh_collectible_assemblies_and_language_switch() {
    let supervisor = Supervisor::default();
    let first = run(&supervisor, "one", "Environment.ProcessId.Dump();", true).await;
    let second = run(&supervisor, "one", "Environment.ProcessId.Dump();", true).await;
    assert_eq!(dumps(&first)[0]["v"], dumps(&second)[0]["v"]);
    let source =
        "Counter.Next(); class Counter { static int value; public static int Next() => ++value; }";
    for _ in 0..2 {
        assert_eq!(
            dumps(&run(&supervisor, "one", source, true).await)[0]["v"],
            1
        );
    }
    let events = run(&supervisor, "one", "System.Runtime.Loader.AssemblyLoadContext.GetLoadContext(typeof(Local).Assembly)!.IsCollectible.Dump(); class Local {}", true).await;
    assert_eq!(dumps(&events)[0]["v"], true);
    let events = run(&supervisor, "one", "GC.Collect(); GC.WaitForPendingFinalizers(); GC.Collect(); System.Runtime.Loader.AssemblyLoadContext.All.Count(c => c.Name == \"script\");", true).await;
    assert_eq!(
        dumps(&events)[0]["v"],
        1,
        "Previous script assemblies should unload"
    );
    // Changing language on a saved scratch must replace its process.
    assert!(supervisor.run("one", "42", |_| {}).await.unwrap().ok);
    assert_eq!(
        dumps(&run(&supervisor, "one", "42.Dump();", true).await)[0]["v"],
        42
    );
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_no_statement_scripts_succeed_but_invalid_types_still_fail() {
    let supervisor = Supervisor::default();
    for source in [
        "",
        "  \n",
        "// comment only",
        "using System;",
        "class Only {}",
        "namespace Example { class Only {} }",
    ] {
        let events = run(&supervisor, "one", source, true).await;
        assert!(dumps(&events).is_empty(), "{source}");
        assert_eq!(
            events
                .iter()
                .find(|e| e.method == "diagnostics")
                .unwrap()
                .params["diagnostics"],
            serde_json::json!([])
        );
    }
    let events = run(
        &supervisor,
        "one",
        "class Invalid { Missing field; }",
        false,
    )
    .await;
    assert!(dumps(&events)[0]["message"]
        .as_str()
        .unwrap()
        .contains("CS0246"));
    // Explicit entry points were already supported and must keep executing.
    let events = run(
        &supervisor,
        "one",
        "class Entry { static void Main() { 42.Dump(); } }",
        true,
    )
    .await;
    assert_eq!(dumps(&events)[0]["v"], 42);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_scripts_cannot_reference_private_kernel_assemblies() {
    let supervisor = Supervisor::default();
    for source in [
        "Microsoft.CodeAnalysis.CSharp.SyntaxFactory.IdentifierName(\"x\");",
        "Microsoft.CodeAnalysis.DiagnosticSeverity.Error.Dump();",
    ] {
        let events = run(&supervisor, "one", source, false).await;
        assert_eq!(dumps(&events)[0]["type"], "Compile error");
    }
    // Framework references and the public helper still work.
    let events = run(
        &supervisor,
        "one",
        "Enumerable.Range(1, 3).Sum().Dump();",
        true,
    )
    .await;
    assert_eq!(dumps(&events)[0]["v"], 6);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_assets_are_shared_across_tabs_and_resets() {
    let supervisor = Supervisor::default();
    let source = "typeof(CodingPad.DumpExtensions).Assembly.Location.Dump();";
    let first = run(&supervisor, "one", source, true).await;
    let other = run(&supervisor, "two", source, true).await;
    assert_eq!(dumps(&first)[0]["v"], dumps(&other)[0]["v"]);
    supervisor.reset("one").await.unwrap();
    let restarted = run(&supervisor, "one", source, true).await;
    assert_eq!(dumps(&first)[0]["v"], dumps(&restarted)[0]["v"]);
    supervisor.shutdown().await;
}

fn field<'a>(node: &'a Value, name: &str) -> &'a Value {
    &node["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["name"] == name)
        .unwrap()["value"]
}

#[tokio::test]
async fn csharp_reflection_is_lazy_bounded_and_recovers_from_getter_errors() {
    let supervisor = Supervisor::default();
    let events = run(&supervisor, "tree", r#"
var item = new Item(); item.Self = item; item.Dump();
class Item {
    public int Number = 7;
    public Item? Self;
    public int Property => 42;
    public int Broken => throw new InvalidOperationException("getter failed", new Exception("inner"));
    public override string ToString() => throw new Exception("must not call ToString");
    public int this[int index] => throw new Exception("must not call indexer");
}
"#, true).await;
    let node = dumps(&events)[0];
    assert_eq!(node["kind"], "object");
    assert_eq!(field(node, "Number")["v"], 7);
    assert_eq!(field(node, "Self")["v"], "<cycle>");
    let property = field(node, "Property")["handle"].as_str().unwrap();
    assert_eq!(
        supervisor
            .expand("tree", property, None, None)
            .await
            .unwrap()
            .node["v"],
        42
    );
    let broken = field(node, "Broken")["handle"].as_str().unwrap();
    let error = supervisor
        .expand("tree", broken, None, None)
        .await
        .unwrap()
        .node;
    assert_eq!(error["type"], "InvalidOperationException");
    assert_eq!(error["message"], "getter failed");
    assert_eq!(error["causes"][0]["message"], "inner");
    assert!(error["traceback"]
        .as_array()
        .unwrap()
        .iter()
        .all(|frame| frame["file"] == "script.cs" && frame["script"] == true));
    assert!(error["traceback"]
        .as_array()
        .unwrap()
        .iter()
        .any(|frame| frame["script"] == true && frame["line"].as_u64().unwrap() > 0));
    run(&supervisor, "tree", "1.Dump();", true).await;
    assert!(supervisor
        .expand("tree", property, None, None)
        .await
        .unwrap_err()
        .contains("Run the script again"));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_endless_sequence_reads_only_requested_pages_and_disposes_on_rerun() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "seq",
        r#"
Sequence.Items().Dump();
static class Sequence {
    public static IEnumerable<int> Items() {
        try {
            for (int i = 0; ; i++) { Console.WriteLine($"read {i}"); yield return i; }
        } finally { Environment.SetEnvironmentVariable("CPAD_ITERATOR_DISPOSED", "yes"); }
    }
}
"#,
        true,
    )
    .await;
    let node = dumps(&events)[0];
    let handle = node["handle"].as_str().unwrap();
    assert_eq!(node["rows"].as_array().unwrap().len(), 100);
    assert_eq!(node["totalKnown"], false);
    assert_eq!(node["next"], "100");
    let output: String = events
        .iter()
        .filter(|event| event.method == "stream")
        .filter_map(|event| event.params["text"].as_str())
        .collect();
    assert!(output.contains("read 99\n"));
    assert!(!output.contains("read 100\n"));
    let page = supervisor
        .expand("seq", handle, Some("100"), None)
        .await
        .unwrap()
        .node;
    assert_eq!(page["rows"][0][0]["v"], 100);
    assert_eq!(page["rows"][99][0]["v"], 199);
    assert_eq!(page["next"], "200");
    let repeated = supervisor
        .expand("seq", handle, Some("100"), None)
        .await
        .unwrap()
        .node;
    assert_eq!(repeated["rows"][0][0]["v"], 100);
    assert_eq!(repeated["next"], "200");
    let events = run(
        &supervisor,
        "seq",
        "Environment.GetEnvironmentVariable(\"CPAD_ITERATOR_DISPOSED\").Dump();",
        true,
    )
    .await;
    assert_eq!(dumps(&events)[0]["v"], "yes");
    assert!(supervisor
        .expand("seq", handle, None, None)
        .await
        .unwrap_err()
        .contains("Run the script again"));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_sequence_records_nested_cells_and_iteration_errors() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "seq",
        r#"
Enumerable.Range(0, 102).Select(i => new { Name = $"row {i}", Values = new[] { i } }).Dump();
"#,
        true,
    )
    .await;
    let node = dumps(&events)[0];
    assert_eq!(node["columns"][0]["name"], "Name");
    assert_eq!(node["rows"][0][0]["v"], "row 0");
    assert_eq!(node["rows"][0][1]["kind"], "lazy");
    let cell = node["rows"][0][1]["handle"].as_str().unwrap();
    assert_eq!(
        supervisor
            .expand("seq", cell, None, None)
            .await
            .unwrap()
            .node["rows"][0][0]["v"],
        0
    );
    let page = supervisor
        .expand("seq", node["handle"].as_str().unwrap(), Some("100"), None)
        .await
        .unwrap()
        .node;
    assert_eq!(page["rows"].as_array().unwrap().len(), 2);
    assert_eq!(page["total"], 102);
    assert_eq!(page["totalKnown"], true);
    assert!(page["next"].is_null());
    let events = run(
        &supervisor,
        "seq",
        r#"
IEnumerable<int> Items() { yield return 1; throw new Exception("iteration failed"); }
Items().Dump();
"#,
        true,
    )
    .await;
    let node = dumps(&events)[0];
    assert_eq!(node["rows"][0][0]["v"], 1);
    assert_eq!(node["rows"][1][0]["message"], "iteration failed");
    assert_eq!(node["totalKnown"], true);
    supervisor.reset("seq").await.unwrap();
    assert!(supervisor
        .expand("seq", node["handle"].as_str().unwrap(), None, None)
        .await
        .is_err());
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_rich_values_and_live_updates_share_the_dump_protocol() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "rich",
        r#"
new HtmlValue("<b>Hello</b>").Dump();
new ImageValue(new byte[] { 1, 2, 3 }, "image/png", "plot.png").Dump();
var live = new LiveValue(1); live.Dump(); live.Value = 2;
"#,
        true,
    )
    .await;
    let nodes = dumps(&events);
    assert_eq!(nodes[0]["kind"], "html");
    assert_eq!(nodes[0]["html"], "<b>Hello</b>");
    assert_eq!(nodes[1]["src"], "data:image/png;base64,AQID");
    assert_eq!(nodes[2]["kind"], "live");
    assert_eq!(nodes[2]["node"]["v"], 1);
    let update = events
        .iter()
        .find(|event| event.method == "dump_update")
        .unwrap();
    assert_eq!(update.params["nodeId"], nodes[2]["node"]["id"]);
    assert_eq!(update.params["node"]["v"], 2);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_large_sequence_first_page_is_bounded_and_fast() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "large",
        r#"
var clock = System.Diagnostics.Stopwatch.StartNew();
int visited = 0;
Enumerable.Range(0, 100_000).Select(i => { visited++; return i; }).Dump();
clock.Stop(); visited.Dump(); clock.Elapsed.TotalMilliseconds.Dump();
"#,
        true,
    )
    .await;
    let nodes = dumps(&events);
    assert_eq!(nodes[0]["rows"].as_array().unwrap().len(), 100);
    assert_eq!(nodes[1]["v"], 100, "Dump must not traverse all 100k items");
    let milliseconds = nodes[2]["v"].as_f64().unwrap();
    assert!(milliseconds < 1000.0, "First page took {milliseconds} ms");
    println!("100k-item C# sequence first page: {milliseconds:.1} ms");
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_deep_trees_and_object_pages_remain_expandable() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "deep",
        r#"
var root = new Item(); var tail = root;
for (int i = 0; i < 12; i++) { tail.Next = new Item(); tail = tail.Next; }
root.Dump();
class Item { public Item? Next; }
"#,
        true,
    )
    .await;
    let mut node = dumps(&events)[0];
    for _ in 0..6 {
        node = field(node, "Next");
    }
    assert_eq!(node["kind"], "lazy");
    let expanded = supervisor
        .expand("deep", node["handle"].as_str().unwrap(), None, None)
        .await
        .unwrap()
        .node;
    assert_eq!(expanded["kind"], "object");
    assert_eq!(field(&expanded, "Next")["kind"], "object");
    let fields: String = (0..105)
        .map(|i| format!("public int F{i:03} = {i}; "))
        .collect();
    let source = format!("new Wide().Dump(); class Wide {{ {fields} }}");
    let events = run(&supervisor, "deep", &source, true).await;
    let node = dumps(&events)[0];
    assert_eq!(node["fields"].as_array().unwrap().len(), 100);
    let page = supervisor
        .expand(
            "deep",
            node["handle"].as_str().unwrap(),
            Some("100"),
            Some(1000),
        )
        .await
        .unwrap()
        .node;
    assert_eq!(page["fields"].as_array().unwrap().len(), 5);
    assert_eq!(field(&page, "F104")["v"], 104);
    assert!(page["next"].is_null());
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_rich_value_limits_and_late_output_do_not_break_the_next_run() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "limits",
        r#"
new ImageValue(new byte[4 * 1024 * 1024 + 1]).Dump();
new ImageValue(new byte[] { 1 }, "text/html").Dump();
new string('x', 20000).Dump();
new HtmlValue(new string('y', 200001)).Dump();
"#,
        true,
    )
    .await;
    let nodes = dumps(&events);
    assert_eq!(nodes[0]["kind"], "error");
    assert_eq!(nodes[1]["message"], "Unsupported image MIME type.");
    assert_eq!(nodes[2]["v"].as_str().unwrap().len(), 10000);
    assert_eq!(nodes[2]["truncated"], true);
    assert_eq!(nodes[3]["html"].as_str().unwrap().len(), 200000);
    run(&supervisor, "limits", r#"
var live = new LiveValue(0); live.Dump();
_ = Task.Run(async () => { await Task.Delay(100); live.Value = 99; Enumerable.Range(0, 10).Dump(); });
"#, true).await;
    let events = run(
        &supervisor,
        "limits",
        "await Task.Delay(250); 42.Dump();",
        true,
    )
    .await;
    assert_eq!(dumps(&events).len(), 1);
    assert_eq!(dumps(&events)[0]["v"], 42);
    assert!(!events.iter().any(|event| event.method == "dump_update"));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_repeated_dumps_and_live_updates_have_independent_sequence_views() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "views",
        r#"
var list = new List<int> { 1 }; list.Dump(); list.Add(2); list.Dump();
var large = Enumerable.Range(0, 101).ToList(); large.Dump(); large.Add(101); large.Dump();
var live = new LiveValue(list); live.Dump(); list.Add(3); live.Value = list;
"#,
        true,
    )
    .await;
    let nodes = dumps(&events);
    assert_eq!(nodes[0]["rows"].as_array().unwrap().len(), 1);
    assert_eq!(nodes[1]["rows"].as_array().unwrap().len(), 2);
    assert_ne!(nodes[0]["handle"], nodes[1]["handle"]);
    assert_ne!(nodes[2]["handle"], nodes[3]["handle"]);
    let page = supervisor
        .expand(
            "views",
            nodes[3]["handle"].as_str().unwrap(),
            Some("100"),
            None,
        )
        .await
        .unwrap()
        .node;
    assert_eq!(page["rows"][0][0]["v"], 100);
    assert_eq!(page["rows"][1][0]["v"], 101);
    assert_eq!(page["total"], 102);
    let update = events
        .iter()
        .find(|event| event.method == "dump_update")
        .unwrap();
    assert_eq!(update.params["nodeId"], nodes[4]["node"]["id"]);
    assert_eq!(update.params["node"]["rows"].as_array().unwrap().len(), 3);
    // Revisit the first dump without replacing its cached page with the later one.
    let original = supervisor
        .expand("views", nodes[0]["handle"].as_str().unwrap(), None, None)
        .await
        .unwrap()
        .node;
    assert_eq!(original["rows"].as_array().unwrap().len(), 1);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_property_caches_are_per_dump_and_lazy_getters_read_on_first_expansion() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "properties",
        r#"
var person = new Person(); var people = new List<Person> { person };
people.Dump(); person.Name = "B"; people.Dump();
person.Dump(); person.Name = "C"; person.Dump();
class Person { public string Name { get; set; } = "A"; }
"#,
        true,
    )
    .await;
    let nodes = dumps(&events);
    assert_eq!(nodes[0]["rows"][0][0]["v"], "A");
    assert_eq!(nodes[1]["rows"][0][0]["v"], "B");
    let first = field(nodes[2], "Name")["handle"].as_str().unwrap();
    let second = field(nodes[3], "Name")["handle"].as_str().unwrap();
    assert_ne!(first, second);
    // Object getters intentionally evaluate on expansion; an earlier table's
    // eager getter reads must not leak into either lazy object dump.
    assert_eq!(
        supervisor
            .expand("properties", first, None, None)
            .await
            .unwrap()
            .node["v"],
        "C"
    );
    assert_eq!(
        supervisor
            .expand("properties", second, None, None)
            .await
            .unwrap()
            .node["v"],
        "C"
    );
    let old_table = supervisor
        .expand(
            "properties",
            nodes[0]["handle"].as_str().unwrap(),
            None,
            None,
        )
        .await
        .unwrap()
        .node;
    assert_eq!(old_table["rows"][0][0]["v"], "A");
    let events = run(&supervisor, "properties", "var c = new Counter(); c.Dump(); c.Dump(); class Counter { private int count; public int Next => ++count; }", true).await;
    let nodes = dumps(&events);
    let first = field(nodes[0], "Next")["handle"].as_str().unwrap();
    let second = field(nodes[1], "Next")["handle"].as_str().unwrap();
    for (handle, expected) in [(first, 1), (first, 1), (second, 2)] {
        assert_eq!(
            supervisor
                .expand("properties", handle, None, None)
                .await
                .unwrap()
                .node["v"],
            expected
        );
    }
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_wide_tables_keep_scalar_cells_visible_after_the_node_budget() {
    let supervisor = Supervisor::default();
    let members: String = (0..16)
        .map(|i| format!("N{i:02} = i, S{i:02} = i.ToString(), "))
        .collect();
    let source = format!("Enumerable.Range(0, 100).Select(i => new {{ {members} }}).Dump();");
    let events = run(&supervisor, "wide", &source, true).await;
    let node = dumps(&events)[0];
    assert_eq!(node["columns"].as_array().unwrap().len(), 32);
    let rows = node["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 100);
    for (index, row) in rows.iter().enumerate() {
        for (column, cell) in row.as_array().unwrap().iter().enumerate() {
            if column < 16 {
                assert_eq!(cell["kind"], "primitive");
                assert_eq!(cell["v"], index);
            } else {
                assert_eq!(cell["kind"], "string");
                assert_eq!(cell["v"], index.to_string());
            }
            assert!(cell.get("handle").is_none());
        }
    }
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_hung_previews_can_be_stopped_reset_closed_or_shut_down() {
    let supervisor = Supervisor::default();
    for action in ["stop", "reset", "close", "shutdown"] {
        let iterator = matches!(action, "stop" | "close");
        let source = if iterator {
            "IEnumerable<int> Gen() { for (var i = 0; i < 100; i++) yield return i; while (true) { } } Gen().Dump(); Environment.ProcessId.Dump();"
        } else {
            "new Item().Dump(); Environment.ProcessId.Dump(); class Item { public int Value { get { Thread.Sleep(Timeout.Infinite); return 0; } } }"
        };
        let events = run(&supervisor, "hung", source, true).await;
        let nodes = dumps(&events);
        let handle = if iterator {
            &nodes[0]["handle"]
        } else {
            &field(nodes[0], "Value")["handle"]
        }
        .as_str()
        .unwrap();
        let expand = supervisor.expand(
            "hung",
            handle,
            if iterator { Some("100") } else { None },
            None,
        );
        tokio::pin!(expand);
        // Poll the actual request without dropping it when the wait expires.
        assert!(timeout(Duration::from_millis(100), &mut expand)
            .await
            .is_err());
        let cancel = async {
            match action {
                "stop" => supervisor.stop("hung").await,
                "reset" => supervisor.reset("hung").await.unwrap(),
                "close" => supervisor.close("hung").await,
                "shutdown" => supervisor.shutdown().await,
                _ => unreachable!(),
            }
        };
        let (result, ()) = timeout(Duration::from_secs(3), async {
            tokio::join!(expand, cancel)
        })
        .await
        .expect("Preview cancellation blocked");
        assert!(result.unwrap_err().contains("Run the script again"));
        let recovered = run(&supervisor, "hung", "Environment.ProcessId.Dump();", true).await;
        assert_ne!(
            dumps(&recovered)[0]["v"],
            nodes[1]["v"],
            "{action} must discard the hung process"
        );
    }
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_live_update_targets_match_complex_inner_node_ids() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "live",
        r#"
foreach (var value in new object[] {
    new ImageValue(new byte[] { 1, 2, 3 }),
    new[] { 1, 2 },
    new { Name = "Ada" },
    new InvalidOperationException("before")
}) { var live = new LiveValue(value); live.Dump(); live.Value = "after"; }
"#,
        true,
    )
    .await;
    let nodes = dumps(&events);
    let updates: Vec<_> = events
        .iter()
        .filter(|event| event.method == "dump_update")
        .collect();
    assert_eq!(nodes.len(), 4);
    assert_eq!(updates.len(), 4);
    for (node, update) in nodes.iter().zip(updates) {
        assert_eq!(update.params["nodeId"], node["node"]["id"]);
        assert_eq!(update.params["node"]["v"], "after");
    }
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_replaced_live_tables_dispose_iterators_and_expire_only_their_handles() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "release",
        r#"
Feed.Items(-1).Dump();
var live = new LiveValue(Feed.Items(0)); live.Dump();
for (int i = 1; i <= 30; i++) {
    live.Value = Feed.Items(i);
    if (Feed.Open != 2) throw new Exception($"Leaked iterators: {Feed.Open}");
}
Feed.Open.Dump(); Feed.Closed.Dump();
static class Feed {
    public static int Open, Closed;
    public static IEnumerable<int> Items(int value) {
        Open++;
        try { for (int i = 0; i < 200; i++) yield return value; }
        finally { Open--; Closed++; }
    }
}
"#,
        true,
    )
    .await;
    let nodes = dumps(&events);
    assert_eq!(nodes[2]["v"], 2);
    assert_eq!(
        nodes[3]["v"], 30,
        "Every replaced iterator must be disposed during the run"
    );
    let initial = nodes[1]["node"]["handle"].as_str().unwrap();
    assert!(supervisor
        .expand("release", initial, None, None)
        .await
        .unwrap_err()
        .contains("Run the script again"));
    let updates: Vec<_> = events
        .iter()
        .filter(|event| event.method == "dump_update")
        .collect();
    assert_eq!(updates.len(), 30);
    for update in &updates[..29] {
        let handle = update.params["node"]["handle"].as_str().unwrap();
        assert!(supervisor
            .expand("release", handle, None, None)
            .await
            .is_err());
    }
    let latest = updates[29].params["node"]["handle"].as_str().unwrap();
    let page = supervisor
        .expand("release", latest, Some("100"), None)
        .await
        .unwrap()
        .node;
    assert_eq!(page["rows"][0][0]["v"], 30);
    let independent = supervisor
        .expand(
            "release",
            nodes[0]["handle"].as_str().unwrap(),
            Some("100"),
            None,
        )
        .await
        .unwrap()
        .node;
    assert_eq!(independent["rows"][0][0]["v"], -1);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_replacing_a_live_parent_releases_nested_live_subscriptions() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "nested",
        r#"
var inner = new LiveValue(Feed.Items(0)); inner.Dump();
var outer = new LiveValue(inner); outer.Dump();
outer.Value = 42;
Feed.Open.Dump();
inner.Value = Feed.Items(1);
Feed.Open.Dump(); Feed.Closed.Dump();
static class Feed {
    public static int Open, Closed;
    public static IEnumerable<int> Items(int value) {
        Open++;
        try { for (int i = 0; i < 200; i++) yield return value; }
        finally { Open--; Closed++; }
    }
}
"#,
        true,
    )
    .await;
    let nodes = dumps(&events);
    assert_eq!(nodes[2]["v"], 1);
    assert_eq!(nodes[3]["v"], 1);
    assert_eq!(nodes[4]["v"], 2);
    let updates: Vec<_> = events
        .iter()
        .filter(|event| event.method == "dump_update")
        .collect();
    // The old nested subscription must not produce a third, invisible update.
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].params["nodeId"], nodes[1]["node"]["id"]);
    assert_eq!(updates[1].params["nodeId"], nodes[0]["node"]["id"]);
    let expired = nodes[1]["node"]["node"]["handle"].as_str().unwrap();
    assert!(supervisor
        .expand("nested", expired, None, None)
        .await
        .is_err());
    let latest = updates[1].params["node"]["handle"].as_str().unwrap();
    assert_eq!(
        supervisor
            .expand("nested", latest, Some("100"), None)
            .await
            .unwrap()
            .node["rows"][0][0]["v"],
        1
    );
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_live_replacement_preserves_sibling_handles_and_expires_lazy_children() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "siblings",
        r#"
var live = new LiveValue(new { Value = 7 });
new Holder { Live = live, Sibling = Enumerable.Range(0, 101).ToArray() }.Dump();
live.Value = 42;
class Holder { public LiveValue? Live; public int[]? Sibling; }
"#,
        true,
    )
    .await;
    let node = dumps(&events)[0];
    let old_property = field(&field(node, "Live")["node"], "Value")["handle"]
        .as_str()
        .unwrap();
    assert!(supervisor
        .expand("siblings", old_property, None, None)
        .await
        .is_err());
    let sibling = field(node, "Sibling")["handle"].as_str().unwrap();
    assert_eq!(
        supervisor
            .expand("siblings", sibling, Some("100"), None)
            .await
            .unwrap()
            .node["rows"][0][0]["v"],
        100
    );
    assert_eq!(
        supervisor
            .expand(
                "siblings",
                node["handle"].as_str().unwrap(),
                Some("0"),
                None
            )
            .await
            .unwrap()
            .node["kind"],
        "object"
    );
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_live_updates_from_iterator_cleanup_keep_the_newest_value() {
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        "cleanup",
        r#"
var live = new LiveValue(Feed.Items()); live.Dump();
Feed.OnClose = () => { Feed.OnClose = null; live.Value = 42; };
live.Value = Feed.Items();
Feed.Open.Dump();
static class Feed {
    public static int Open;
    public static Action? OnClose;
    public static IEnumerable<int> Items() {
        Open++;
        try { for (int i = 0; i < 200; i++) yield return i; }
        finally { Open--; OnClose?.Invoke(); }
    }
}
"#,
        true,
    )
    .await;
    let updates: Vec<_> = events
        .iter()
        .filter(|event| event.method == "dump_update")
        .collect();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].params["node"]["v"], 42);
    assert_eq!(dumps(&events)[1]["v"], 0);
    supervisor.shutdown().await;
}

async fn csharp_start_until_ready(
    supervisor: std::sync::Arc<Supervisor>,
    source: &'static str,
) -> tokio::task::JoinHandle<codingpad_kernel_core::RunResult> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut tx = Some(tx);
        supervisor
            .run_language("control", source, Language::Csharp, |event| {
                if event.method == "stream"
                    && event.params["text"]
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
    timeout(Duration::from_secs(30), rx).await.unwrap().unwrap();
    task
}

#[tokio::test]
async fn csharp_cooperative_stop_keeps_cache_and_process() {
    let supervisor = std::sync::Arc::new(Supervisor::default());
    let before = run(
        &supervisor,
        "control",
        "Cache(\"pid\", () => Environment.ProcessId).Dump();",
        true,
    )
    .await;
    for source in [
        "Console.WriteLine(\"ready\"); await Task.Delay(60000, Cancel);",
        "Console.WriteLine(\"ready\"); while (true) Cancel.ThrowIfCancellationRequested();",
    ] {
        let task = csharp_start_until_ready(supervisor.clone(), source).await;
        supervisor.stop("control").await;
        let result = timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert!(result.stopped && !result.ok && !result.cache_cleared);
        let after = run(
            &supervisor,
            "control",
            "Cache(\"pid\", () => -1).Dump(); Environment.ProcessId.Dump();",
            true,
        )
        .await;
        assert_eq!(dumps(&before)[0]["v"], dumps(&after)[0]["v"]);
        assert_eq!(dumps(&before)[0]["v"], dumps(&after)[1]["v"]);
    }
    supervisor.stop("control").await;
    run(
        &supervisor,
        "control",
        "Cancel.ThrowIfCancellationRequested();",
        true,
    )
    .await;
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_forced_stop_and_reset_clear_only_the_target_cache() {
    let supervisor = std::sync::Arc::new(Supervisor::default());
    let before = run(
        &supervisor,
        "control",
        "Cache(\"saved\", () => 42); Environment.ProcessId.Dump();",
        true,
    )
    .await;
    run(&supervisor, "other", "Cache(\"saved\", () => 99);", true).await;
    let task = csharp_start_until_ready(
        supervisor.clone(),
        "Console.WriteLine(\"ready\"); Thread.Sleep(60000);",
    )
    .await;
    supervisor.stop("control").await;
    let result = timeout(Duration::from_secs(6), task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.stopped && result.cache_cleared);
    let after = run(
        &supervisor,
        "control",
        "Cache(\"saved\", () => -1).Dump(); Environment.ProcessId.Dump();",
        true,
    )
    .await;
    assert_eq!(dumps(&after)[0]["v"], -1);
    assert_ne!(dumps(&before)[0]["v"], dumps(&after)[1]["v"]);
    supervisor.reset("control").await.unwrap();
    let reset = run(
        &supervisor,
        "control",
        "Cache(\"saved\", () => 7).Dump();",
        true,
    )
    .await;
    assert_eq!(dumps(&reset)[0]["v"], 7);
    let other = run(
        &supervisor,
        "other",
        "Cache(\"saved\", () => -1).Dump();",
        true,
    )
    .await;
    assert_eq!(dumps(&other)[0]["v"], 99);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_cache_reuses_safe_values_and_refreshes_script_graphs() {
    let supervisor = Supervisor::default();
    let source = r#"
Cache("safe", () => new List<int> { 42 }).Dump();
Cache<string?>("null", () => null).Dump();
Cache("direct", () => new Item()).GetType().Assembly.FullName.Dump();
Cache("nested", () => new List<object> { new Item() })[0].GetType().Assembly.FullName.Dump();
var mutated = Cache("mutated", () => new List<object>());
mutated.Count.Dump(); mutated.Add(new Item());
Cache("generic", () => new List<Item>()).GetType().AssemblyQualifiedName.Dump();
Cache("delegate", () => (Func<int>)Item.Read)().Dump();
Cache("weak", () => new WeakReference<object>(new Item()));
class Item { public static int Read() => 17; }
"#;
    let first = run(&supervisor, "cache", source, true).await;
    let second = run(&supervisor, "cache", source, true).await;
    assert_ne!(dumps(&first)[2]["v"], dumps(&second)[2]["v"]);
    assert_ne!(dumps(&first)[3]["v"], dumps(&second)[3]["v"]);
    assert_eq!(
        dumps(&second)[4]["v"],
        0,
        "Mutated cached containers must be rechecked"
    );
    assert_ne!(dumps(&first)[5]["v"], dumps(&second)[5]["v"]);
    assert_eq!(dumps(&second)[6]["v"], 17);
    let notes: Vec<_> = second
        .iter()
        .filter(|event| event.params["text"] == "cache refreshed: script types changed\n")
        .collect();
    assert_eq!(notes.len(), 6);
    let safe = run(
        &supervisor,
        "cache",
        r#"
Cache("safe", () => new List<int> { -1 })[0].Dump();
Cache<string?>("null", () => "lost").Dump();
try { Cache<int>("failed", () => throw new Exception("failure")); } catch {}
Cache("failed", () => 7).Dump();
Cache("thread", () => Task.Run(() => Cache("inner", () => 8)).Result).Dump();
"#,
        true,
    )
    .await;
    assert_eq!(dumps(&safe)[0]["v"], 42);
    assert_eq!(dumps(&safe)[1]["kind"], "null");
    assert_eq!(dumps(&safe)[2]["v"], 7);
    assert_eq!(dumps(&safe)[3]["v"], 8);
    let secret = run(&supervisor, "cache", "Secret(\"example\");", false).await;
    assert!(dumps(&secret)[0]["message"]
        .as_str()
        .unwrap()
        .contains("M3"));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_large_safe_struct_collections_survive_reruns() {
    let supervisor = Supervisor::default();
    run(
        &supervisor,
        "large-cache",
        r#"
Cache("lookup", () => Enumerable.Range(0, 60_000).ToDictionary(i => i.ToString(), i => i));
Cache("tuples", () => Enumerable.Range(0, 150_000).Select(i => (i, i + 1)).ToList());
"#,
        true,
    )
    .await;
    let events = run(&supervisor, "large-cache", r#"
Cache<Dictionary<string, int>>("lookup", () => throw new Exception("lookup recomputed"))["59999"].Dump();
Cache<List<(int, int)>>("tuples", () => throw new Exception("tuples recomputed"))[149999].Item2.Dump();
"#, true).await;
    assert_eq!(dumps(&events)[0]["v"], 59_999);
    assert_eq!(dumps(&events)[1]["v"], 150_000);
    assert!(!events.iter().any(|event| event.method == "stream"));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_cache_type_conflicts_do_not_evict_the_winner() {
    let supervisor = Supervisor::default();
    let events = run(&supervisor, "types", r#"
Cache("number", () => 42);
try { Cache("number", () => 7L); } catch (InvalidOperationException error) { error.Message.Dump(); }
Cache("number", () => -1).Dump();
var barrier = new Barrier(2);
var tasks = new[] {
    Task.Run(() => { try { Cache("race", () => { barrier.SignalAndWait(); return 1; }); return false; }
        catch (InvalidOperationException) { return true; } }),
    Task.Run(() => { try { Cache("race", () => { barrier.SignalAndWait(); return 2L; }); return false; }
        catch (InvalidOperationException) { return true; } })
};
(await Task.WhenAll(tasks)).Count(failed => failed).Dump();
"#, true).await;
    let nodes = dumps(&events);
    assert!(nodes[0]["v"]
        .as_str()
        .unwrap()
        .contains("Cache key 'number'"));
    assert_eq!(nodes[1]["v"], 42);
    assert_eq!(
        nodes[2]["v"], 1,
        "One incompatible factory loses with an explicit error"
    );
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_preview_and_late_workers_cannot_write_to_cache() {
    let supervisor = Supervisor::default();
    let events = run(&supervisor, "scope", r#"
IEnumerable<int> Items() {
    for (int i = 0; i < 101; i++) {
        if (i == 100) Cache("preview", () => 42);
        yield return i;
    }
}
Items().Dump();
var gate = Cache("gate", () => new int[2]);
new Thread(() => {
    while (Volatile.Read(ref gate[0]) == 0) Thread.Sleep(1);
    try { Cache("late", () => 42); }
    catch (InvalidOperationException) { Environment.SetEnvironmentVariable("CPAD_LATE_REJECTED", "yes"); }
    finally { Volatile.Write(ref gate[1], 1); }
}) { IsBackground = true }.Start();
"#, true).await;
    let page = supervisor
        .expand(
            "scope",
            dumps(&events)[0]["handle"].as_str().unwrap(),
            Some("100"),
            None,
        )
        .await
        .unwrap()
        .node;
    assert!(page["rows"][0][0]["message"]
        .as_str()
        .unwrap()
        .contains("active script run"));
    // Expansion invokes user code outside a run. Besides rejecting its write,
    // signal the original background worker only after run completion.
    let next = run(&supervisor, "scope", r#"
var gate = Cache("gate", () => new int[2]);
Volatile.Write(ref gate[0], 1);
if (!SpinWait.SpinUntil(() => Volatile.Read(ref gate[1]) == 1, 5000)) throw new Exception("worker stalled");
Environment.GetEnvironmentVariable("CPAD_LATE_REJECTED").Dump();
Cache("preview", () => -1).Dump(); Cache("late", () => -1).Dump();
"#, true).await;
    assert_eq!(dumps(&next)[0]["v"], "yes");
    assert_eq!(dumps(&next)[1]["v"], -1);
    assert_eq!(dumps(&next)[2]["v"], -1);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_cooperative_stop_works_with_an_exhausted_thread_pool() {
    let supervisor = std::sync::Arc::new(Supervisor::default());
    run(&supervisor, "control", "Cache(\"saved\", () => 42);", true).await;
    let task = csharp_start_until_ready(supervisor.clone(), r#"
ThreadPool.GetMinThreads(out var minWorker, out var minIo);
ThreadPool.GetMaxThreads(out var maxWorker, out var maxIo);
ThreadPool.SetMinThreads(1, minIo);
if (!ThreadPool.SetMaxThreads(1, maxIo)) throw new Exception("Could not restrict pool");
using var started = new ManualResetEventSlim();
using var registration = Cancel.Register(() => Environment.SetEnvironmentVariable("CPAD_CANCEL_CALLBACK", "yes"));
var worker = Task.Run(() => { started.Set(); Cancel.WaitHandle.WaitOne(); });
started.Wait();
Console.WriteLine("ready");
try { Cancel.WaitHandle.WaitOne(); worker.GetAwaiter().GetResult(); Cancel.ThrowIfCancellationRequested(); }
finally { ThreadPool.SetMaxThreads(maxWorker, maxIo); ThreadPool.SetMinThreads(minWorker, minIo); }
"#).await;
    supervisor.stop("control").await;
    let result = timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.stopped && !result.cache_cleared);
    let next = run(
        &supervisor,
        "control",
        r#"
Cache("saved", () => -1).Dump();
Environment.GetEnvironmentVariable("CPAD_CANCEL_CALLBACK").Dump();
"#,
        true,
    )
    .await;
    assert_eq!(dumps(&next)[0]["v"], 42);
    assert_eq!(dumps(&next)[1]["v"], "yes");
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_stop_during_compilation_preserves_cache() {
    let supervisor = std::sync::Arc::new(Supervisor::default());
    run(
        &supervisor,
        "compile-stop",
        "Cache(\"saved\", () => 42);",
        true,
    )
    .await;
    let mut source =
        String::from("Environment.SetEnvironmentVariable(\"CPAD_COMPILE_EXECUTED\", \"yes\");\n");
    for i in 0..100_000 {
        source.push_str(&format!("static int Method{i}() => {i};\n"));
    }
    let running = supervisor.clone();
    let task = tokio::spawn(async move {
        running
            .run_language("compile-stop", &source, Language::Csharp, |_| {})
            .await
            .unwrap()
    });
    // The warm kernel is already ready. Give it time to accept the source and
    // enter Roslyn, then exercise Stop while there is no script frame yet.
    tokio::time::sleep(Duration::from_millis(100)).await;
    supervisor.stop("compile-stop").await;
    let result = timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.stopped && !result.ok && !result.cache_cleared);
    let next = run(
        &supervisor,
        "compile-stop",
        r#"
Cache("saved", () => -1).Dump();
Environment.GetEnvironmentVariable("CPAD_COMPILE_EXECUTED").Dump();
"#,
        true,
    )
    .await;
    assert_eq!(dumps(&next)[0]["v"], 42);
    assert_eq!(dumps(&next)[1]["kind"], "null");
    supervisor.shutdown().await;
}
