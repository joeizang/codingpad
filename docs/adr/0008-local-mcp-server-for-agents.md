---
status: accepted
---

# A local, stdio-only MCP server so the user's own agents can use CodingPad

CodingPad ships an MCP **server**, `cpad mcp`, before it ships any AI of its own (D64). An agent the user already runs, such as Claude Code, Cursor or a local model, starts `cpad mcp` as a child process and uses CodingPad as a polyglot sandbox: it runs scripts in any installed language, queries the database connections the user has allowed, reads the library, and gets rich dumps back. CodingPad itself still contains no model and needs no API key.

## Decisions

- **Local only (D65).** The only transport is stdio. There is no HTTP or SSE transport and no listening port, so nothing on the network can reach the server; agents run as the user, on the user's machine. When the app is open, `cpad mcp` connects to it over a Unix domain socket (Linux) or a named pipe (Windows) that only the current user can open, never a TCP port.
- **Headless first, and the window too when it is open (D66).** `cpad mcp` owns its kernels, like `cpad run`, and works with no GUI. When the app is running, each agent run also appears in the window as a read-only *agent run* so the user can explore the dump.
- **Code runs without approval; databases and secrets are gated (D67).** The agent's own permission system already decides whether it may run code, so CodingPad adds no per-run prompt. What is new is access to the user's connections and vault: every connection is **off for agents** until the user turns it on, and `secret("name")` always fails in agent runs. Agent runs get a connection handle, never the credential.
- **Read-only in two layers (D68).** A connection allowed for agents can be marked read-only. By default CodingPad opens agent sessions read-only at the session level (Postgres `default_transaction_read_only`, SQLite `mode=ro`, SQL Server `ApplicationIntent=ReadOnly` with a read-only transaction). For a hard guarantee, the user can store a separate **agent credential** in the vault for the connection, pointing at a read-only database role; agent runs then use it instead of the main credential.
- **Previews, then fetch on request (D69).** `run_script` returns stdout and stderr (truncated past a limit), any structured error, and a **dump preview**: the top levels of each tree and the first ~20 rows of each table with the total count, plus a `dump_id` and node ids for collapsed parts. `expand_node` and `get_rows` fetch more on request, reusing the protocol's lazy nodes and paging. Dumps live for the MCP session, with an LRU limit.
- **Library: read everywhere, write to one folder (D70).** Agents can list and read scripts in the default library and the pinned folders. They can save only to an `Agent` folder inside the default library, created on first use, and cannot overwrite or delete anything outside it.

## First tool set

`run_script(language, source | path, connection?)`, `expand_node(dump_id, node_id)`, `get_rows(dump_id, table_id, offset, limit)`, `list_connections()` (only connections allowed for agents), `get_schema(connection)`, `list_library()`, `read_script(path)`, `save_script(name, language, source)` (into `Agent/`). Exact names and shapes are settled in the milestone's first issue.

## Considered options

- **MCP client first.** Useful only once there is an in-app assistant (D15) or a feature that browses MCP servers like data sources. Both come later; the client is not rejected, only deferred.
- **HTTP/SSE transport.** Rejected: it would let remote or hosted agents find and call CodingPad, which breaks the local-only guarantee that justifies running code without approval.
- **Approval prompt for every run.** Rejected: it breaks headless use, duplicates the agent's own permissions, and trains users to click Allow.
- **Full dump in one response, truncated.** Rejected: large dumps flood the agent's context, and anything past the limit is unreachable without rerunning.
- **Read-only or full write library access.** Read-only stops agents handing scripts to the user; full write risks overwriting plain files that may not be in git.

## Consequences

- A new milestone, **MCP server (0.3)**, sits between M5 (TypeScript) and M6 (Rust). Rust becomes available to agents automatically when M6 lands.
- D15 is amended: the MCP server is in scope before 1.0; an MCP client arrives with the in-app AI.
- Connection settings gain an "Allow agents" switch, a read-only switch, and an optional agent credential.
- Running `ssh host cpad mcp` still works, but only for someone who already has a shell on that host, so it grants nothing new.
