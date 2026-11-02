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
