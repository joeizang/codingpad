---
status: accepted
---

# A local, stdio-only MCP server so the user's own agents can use CodingPad

CodingPad ships an MCP **server**, `cpad mcp`, before it ships any AI of its own (D64). An agent the user already runs, such as Claude Code, Cursor or a local model, starts `cpad mcp` as a child process and uses CodingPad as a polyglot sandbox: it runs scripts in any installed language, queries the database connections the user has allowed, reads the library, and gets rich dumps back. CodingPad itself still contains no model and needs no API key.

## Decisions

### Local only, with stdout reserved for MCP (D65)

The only transport is stdio. There is no HTTP or SSE transport and no listening port, so nothing on the network can reach the server; agents run as the user, on the user's machine. When the app is open, `cpad mcp` connects to it over a Unix domain socket (Linux) or a named pipe (Windows) that only the current user can open, never a TCP port.

The MCP stdio transport requires that the server writes nothing but MCP messages to its stdout. So in `cpad mcp`:

- No kernel or child process inherits the server's stdout or stderr. Every child is started with both piped to the supervisor. This includes the Bun kernel, which today lets raw writes to file descriptors 1 and 2 reach the app's inherited stdout (see [kernel-protocol.md](../kernel-protocol.md#typescript-kernel)); in MCP mode those raw writes are captured and become `stream` output of the run, like `print`/`console.log`.
- The server's own logs go to stderr only (which MCP allows), never stdout.
- No language servers, installers or other helpers run under `cpad mcp` unless their output is piped the same way.
- An acceptance test runs, in every language, a script that writes raw bytes to file descriptor 1 and 2, and checks that the MCP stream stays valid and the bytes appear in the run's output.

### Headless first, and the window too when it is open (D66)

`cpad mcp` owns its kernels, like `cpad run`, and works with no GUI. When the app is running, each agent run also appears in the window as a read-only *agent run* so the user can explore the dump.

### Code runs without approval; credentials never enter agent code (D67)

The agent's own permission system already decides whether it may run code, so CodingPad adds no per-run prompt. What is new is access to the user's connections and vault: every connection is **off for agents** until the user turns it on, and `secret("name")` always fails in agent runs.

Today a run with a connection reads the password in the supervisor and sends it to the kernel inside the run request (`kernel-core` builds `{"settings", "password"}`), and the Python runtime keeps that connection in module state that script code can reach. Agent runs must not do this. Instead:

- **The supervisor proxies all database access for agent runs.** The kernel receives only an opaque handle (the connection's name plus a per-run token). The `db` client, in agent mode, sends each query to the supervisor over the kernel protocol (a new `db_query` request). The supervisor reads the credential from the vault, opens and owns the database connection, applies the read-only guard (D68), runs the statement and returns the rows as dump table pages. No password, connection string or driver connection object ever exists inside the kernel process or its environment.
- **Typed data contexts (Level 3, D9) are not available in agent runs** in the first version, because EF Core and SQLAlchemy need a live driver connection inside the script process. Agents use the Level 2 `db` client and the `get_schema` tool. A later version could add a supervisor-side proxy driver for them.
- **The kernel process starts with a clean environment**: no vault values, no connection strings, and no CodingPad settings beyond what the run needs.

This keeps credentials out of everything CodingPad hands to agent code. It does not stop an agent that already has shell access from reading the OS vault directly with the user's own tools: that agent already had that power without CodingPad (the same reasoning as D65).

### Read-only: a guard by default, enforced only by database permissions (D68)

A connection allowed for agents can be marked read-only. There are two levels, and the UI names them honestly.

**Guarded (the default).** This protects against an agent's mistakes, not a hostile script. Because the supervisor owns the connection (D67), the script never controls the session:

| Database | Guard |
|---|---|
| PostgreSQL | Each agent statement runs alone, through the extended query protocol (one statement, no `;`-joined batches), inside `BEGIN TRANSACTION READ ONLY` … `ROLLBACK`. `SET` and `SET TRANSACTION` statements are rejected before they reach the server, so the script cannot switch the transaction back to read-write. |
| SQL Server | T-SQL has no read-only transaction mode, and `ApplicationIntent=ReadOnly` only routes connections in an Availability Group; on an ordinary database it allows writes. So the guard is: each agent statement runs inside a transaction that the supervisor always rolls back, and statements that cannot run in a user transaction (for example `BACKUP`, `ALTER DATABASE`) fail. Some side effects are not transactional (for example `xp_cmdshell`, linked-server calls, sequence increments), which is why this is only a guard. |
| SQLite | The supervisor opens the file with `mode=ro` and `PRAGMA query_only = ON`. Because agent code runs as the user, it can still open the file itself and write to it; SQLite has no permissions to enforce read-only access, so a SQLite connection is never more than guarded. |

**Enforced.** This needs an optional **agent credential** in the vault: a second login for the connection that has only read permissions in the database. Agent runs then use it instead of the main credential, and the database itself refuses writes. The guard still runs on top. The settings dialog explains the permissions needed:

- **PostgreSQL**: a role with `CONNECT` on the database, `USAGE` on the schemas, and `SELECT` on the tables (plus `ALTER DEFAULT PRIVILEGES … GRANT SELECT` for future tables). The role owns nothing and has no `INSERT`, `UPDATE`, `DELETE`, `TRUNCATE` or `CREATE` grants. Add `ALTER ROLE … SET default_transaction_read_only = on` as a second guard.
- **SQL Server**: a login mapped to a database user that is a member of `db_datareader` only, and not of `db_datawriter`, `db_ddladmin` or `db_owner`, with no server roles. Add `DENY EXECUTE` if stored procedures could write.

An agent credential is **required** whenever the user needs a guarantee that agent runs cannot change data, which means any connection to a database they are not willing to have modified. CodingPad cannot tell which databases those are, so it does not force the choice. It shows "Read-only (guarded)" or "Read-only (enforced)" next to each connection that agents can use, and a guarded connection's tooltip states that a script can get past the guard.

### Previews, then fetch on request (D69)

`run_script` returns stdout and stderr (truncated past a limit), any structured error, and a **dump preview**: the top levels of each tree and the first ~20 rows of each table, plus a `dump_id` and node ids for collapsed parts. `expand_node` and `get_rows` fetch more on request, using the protocol's lazy nodes and paging.

**Keeping dumps pageable.** Kernel handles expire when the next run starts in the same kernel, or when it resets or restarts ([kernel-protocol.md](../kernel-protocol.md)). So the MCP server keeps track of which kernel produced each dump:

- The server keeps a small **pool of warm kernels per language** (default 2). A new run goes to a kernel whose dumps the agent no longer needs. If every kernel in the pool still holds live dumps, the oldest of those dumps is evicted and its kernel is reused.
- The server **keeps every page and node it has already returned**, so anything the agent has fetched stays readable after its kernel moves on.
- A request that needs a kernel handle that has since expired returns an `expired` error that tells the agent to run the script again. It never returns a page from a different run.
- Dumps live for the MCP session, within the pool limit and an LRU memory budget.

**Unknown totals.** C# and TypeScript iterables can have an unknown length until they are fully enumerated (`totalKnown: false` in the protocol, where `total` includes a placeholder row for the desktop). The MCP layer never passes that placeholder through. A preview or page states one of:

- `total: <n>`, when the length is known;
- `total: null, rowsSoFar: <n>, more: true`, while enumeration is still going;
- `total: <n>, more: false`, once `get_rows` reaches the end, when the total becomes known.

`get_rows` can page forward only from the end of the rows already enumerated, and the server caps each call (1,000 rows, like the protocol), so an endless sequence never hangs a request.

### Library: read everywhere, write to one folder (D70)

Agents can list and read scripts in the default library and the pinned folders. They can save only to an `Agent` folder inside the default library, created on first use, and cannot overwrite or delete anything outside it.

## First tool set

`run_script(language, source | path, connection?)`, `expand_node(dump_id, node_id)`, `get_rows(dump_id, table_id, offset, limit)`, `list_connections()` (only connections allowed for agents, with their read-only level), `get_schema(connection)`, `list_library()`, `read_script(path)`, `save_script(name, language, source)` (into `Agent/`). Exact names and shapes are settled in the milestone's first issue.

## Considered options

- **MCP client first.** Useful only once there is an in-app assistant (D15) or a feature that browses MCP servers like data sources. Both come later; the client is not rejected, only deferred.
- **HTTP/SSE transport.** Rejected: it would let remote or hosted agents find and call CodingPad, which breaks the local-only guarantee that justifies running code without approval.
- **Approval prompt for every run.** Rejected: it breaks headless use, duplicates the agent's own permissions, and trains users to click Allow.
- **Passing credentials to the kernel, as GUI runs do.** Rejected for agent runs: the script could read them and use them outside CodingPad's guard.
- **Session settings alone as "read-only".** Rejected as a guarantee: SQL Server's `ApplicationIntent` does not stop writes on an ordinary database, and a PostgreSQL session default can be overridden per transaction. Kept only as a guard.
- **Full dump in one response, truncated.** Rejected: large dumps flood the agent's context, and anything past the limit is unreachable without rerunning.
- **Read-only or full write library access.** Read-only stops agents handing scripts to the user; full write risks overwriting plain files that may not be in git.

## Consequences

- A new milestone, **MCP server (0.3)**, sits between M5 (TypeScript) and M6 (Rust). Rust becomes available to agents automatically when M6 lands. D33 and D34 are amended to include it.
- D15 is amended: the MCP server is in scope before 1.0; an MCP client arrives with the in-app AI.
- The kernel protocol gains a `db_query` request, from the kernel to the supervisor, and each language's `db` client gains an agent mode that uses it. GUI runs keep their direct connections.
- The Bun kernel gains a mode with its standard output piped, and the supervisor gains a per-language kernel pool for MCP.
- Connection settings gain an "Allow agents" switch, a read-only switch, and an optional agent credential.
- Running `ssh host cpad mcp` still works, but only for someone who already has a shell on that host, so it grants nothing new.
