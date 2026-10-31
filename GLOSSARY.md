# CodingPad

CodingPad is an open-source, polyglot scratchpad: write a script in Python, C#, Rust or TypeScript, run it instantly, and explore the results, including data from your databases.

## Language

### Scripts

**Script**:
A single file of code in one language that the user writes, runs and saves in CodingPad.
_Avoid_: Query, snippet, pad

**Script header**:
The single `codingpad:` comment line at the top of a script that holds CodingPad-only settings, such as the attached connection.
_Avoid_: Metadata, sidecar

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
A hidden project CodingPad keeps behind a script so that a language's tools can build it and understand it. The user never edits it.
_Avoid_: Hidden project, workspace

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
