use codingpad_kernel_core::lsp::{LanguageServers, LspEvent};
use serde_json::json;
use std::time::Duration;
use tokio::time::timeout;

async fn next_diagnostics(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<LspEvent>,
) -> Vec<serde_json::Value> {
    timeout(Duration::from_secs(120), async {
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
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let started = timeout(
        Duration::from_secs(120),
        servers.start_python("one", source, move |event| {
            let _ = sender.send(event);
        }),
    )
    .await
    .expect("language server startup timed out")
    .expect("language server failed to start");

    let diagnostics = next_diagnostics(&mut receiver).await;

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
        servers.start_python("packages", source, move |event| {
            let _ = sender.send(event);
        }),
    )
    .await
    .expect("language server startup timed out")
    .expect("language server failed to start");

    let diagnostics = next_diagnostics(&mut receiver).await;

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
    let source = "value: int = \"bad\"\n";
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let first = timeout(
        Duration::from_secs(120),
        servers.start_python("reuse", source, move |event| {
            let _ = sender.send(event);
        }),
    )
    .await
    .expect("language server startup timed out")
    .expect("language server failed to start");
    let first_diagnostics = next_diagnostics(&mut receiver).await;
    assert!(!first_diagnostics.is_empty());

    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let second = timeout(
        Duration::from_secs(120),
        servers.start_python("reuse", source, move |event| {
            let _ = sender.send(event);
        }),
    )
    .await
    .expect("language server restart timed out")
    .expect("language server failed to restart");
    assert_eq!(first.uri, second.uri);
    // The reused server must send diagnostics to the new channel.
    let _ = next_diagnostics(&mut receiver).await;

    let package_source = r#"# /// script
# dependencies = ["requests"]
# ///
from requests import get
value: int = "bad"
"#;
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    let third = timeout(
        Duration::from_secs(120),
        servers.start_python("reuse", package_source, move |event| {
            let _ = sender.send(event);
        }),
    )
    .await
    .expect("language server package restart timed out")
    .expect("language server failed to restart with packages");
    assert_ne!(first.uri, third.uri);
    servers.close("reuse").await;
}
