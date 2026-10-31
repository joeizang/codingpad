# 01: LINQPad analysis

Sources: linqpad.net (home, purchase and LINQPad 9 pages), October 2026.

## What LINQPad is

LINQPad is a desktop scratchpad for .NET, and it has been around since 2007. You type a C#, F# or VB expression, statement block or full program, press F5, and see rich results right away. It began as a tool for testing LINQ-to-SQL queries and grew into a general .NET "Swiss-army knife". It runs on Windows and macOS (LINQPad 9 merged the two codebases) and has about 5M downloads.

## Feature inventory

### Execution
- Three script kinds: **Expression**, **Statements** and **Program**, so you don't need a `Main` or boilerplate.
- References: your own assemblies, NuGet packages, and other scripts or `.cs` files via `#load`.
- Async/await, `unsafe` code, and scripts that target .NET 6 to 10.
- `lprun` CLI, so scripts double as build and automation scripts.
- `Util.Cache`: keeps expensive intermediate results between runs, so you can edit and rerun quickly.
- Clone a script with one keystroke to try variations.
- Shows the generated SQL, the fluent/lambda form, IL and native code for a query.

### Output: `.Dump()`, the killer feature
- `.Dump()` works on any object and renders the object graph as nested, collapsible HTML tables.
- Lazy and deep properties appear as hyperlinks you click to expand.
- Async values and observables show a placeholder that fills in or animates live (Rx, Dataflow).
- Renders images, WPF and WinForms controls.
- `.Chart()`, a data-grid view, a watch window, and export to Excel/xlsx.
- Results are rich text (HTML) or grids.

### Data
- Connections to SQL Server, Azure SQL, SQLite, PostgreSQL, Oracle and MySQL, plus third-party drivers (EF Core, OData and others).
- Schema explorer, with **automatic typed data context**: tables become typed `IQueryable`s, and foreign keys become navigation properties, so you rarely need joins.
- LINQ or raw SQL, and editable grids (paid tiers).
- SQL→LINQ conversion with AI (Premium).

### Editor and IDE features
- Roslyn-powered autocompletion, smart tags, go-to-definition, rename and find references (paid tiers).
- Code fixes and refactorings, plus an integrated debugger (Premium).
- Snippets, outlining, bookmarks and feature search (Ctrl+Q).
- Samples and tutorials, including 1300 samples from *C# in a Nutshell*.

### Tooling and utilities
- One-keypress BenchmarkDotNet with a live visualizer.
- A RegEx evaluator, which is itself a LINQPad script and can be changed.
- A password manager for API keys, plus an OAuth/MFA helper that keeps tokens across runs.
- AI (Premium, opt-in): completion, chat, inline edit with Ctrl+I shown as a diff, a coding agent that can read the DB schema, multi-provider support (Copilot, OpenRouter), and the Claude Code CLI inside the app.

### Business model
Freemium. Free tier, then **Pro $65 / Developer $99 / Premium $149** per user, plus team and enterprise licences. Most "IDE" features (autocomplete, NuGet, debugger) are paid.

## Why people love it (what we must keep)
1. **Zero ceremony.** You go from an idea to running code in seconds, with no project, no csproj and no `Main`.
2. **`.Dump()` everything.** Structured, explorable output beats `print` by a wide margin.
3. **Typed database access without setup.** You point it at a DB and get a typed, autocompleting query surface.
4. **Persistent scratch library.** "My Queries" becomes a personal knowledge base of snippets.
5. **Fast feedback loop.** Caching, cloning and quick reruns.

## Gaps and opportunities
| Gap | Opportunity |
|---|---|
| Only .NET languages | First-class **C#, Python, Rust and TypeScript** behind one consistent UX |
| Windows/macOS only | **Linux** too, since the user base is cross-platform |
| Autocomplete, NuGet and debugger are paywalled | Full language intelligence included, via LSP |
| Proprietary `.linq` file format | Scripts are **valid native files** (`.cs`, `.py`, `.rs`, `.ts`), so they stay portable and git-friendly |
| Typed DB context only for LINQ | Typed DB access in every language (see vision doc) |
| Closed, single-vendor plugins | Open driver/visualizer plugin model |
| Desktop only | Possibly a web/remote mode later |

## Adjacent tools (prior art to study)
- **RoslynPad**: open-source C# scratchpad.
- **.NET Interactive / Polyglot Notebooks**: a multi-language kernel protocol for .NET. Worth checking its current support status.
- **Jupyter**: the kernel protocol, rich MIME outputs and a huge ecosystem, but its notebook UX is not "scratchpad" UX.
- **marimo**: reactive Python notebooks stored as plain `.py` files.
- **RunJS / Quokka.js**: JS/TS scratchpads with inline values.
- **Rust Playground / evcxr**: Rust snippet runner and REPL/Jupyter kernel.
- **Deno**: built-in TS, `npm:` specifiers and a Jupyter kernel.
