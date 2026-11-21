use codingpad_kernel_core::lsp::{LanguageServers, LspEvent};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::time::timeout;

fn has_label(result: &serde_json::Value, expected: &str) -> bool {
    result
        .get("items")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item["label"].as_str() == Some(expected)
                    || item["label"]["label"].as_str() == Some(expected)
            })
        })
}

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
typed_dump: str = dump(42)
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

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["range"]["start"]["line"] == 7),
        "dump should preserve its argument type via the bundled stub: {diagnostics:?}"
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
    let source = "#:property Nullable=disable\n#:property ImplicitUsings=disable\nvar value = 42;\nvalue.Dump();\nCache(\"answer\", () => 42);\nConsole.WriteLine(value);\nmissing();\n";
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
                    "position": { "line": 3, "character": 6 },
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

    // A tab switch remounts the editor with a new event sink but reuses the
    // warm server. Unchanged diagnostics must still reach the new sink.
    let (second_sender, mut second_receiver) = tokio::sync::mpsc::unbounded_channel();
    let restarted = servers
        .start_csharp("csharp", source, Some(&script), "test", move |event| {
            let _ = second_sender.send(event);
        })
        .await
        .expect("reused language server failed");
    assert_eq!(restarted.uri, started.uri);
    let repeated = timeout(Duration::from_secs(10), async {
        loop {
            let Some(event) = second_receiver.recv().await else {
                panic!("language server closed before resending diagnostics");
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
    .expect("reused server did not resend diagnostics");
    assert!(
        repeated
            .iter()
            .any(|diagnostic| diagnostic["code"] == "CS0103"),
        "reused server sent no current diagnostics: {repeated:?}"
    );
}

#[tokio::test]
async fn typescript_code_intelligence_uses_the_shadow_project() {
    // The TypeScript 7 native server is a separate preview install. Set
    // CODINGPAD_TS_LSP to opt in to this end-to-end check.
    if std::env::var_os("CODINGPAD_TS_LSP").is_none() {
        eprintln!("skipping: set CODINGPAD_TS_LSP to a TypeScript 7 language server");
        return;
    }
    let servers = LanguageServers::default();
    let source = r#"import {  } from "codingpad";
import { dump, cache, db as attached, secret as readSecret } from "codingpad";
const value: number = "not a number";
dump(value);
cache("answer", () => 42);
await attached.query("SELECT 1");
await db.query("SELECT ?", [secret("name")]);
const token: string = readSecret("token");
dump(token);
"#;
    servers.begin_session("test").await;
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let started = timeout(
        Duration::from_secs(120),
        servers.start_typescript("typescript", source, "test", move |event| {
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
            .any(|diagnostic| diagnostic["code"] == 2322),
        "expected the assignment type error: {diagnostics:?}"
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic["severity"] == 1)
            .count(),
        1,
        "unexpected helper errors: {diagnostics:?}"
    );
    assert!(
        !diagnostics.iter().any(|diagnostic| diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("Cannot find module 'codingpad'"))),
        "the bundled codingpad types were not visible: {diagnostics:?}"
    );
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == 5102),
        "the generated project used a setting TypeScript 7 removed: {diagnostics:?}"
    );

    let mut labels = Vec::new();
    for _ in 0..40 {
        let completions = servers
            .request(
                "typescript",
                "textDocument/completion",
                json!({
                    "textDocument": { "uri": started.uri },
                    "position": { "line": 0, "character": 9 },
                    "context": { "triggerKind": 1 }
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
            .filter_map(|item| {
                item["label"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| item["label"]["label"].as_str().map(str::to_owned))
            })
            .collect();
        if ["dump", "cache", "db", "secret"]
            .iter()
            .all(|name| labels.iter().any(|label| label == name))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    for name in ["dump", "cache", "db", "secret"] {
        assert!(
            labels.iter().any(|label| label == name),
            "missing {name} completion: {labels:?}"
        );
    }
    servers.stop("typescript").await;
}

#[tokio::test]
#[ignore = "installs a versioned package and needs Bun plus network or a warm Bun cache"]
async fn typescript_code_intelligence_resolves_versioned_packages() {
    if std::env::var_os("CODINGPAD_TS_LSP").is_none() {
        eprintln!("skipping: set CODINGPAD_TS_LSP to a TypeScript 7 language server");
        return;
    }
    let servers = LanguageServers::default();
    let source = r#"import { z } from "zod@3";
const schema = z.string();
const value: number = "not a number";
schema.parse("hi");
"#;
    servers.begin_session("test").await;
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let started = timeout(
        Duration::from_secs(120),
        servers.start_typescript("typescript-packages", source, "test", move |event| {
            let _ = sender.send(event);
        }),
    )
    .await
    .expect("language server startup timed out")
    .expect("language server failed to start");

    let diagnostics = next_diagnostics(&mut receiver, &started.uri).await;
    assert!(
        !diagnostics.iter().any(|diagnostic| diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("Cannot find module 'zod@3'"))),
        "the versioned package was not visible: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == 2322),
        "expected the assignment type error: {diagnostics:?}"
    );
    servers.stop("typescript-packages").await;
}

#[tokio::test]
async fn typescript_completion_is_under_300_ms() {
    if std::env::var_os("CODINGPAD_TS_LSP").is_none() {
        eprintln!("skipping: set CODINGPAD_TS_LSP to a TypeScript 7 language server");
        return;
    }
    let servers = LanguageServers::default();
    let source = "import {  } from \"codingpad\";\nconst value = 42;\n";
    servers.begin_session("test").await;
    let started = timeout(
        Duration::from_secs(120),
        servers.start_typescript("typescript-latency", source, "test", |_| {}),
    )
    .await
    .expect("language server startup timed out")
    .expect("language server failed to start");

    let mut warm = false;
    for _ in 0..40 {
        let result = servers
            .request(
                "typescript-latency",
                "textDocument/completion",
                json!({
                    "textDocument": { "uri": started.uri },
                    "position": { "line": 0, "character": 9 },
                    "context": { "triggerKind": 1 }
                }),
            )
            .await
            .expect("completion request failed");
        if has_label(&result, "dump") {
            warm = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(warm, "the helper completion never loaded");

    // Warm the number member list once, then time the first completion after a
    // small edit. This mirrors the D38 warm-completion target.
    servers
        .did_change(
            "typescript-latency",
            "import {  } from \"codingpad\";\nconst value = 42;\nvalue.",
        )
        .await
        .expect("document change failed");
    let mut members = false;
    for _ in 0..40 {
        let result = servers
            .request(
                "typescript-latency",
                "textDocument/completion",
                json!({
                    "textDocument": { "uri": started.uri },
                    "position": { "line": 2, "character": 6 },
                    "context": { "triggerKind": 2, "triggerCharacter": "." }
                }),
            )
            .await
            .expect("completion request failed");
        if has_label(&result, "toFixed") {
            members = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(members, "the number member completion never loaded");

    servers
        .did_change(
            "typescript-latency",
            "import {  } from \"codingpad\";\nconst value = 42;\nvalue.toF",
        )
        .await
        .expect("document change failed");
    let measured = Instant::now();
    let result = servers
        .request(
            "typescript-latency",
            "textDocument/completion",
            json!({
                "textDocument": { "uri": started.uri },
                "position": { "line": 2, "character": 9 },
                "context": { "triggerKind": 1 }
            }),
        )
        .await
        .expect("completion request failed");
    let elapsed = measured.elapsed();
    eprintln!(
        "TypeScript completion: {:.1} ms",
        elapsed.as_secs_f64() * 1000.0
    );
    assert!(
        has_label(&result, "toFixed"),
        "unexpected completion: {result:?}"
    );
    assert!(
        elapsed < Duration::from_millis(300),
        "TypeScript completion took {elapsed:?}"
    );
    servers.stop("typescript-latency").await;
}

#[tokio::test]
#[ignore = "requires rust-analyzer and rust-src; run with --ignored"]
async fn rust_code_intelligence_maps_shadow_edits_and_compiler_errors() {
    use codingpad_kernel_core::{Language, Supervisor};
    use serde_json::Value;

    let source = concat!(
        "---cargo\n[dependencies]\nserde_json = \"1\"\n---\n// codingpad: connection=example\n",
        "let _emoji = \"😀\"; let _ = codingpad::du;\n",
        "let _ = codingpad::ca;\n",
        "let _ = codingpad::se;\n",
        "let _: Option<serde_json::Valu> = None;\n",
    );
    let servers = LanguageServers::default();
    servers.begin_session("rust-test").await;
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let started = servers
        .start_rust("one", source, None, "rust-test", move |event| {
            let _ = sender.send(event);
        })
        .await
        .expect("start rust-analyzer");
    let complete = |line, character| {
        servers.request(
            "one",
            "textDocument/completion",
            json!({
                "textDocument": { "uri": started.uri },
                "position": { "line": line, "character": character },
                "context": { "triggerKind": 1 }
            }),
        )
    };
    let matches_item =
        |item: &Value, expected: &str| item["label"] == expected || item["filterText"] == expected;
    let positions = [
        ("du;", "dump"),
        ("ca;", "cache"),
        ("se;", "secret"),
        ("Valu>", "Value"),
    ];
    for (index, (token, expected)) in positions.into_iter().enumerate() {
        let line = index + 5;
        let text = source.lines().nth(line).unwrap();
        let start = text.rfind(token).unwrap();
        let end = start + token.len() - 1;
        let column = text[..end].encode_utf16().count();
        timeout(Duration::from_secs(90), async {
            loop {
                let result = complete(line, column).await.expect("Rust completion");
                if result["items"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| matches_item(item, expected)))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("rust-analyzer did not resolve {expected}"));

        let start_time = Instant::now();
        let result = complete(line, column).await.unwrap();
        let elapsed = start_time.elapsed();
        assert!(
            elapsed < Duration::from_millis(300),
            "Warm Rust {expected} completion took {elapsed:?}"
        );
        let item = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| matches_item(item, expected))
            .unwrap();
        let edit = &item["textEdit"];
        let range = edit.get("range").unwrap_or(&edit["replace"]);
        assert_eq!(range["start"]["line"], line, "{item}");
        assert_eq!(
            range["start"]["character"],
            text[..start].encode_utf16().count(),
            "{item}"
        );
    }

    let edited = concat!(
        "---cargo\n[dependencies]\nserde_json = \"1\"\n---\n// codingpad: connection=example\n",
        "let _: u32 = \"not a number\";\n"
    );
    servers.did_change("one", edited).await.unwrap();
    let mut compiled = None;
    let result = Supervisor::default()
        .run_language("one", edited, Language::Rust, |event| {
            if event.method == "diagnostics" {
                compiled = event.params["diagnostics"]
                    .as_array()
                    .and_then(|items| items.first())
                    .cloned();
            }
        })
        .await
        .unwrap();
    assert!(!result.ok);
    let compiled = compiled.expect("rustc diagnostics");
    let received = timeout(Duration::from_secs(90), async {
        loop {
            let event = receiver.recv().await.expect("rust-analyzer diagnostics");
            if let Some(diagnostic) = event.params["diagnostics"]
                .as_array()
                .and_then(|items| items.iter().find(|item| item["code"] == "E0308").cloned())
            {
                break diagnostic;
            }
        }
    })
    .await
    .expect("rust-analyzer did not publish rustc's type mismatch");
    let range = |key: &str| -> &Value { &received["range"][key] };
    assert_eq!(
        range("start")["line"].as_u64().unwrap() + 1,
        compiled["line"].as_u64().unwrap()
    );
    assert_eq!(
        range("start")["character"].as_u64().unwrap() + 1,
        compiled["column"].as_u64().unwrap()
    );
    servers.close("one").await;
}
