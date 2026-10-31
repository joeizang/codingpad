# 04: Decision log

## Decided

| # | Decision | Date |
|---|---|---|
| D1 | Primary users: **polyglot developers and learners** | 2026-10-30 |
| D2 | **Open source**, dual-licensed **MIT OR Apache-2.0** (the core app and the per-language helper libraries) | 2026-10-30 |
| D3 | **Database support is in the MVP** | 2026-10-30 |
| D4 | Language build order: **Python → C# → Rust → TypeScript** | 2026-10-30 |
| D5 | TypeScript runs on **Bun**, with a hidden project behind each script for completions. See [ADR 0002](adr/0002-bun-for-typescript.md) | 2026-10-30 |
| D6 | Product name stays **CodingPad** | 2026-10-30 |
| D7 | Desktop shell: **Tauri 2 on system webviews, move to bundled Chromium (CEF) when stable**. See [ADR 0001](adr/0001-tauri-with-system-webview-then-cef.md) | 2026-10-30 |
| D8 | MVP databases: **SQLite, PostgreSQL, SQL Server**. MySQL/MariaDB comes right after the MVP | 2026-10-30 |
| D9 | Database levels: **Level 2 (ready `db` client) for all four languages in the MVP; Level 3 (typed, schema-generated) in the MVP for C# (EF Core) and Python (SQLAlchemy 2.0)**; Drizzle planned for TypeScript, Rust deferred. See [ADR 0003](adr/0003-database-access-levels.md) | 2026-10-30 |
| D10 | Scripts are **plain native files** with a one-line `codingpad:` comment header. See [ADR 0004](adr/0004-scripts-are-plain-native-files.md) | 2026-10-30 |
| D11 | Execution: **one warm process per open script, a clean slate on every run**, an explicit `cache()` helper and a **Reset** button. Rust starts fresh each run, and its `cache()` persists to disk | 2026-10-30 |
| D12 | Toolchains: **use the user's installed toolchains first; if one is missing, offer a one-click private install** into CodingPad's data folder. Nothing is bundled in the installer | 2026-10-30 |
| D13 | **Windows, macOS and Linux are equal first-class platforms** from day one: CI builds and tests all three, and manual testing covers all three | 2026-10-30 |
| D14 | Learner content in the MVP: **built-in samples** (~10 runnable scripts per language) plus a **bundled SQLite sample database**. Full tutorials come later | 2026-10-30 |
| D15 | AI: **not in the MVP**. In a later phase, **bring-your-own-key, multi-provider including local models**. The MVP leaves clean extension points for it | 2026-10-30 |
| D16 | **Scratchpad layout only** (one script, one results panel). No notebook/cells view planned | 2026-10-30 |
| D17 | App ↔ kernel communication uses **CodingPad's own JSON-RPC protocol**, designed around `dump()`. A Jupyter adapter may come later. See [ADR 0005](adr/0005-own-kernel-protocol.md) | 2026-10-30 |
| D18 | C# runs by **in-memory Roslyn compilation inside the warm kernel**, keeping `#:package` directives compatible with `dotnet run`. See [ADR 0006](adr/0006-csharp-in-memory-roslyn.md) | 2026-10-30 |
| D19 | Rust runs as a **hidden cargo project per script with a shared build cache (`target/`)**, plus background pre-compiling of popular crates. evcxr was rejected because its behaviour differs from real Rust | 2026-10-30 |
| D20 | TypeScript and Python scripts **run even with type errors**; errors show as squiggles plus a warning count. C# and Rust block, because they cannot compile | 2026-10-31 |
| D21 | Script storage: a default **library** folder (`Documents/CodingPad`, configurable) plus user-added **pinned folders**. Any single file can also be opened | 2026-10-31 |
| D22 | **The last expression is dumped automatically** in Python, C# and TypeScript. In Rust only when the script ends with a plain expression; otherwise explicit `dump()` | 2026-10-31 |
| D23 | Connection passwords and other secrets live in the **OS password vault** (Keychain / Credential Manager / Secret Service). A `secret("name")` helper is in all four languages in the MVP. Without a vault, prompt once per session | 2026-10-31 |
| D24 | Code editor component: **Monaco** | 2026-10-31 |
| D25 | UI framework: **React + TypeScript**, with virtualised rendering for large dumps and tables | 2026-10-31 |
| D26 | Language servers: **basedpyright** (switch to **ty** at 1.0), **Roslyn LSP** (C#), **rust-analyzer**, **TypeScript 7 native LSP** | 2026-10-31 |
| D27 | MVP dump supports **object trees with lazy nodes, virtualised tables, live-updating dumps, images, HTML, and CSV/Excel export of tables**. `chart()` comes in the next phase | 2026-10-31 |
| D28 | MVP includes a terminal command **`cpad run <script>`**: it uses connections, secrets and `cache()`, and dumps as text or HTML | 2026-10-31 |
| D29 | **No auto-updater for now.** The app checks GitHub Releases and shows a "new version available" notice linking to the release page. Releases are published on GitHub Releases: `.dmg` (macOS), installer (Windows), AppImage/`.deb`/`.rpm` (Linux). Flatpak later | 2026-10-31 |
| D30 | Early releases are **unsigned** on macOS and Windows; first-launch instructions in the README; Homebrew via our own tap; apply to SignPath for free Windows signing later. See [ADR 0007](adr/0007-unsigned-releases-no-auto-update.md) | 2026-10-31 |
| D31 | **No telemetry or automatic data collection.** On a crash, a "Report on GitHub" button opens a pre-filled issue. The only network call the app makes on its own is the GitHub release check | 2026-10-31 |
| D32 | **Debugger in the next phase after the MVP**, for all four languages together via DAP (debugpy, netcoredbg, CodeLLDB, Bun inspector) | 2026-10-31 |
| D33 | MVP milestone order: **M1** foundations + Python → **M2** C# (in-memory compile, NuGet) → **M3** databases (connections, schema panel, `db` client and data contexts for Python and C#, secrets, export) → **M4** polish (samples, `cpad run`, update notice, crash reporting, first-launch docs) → **M5** Rust → **M6** TypeScript = 1.0 | 2026-10-31 |
| D34 | **First public preview (0.1) ships after M4**, then 0.2 after M5 (Rust) and 1.0 after M6 (TypeScript). The repo is public throughout, so anyone can build from source | 2026-10-31 |
| D35 | **SQL is a fifth script type** (`.sql` files with a script header naming the connection), run directly through the connection with no kernel. Schema-aware autocomplete. Built in M3 | 2026-10-31 |
| D36 | **Monorepo**: app, kernels and per-language helper libraries all live in this repository, each in its own folder with its own build tooling | 2026-10-31 |
| D37 | Runtime libraries are named **`codingpad`** on PyPI, NuGet, crates.io and npm (C# namespace `CodingPad`). Reserve `codingpad` and `cpad` on all four now with placeholder releases. Packages are ready in `packaging/placeholders/` (**owner: Joseph, publishes with their registry accounts**) | 2026-10-31 |
| D38 | Performance targets for 1.0 (tracked by automated checks): app usable <1s; warm rerun Python/TS <200ms, C# <500ms, Rust after a small edit <3s; first kernel start <3s (Rust longer on first compile); 100k-row dump never freezes, first rows <1s; completions <300ms; idle memory <300MB | 2026-10-31 |


## Open

None. Every branch of the design interview is settled (2026-10-31).
