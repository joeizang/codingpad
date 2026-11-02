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
