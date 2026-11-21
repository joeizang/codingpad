# CodingPad

CodingPad is an open-source, polyglot scratchpad: write a script in Python, C#, Rust or TypeScript, run it instantly, and explore the results, including data from your databases.

## Language

### Scripts

**Script**:
A single file of code in one language that the user writes, runs and saves in CodingPad.
_Avoid_: Query, snippet, pad

**Script header**:
The single `codingpad:` comment that holds CodingPad-only settings, such as the attached connection. In a Rust script with cargo-script frontmatter, the comment follows the frontmatter.
_Avoid_: Metadata, sidecar

**Cargo-script frontmatter**:
The embedded Cargo manifest for a single-file Rust script, including its crate declarations.
_Avoid_: Rust Cargo frontmatter, Cargo single-file frontmatter

**Library**:
The user's main folder of saved scripts, always shown in the sidebar.
_Avoid_: My Queries, workspace

**Pinned folder**:
An extra folder of scripts, outside the library, that the user adds to the sidebar.
_Avoid_: Workspace, project folder

**Sample**:
A runnable script shipped with CodingPad to teach a language or feature.
_Avoid_: Tutorial, example

### Running

**Kernel**:
The background process for one language that runs a script and reports its results back to CodingPad.
_Avoid_: Engine, runner, runtime

**Runtime library**:
The small per-language package named `codingpad` that scripts import for `dump()`, `cache()`, `secret()` and the database client.
_Avoid_: Helper library, SDK, runtime

**Shadow project**:
A hidden project CodingPad keeps behind a script so that a language's tools can build it and understand it. The user never edits it. Rust stages an execution shadow project for Run and a
separate editor shadow project for rust-analyzer. Both use the same Cargo
manifest and source transformation.
_Avoid_: Hidden project, workspace

### Problems

**Crash**:
An unexpected error that stops the interface or ends CodingPad. CodingPad never sends anything about a crash on its own.
_Avoid_: Failure, panic, exception

**Crash report**:
A GitHub issue CodingPad prepares with the app version, the platform and the error details, for the user to review and submit.
_Avoid_: Bug report, telemetry, error log

**Pending crash**:
A crash recorded by an earlier launch and offered for reporting on the next one.
_Avoid_: Saved crash, crash log

### Output

**Dump**:
Rich, explorable output of a value, shown as expandable trees and tables in the results panel.
_Avoid_: Output, print, display

**Auto-dump**:
The automatic dump of a script's final value at the end of a run, without an explicit `dump()` call.
_Avoid_: Implicit output, result

**Lazy node**:
A part of a dump whose contents are fetched only when the user expands it.
_Avoid_: Link, handle

### Data

**Connection**:
A saved database that the user can attach to scripts.
_Avoid_: Data source, database

**Database client**:
The ready-connected `db` object a script receives when a connection is attached. It sends SQL and returns rows.
_Avoid_: Driver, handle

**Secret**:
A named sensitive value (password, API key) kept in the operating system's password vault and never written into a script.
_Avoid_: Credential, password, key

**Data context**:
Typed code generated from a connection's schema, in which tables and their relationships appear as objects the editor understands.
_Avoid_: ORM model, schema classes

### Agents

**Agent**:
An AI coding tool the user already runs, such as Claude Code, that starts `cpad mcp` and uses CodingPad through MCP. CodingPad itself contains no AI.
_Avoid_: Assistant, bot, copilot

**Agent run**:
A script run requested by an agent. It runs headless and, when the app is open, also shows in the window as a read-only run.
_Avoid_: Remote run, tool call

**Dump preview**:
The shortened dump an agent gets back from a run: the top tree levels and the first rows of each table, with ids for fetching the rest.
_Avoid_: Summary, truncated dump

**Agent credential**:
An optional second vault credential for a connection, pointing at a read-only database role, that agent runs use instead of the main credential.
_Avoid_: Agent password, service account
