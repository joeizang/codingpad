use codingpad_kernel_core::lsp::{LanguageServers, LspEvent};
use serde_json::json;
use std::time::Duration;
use tokio::time::timeout;

async fn next_diagnostics(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<LspEvent>,
    document_uri: &str,
) -> Vec<serde_json::Value> {
    timeout(Duration::from_secs(120), async {
        loop {
            let Some(event) = receiver.recv().await else {
                panic!("language server closed before publishing diagnostics");
            };
            if event.method == "textDocument/publishDiagnostics" {
                assert_eq!(event.params["uri"].as_str(), Some(document_uri));
                let diagnostics = event.params["diagnostics"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if !diagnostics.is_empty() {
                    return diagnostics;
                }
            }
        }
    })
    .await
    .expect("no diagnostics arrived")
}

#[tokio::test]
#[ignore = "requires uv and downloads basedpyright; run with --ignored"]
async fn python_code_intelligence_reports_diagnostics_and_completions() {
    let servers = LanguageServers::default();
    let source = r#"from codingpad import dump

def greet(name: str) -> str:
    return f"Hello {name}"

value: int = "not an int"
dump(greet("Ada"))
"#;
    servers.begin_session("test").await;
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let started = timeout(
        Duration::from_secs(120),
        servers.start_python("one", source, "test", move |event| {
            let _ = sender.send(event);
        }),
    )
    .await
    .expect("language server startup timed out")
    .expect("language server failed to start");

    let diagnostics = next_diagnostics(&mut receiver, &started.uri).await;

    assert!(
        diagnostics.iter().any(|diagnostic| diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("int") || message.contains("assign"))),
        "unexpected diagnostics: {diagnostics:?}"
    );

    let completions = servers
        .request(
            "one",
            "textDocument/completion",
            json!({
                "textDocument": { "uri": started.uri },
                "position": { "line": 0, "character": 5 },
                "context": { "triggerKind": 1 }
            }),
        )
        .await
        .expect("completion request failed");
    assert!(completions.is_object() || completions.is_array());

    let hover = servers
        .request(
            "one",
            "textDocument/hover",
            json!({
                "textDocument": { "uri": started.uri },
                "position": { "line": 6, "character": 6 }
            }),
        )
        .await
        .expect("hover request failed");
    assert!(hover.is_null() || hover.is_object());

    servers.stop("one").await;
}

#[tokio::test]
#[ignore = "requires uv and downloads basedpyright; run with --ignored"]
async fn python_code_intelligence_uses_inline_packages() {
    let servers = LanguageServers::default();
    let source = r#"# /// script
# dependencies = ["requests"]
# ///
from requests import get
value: int = "not an int"
"#;
    servers.begin_session("test").await;
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let started = timeout(
        Duration::from_secs(120),
        servers.start_python("packages", source, "test", move |event| {
            let _ = sender.send(event);
        }),
    )
    .await
    .expect("language server startup timed out")
    .expect("language server failed to start");

    let diagnostics = next_diagnostics(&mut receiver, &started.uri).await;

    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("could not be resolved")
                    && message.contains("requests"))),
        "inline package was not visible to basedpyright: {diagnostics:?}"
    );
    servers.stop("packages").await;
}

#[tokio::test]
#[ignore = "requires uv and downloads basedpyright; run with --ignored"]
async fn reuses_the_warm_server_and_restarts_when_packages_change() {
    let servers = LanguageServers::default();
    servers.begin_session("test").await;
    let source = "value: int = \"bad\"\n";
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let first = timeout(
        Duration::from_secs(120),
        servers.start_python("reuse", source, "test", move |event| {
            let _ = sender.send(event);
        }),
    )
    .await
    .expect("language server startup timed out")
    .expect("language server failed to start");
    let first_diagnostics = next_diagnostics(&mut receiver, &first.uri).await;
    assert!(!first_diagnostics.is_empty());

    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let second = timeout(
        Duration::from_secs(120),
        servers.start_python("reuse", source, "test", move |event| {
            let _ = sender.send(event);
        }),
    )
    .await
    .expect("language server restart timed out")
    .expect("language server failed to restart");
    assert_eq!(first.uri, second.uri);
    // The reused server must send diagnostics to the new channel.
    let _ = next_diagnostics(&mut receiver, &second.uri).await;

    let package_source = r#"# /// script
# dependencies = ["requests"]
# ///
from requests import get
value: int = "bad"
"#;
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let third = timeout(
        Duration::from_secs(120),
        servers.start_python("reuse", package_source, "test", move |event| {
            let _ = sender.send(event);
        }),
    )
    .await
    .expect("language server package restart timed out")
    .expect("language server failed to restart with packages");
    assert_ne!(first.uri, third.uri);
    servers.close("reuse").await;
}

#[tokio::test]
async fn csharp_code_intelligence_uses_the_shadow_project() {
    // Roslyn ships with the VS Code C# extension rather than the .NET SDK.
    // Set CODINGPAD_ROSLYN_LSP to opt in to this end-to-end check.
    if std::env::var_os("CODINGPAD_ROSLYN_LSP").is_none() {
        eprintln!("skipping: set CODINGPAD_ROSLYN_LSP to a Roslyn language server");
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("script.cs");
    let source = "var value = 42;\nvalue.Dump();\nCache(\"answer\", () => 42);\nmissing();\n";
    std::fs::write(&script, source).unwrap();

    let servers = LanguageServers::with_data_dir(directory.path().join("data"));
    servers.begin_session("test").await;
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let started = timeout(
        Duration::from_secs(120),
        servers.start_csharp("csharp", source, Some(&script), "test", move |event| {
            let _ = sender.send(event);
        }),
    )
    .await
    .expect("language server startup timed out")
    .expect("language server failed to start");

    let diagnostics = next_diagnostics(&mut receiver, &started.uri).await;
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == "CS0103"),
        "unexpected diagnostics: {diagnostics:?}"
    );

    let mut labels = Vec::new();
    for _ in 0..40 {
        let completions = servers
            .request(
                "csharp",
                "textDocument/completion",
                json!({
                    "textDocument": { "uri": started.uri },
                    "position": { "line": 1, "character": 6 },
                    "context": { "triggerKind": 2, "triggerCharacter": "." }
                }),
            )
            .await
            .expect("completion request failed");
        labels = completions
            .get("items")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| item["label"].as_str().map(str::to_owned))
            .collect();
        if labels.iter().any(|label| label.starts_with("Dump")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        labels.iter().any(|label| label.starts_with("Dump")),
        "missing Dump completion: {labels:?}"
    );
}
