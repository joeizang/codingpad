# 03: Architecture

Decision numbers (D#) refer to [04-open-questions.md](04-open-questions.md). Terms follow [GLOSSARY.md](../GLOSSARY.md).

## High-level shape

```
┌────────────── Tauri 2 window (system webview → CEF later) ──────────────┐
│  React + TypeScript UI: Monaco editor · dump viewer · library sidebar   │
│  schema panel · connection manager                                      │
└───────────────▲─────────────────────────────────────────────────────────┘
                │ Tauri IPC
┌───────────────┴──────────────── Rust core ──────────────────────────────┐
│  script store (library + pinned folders) · script header parser          │
│  kernel supervisor · language-server multiplexer · toolchain manager     │
│  shadow-project manager · secrets (OS vault) · release checker           │
└──────▲──────────────▲──────────────▲──────────────▲─────────────▲───────┘
       │      CodingPad kernel protocol (JSON-RPC over stdio)      │ SQL
  ┌────┴───┐     ┌────┴───┐     ┌────┴───┐     ┌────┴───┐     ┌───┴────┐
  │ Python │     │   C#   │     │  Rust  │     │   TS   │     │  SQL   │
  │ kernel │     │ kernel │     │ runner │     │ (Bun)  │     │ (direct│
  └────────┘     └────────┘     └────────┘     └────────┘     │ conn.) │
   each kernel imports its language's `codingpad` runtime library └──────┘
```

`cpad run` (D28) reuses the Rust core and kernels without the window.

## Decisions at a glance
| Area | Decision | Ref |
|---|---|---|
| Shell | Tauri 2 on system webviews; CEF when stable; safe drawing mode on Linux/NVIDIA | [ADR 0001](adr/0001-tauri-with-system-webview-then-cef.md) |
| UI | React + TypeScript, Monaco, virtualised rendering | D24, D25 |
| Kernel protocol | Own JSON-RPC, built around dump (streaming, lazy nodes, paging, cancel) | [ADR 0005](adr/0005-own-kernel-protocol.md) |
| Execution | Warm kernel per open script, clean slate per run, `cache()`, Reset | D11 |
| Files | Plain native files + one-line script header | [ADR 0004](adr/0004-scripts-are-plain-native-files.md) |
| Toolchains | Detect installed ones; otherwise one-click private install | D12 |
| Secrets | OS vault (Keychain / Credential Manager / Secret Service) | D23 |
| Repo | Monorepo | D36 |

## Per-language strategy
| Language | Execution | Dependencies | Language server | Data context |
|---|---|---|---|---|
| Python | Warm process per script; `exec` of the compiled AST; last expression auto-dumped | PEP 723 via `uv`, cached venv per dependency set | basedpyright (→ ty) | SQLAlchemy 2.0 (MVP) |
| C# | Roslyn in-memory compile into a collectible `AssemblyLoadContext` ([ADR 0006](adr/0006-csharp-in-memory-roslyn.md)) | `#:package`, restored by the kernel via NuGet client libraries | Roslyn LSP | EF Core (MVP) |
| Rust | Shadow cargo project per script, shared `target/`, popular crates precompiled in the background (D19) | cargo-script frontmatter | rust-analyzer on the shadow project | Post-1.0 (SQL Server gap noted) |
| TypeScript | Bun ([ADR 0002](adr/0002-bun-for-typescript.md)) | Versioned imports (`zod@3`), mirrored into a shadow project | TypeScript 7 native LSP on the shadow project | Drizzle, post-1.0 ([ADR 0003](adr/0003-database-access-levels.md)) |
| SQL | Run directly through the connection; no kernel (D35) | none | Schema-aware completion | n/a |

## Dump wire format (sketch, refined in M1)
A language-neutral tree, streamed as it is produced:
```json
{ "id": "n42", "kind": "object", "type": "Customer",
  "fields": [ {"name":"Name","value":{"kind":"string","v":"Ada"}},
              {"name":"Orders","value":{"kind":"lazy","handle":"h17","preview":"List<Order> (12)"}} ] }
```
Kinds: primitive, string, null, object, table (paged), map, lazy, image, html, error, live. Charts come after 1.0. A kernel keeps the objects behind lazy nodes alive until the next run or a Reset.

## Risks
- **WebKitGTK on Linux/NVIDIA**: mitigated with safe drawing mode, and the planned CEF move (ADR 0001).
- **Install footprint** (four toolchains, four language servers): mitigated by on-demand install (D12).
- **Rust compile latency**: mitigated by the shared target dir and precompiling (D19). Target: under 3s after a small edit (D38).
- **C# in-memory compile**: we own NuGet restore and must keep up with new `#:` directives (ADR 0006).
- **Security**: scripts run with full user privileges in every language, as in LINQPad. Any future sandbox should be CodingPad-wide.
- **Unsigned builds** raise first-launch friction (ADR 0007).
