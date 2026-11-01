use codingpad_kernel_core::lsp::LanguageServers;
use serde_json::json;
use std::time::Duration;
use tokio::time::timeout;

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
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let started = timeout(
        Duration::from_secs(120),
        servers.start_python("one", source, None, move |event| {
            let _ = sender.send(event);
        }),
    )
    .await
    .expect("language server startup timed out")
    .expect("language server failed to start");

    let diagnostics = timeout(Duration::from_secs(120), async {
        loop {
            let Some(event) = receiver.recv().await else {
                panic!("language server closed before publishing diagnostics");
            };
            if event.method == "textDocument/publishDiagnostics" {
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
    .expect("no diagnostics arrived");

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
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let _started = timeout(
        Duration::from_secs(120),
        servers.start_python("packages", source, None, move |event| {
            let _ = sender.send(event);
        }),
    )
    .await
    .expect("language server startup timed out")
    .expect("language server failed to start");

    let diagnostics = timeout(Duration::from_secs(120), async {
        loop {
            let Some(event) = receiver.recv().await else {
                panic!("language server closed before publishing diagnostics");
            };
            if event.method == "textDocument/publishDiagnostics" {
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
    .expect("no diagnostics arrived");

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
