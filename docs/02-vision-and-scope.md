# 02: Vision and scope

Decision numbers (D#) refer to [04-open-questions.md](04-open-questions.md). Terms follow [GLOSSARY.md](../GLOSSARY.md).

## One-liner
> An open-source, polyglot scratchpad: write a script in Python, C#, Rust, TypeScript or SQL, hit run, and *explore* the results, including your databases.

## Users (D1)
1. **Polyglot developers** who move between stacks and want one tool instead of four REPLs and a database client.
2. **Learners** who need runnable samples, rich output and a one-click setup.

## Principles
1. **Zero ceremony.** A script is one file. Dependencies are declared inline, and missing toolchains install in one click (D12).
2. **Native files.** Scripts are plain `.py`/`.cs`/`.rs`/`.ts`/`.sql` files that run without CodingPad (D10).
3. **One dump for every language.** The same rich, explorable output everywhere (D17, D22, D27).
4. **Instant feedback.** Warm kernels, a clean slate each run, and `cache()` for slow steps (D11, D38).
5. **Local-first and private.** No telemetry. Secrets stay in the OS vault (D23, D31).
6. **Free and open.** MIT OR Apache-2.0, with every feature included (D2).

## MVP (1.0) feature set
- **Languages:** Python (uv), C# (in-memory Roslyn), Rust (shadow cargo projects), TypeScript (Bun), plus SQL scripts (D4, D18, D19, D5, D35).
- **Editor:** Monaco with language servers: basedpyright, Roslyn LSP, rust-analyzer and TypeScript 7 (D24, D26). TS/Python run despite type errors (D20).
- **Dump:** object trees with lazy nodes, virtualised tables, live updates, images, HTML, and CSV/Excel export. The last expression is auto-dumped (D22, D27).
- **Databases:** SQLite, PostgreSQL and SQL Server (D8). A database client in every language. Data contexts for C# (EF Core) and Python (SQLAlchemy 2.0) (D9).
- **Library:** a default library folder plus pinned folders (D21).
- **Secrets:** OS vault, with `secret("name")` (D23).
- **CLI:** `cpad run <script>` (D28).
- **Learners:** built-in samples plus a bundled SQLite sample database (D14).
- **Platforms:** Windows, macOS and Linux, all equal (D13).
- **Distribution:** GitHub Releases, unsigned at first, with an update notice instead of an auto-updater (D29, D30).

## Milestones (D33, D34)
Builds and tests run locally with `just`, no CI (D39, D42, D43).

| Milestone | Scope | Release |
|---|---|---|
| M1 | Foundations: skeleton first (D40), then full dump, run control, library, Python packages, basedpyright, `uv` install (D47). Closed by owner (D48) | |
| M2 | C# kernel: skeleton, full dump, run control, `#:package`/`#:property`/`#:sdk Web`, Roslyn LSP via shadow project, .NET SDK install (D49–D60). Closed by owner (D55) | |
| M3 | Databases: connections, schema panel, SQL scripts, database client, data contexts (Python + C#), secrets, export | |
| M4 | Polish: samples + sample DB, `cpad run`, update notice, "Report on GitHub", first-launch docs | **0.1 preview** |
| M5 | TypeScript (Bun) kernel | 0.2 |
| M6 | Rust kernel, shared build cache | **1.0** |

## After 1.0
- Debugger via DAP for all four languages (D32).
- `chart()` (D27).
- Data contexts for TypeScript (Drizzle) and Rust (D9). MySQL/MariaDB (D8).
- AI: bring-your-own-key, multi-provider including local models (D15).
- Switch Python's language server to ty at 1.0 (D26). Bundled Chromium (CEF) once stable (D7).
- Code signing and auto-updates when budget allows (D30).

## Not planned
- Notebook/cells view (D16).
- Telemetry (D31).
- Collaboration and cloud hosting.
