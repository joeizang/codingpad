---
status: accepted
---

# A local, stdio-only MCP server so the user's own agents can use CodingPad

CodingPad ships an MCP **server**, `cpad mcp`, before it ships any AI of its own (D64). An agent the user already runs, such as Claude Code, Cursor or a local model, starts `cpad mcp` as a child process and uses CodingPad as a polyglot scratchpad and execution environment: it runs scripts in any installed language, queries the database connections the user has allowed, reads the user's scripts, and gets rich dumps back. CodingPad itself still contains no model and needs no API key.

This is **not a sandbox**. Agent scripts run with the user's full privileges, like every other CodingPad script ([03-architecture.md](../03-architecture.md#risks)). The protections below cover what CodingPad itself hands to agent code (credentials, connections, the library); they do not isolate the script from the machine.

## Decisions

### Local only, with stdout reserved for MCP (D65)

The only transport is stdio. There is no HTTP or SSE transport and no listening port, so nothing on the network can reach the server; agents run as the user, on the user's machine.

When the app is open, `cpad mcp` connects to it over a local channel that must reject every remote or other-user client:

- **Linux:** a Unix domain socket in `$XDG_RUNTIME_DIR` with mode `0600`. Unix sockets cannot be reached over the network. The app also checks the peer's user id (`SO_PEERCRED`) and refuses any other user.
- **Windows:** a named pipe created with `PIPE_REJECT_REMOTE_CLIENTS`, so connections arriving through the Server service (SMB) are refused even when the same user authenticates remotely. Its security descriptor grants access only to the current user's SID and denies the `NETWORK` SID (`S-1-5-2`). The app also checks the client with `GetNamedPipeClientProcessId` and refuses a process running as another user.
- Never a TCP port, including loopback.

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

### Read-only: a guard by default, enforced only by verified database privileges (D68)

A connection allowed for agents can be marked read-only. There are two levels, and the UI names them honestly.

**Guarded (the default).** This protects against an agent's mistakes, not a hostile script. Because the supervisor owns the connection (D67), the script never controls the session or the transaction. Two rules apply on every engine:

1. **Only queries are accepted.** The supervisor parses each request with a SQL parser for the connection's dialect (the Rust `sqlparser` crate) before anything reaches the server. A request must be exactly one statement, and that statement must be a query: `SELECT` (or `WITH … SELECT`), `VALUES`, `EXPLAIN` without `ANALYZE`, and PostgreSQL `SHOW`. Everything else is rejected with a clear error, including:
   - transaction control (`BEGIN`, `COMMIT`, `ROLLBACK`, `SAVE TRANSACTION`, `SET TRANSACTION`) and other `SET` statements;
   - data changes and DDL, including `SELECT … INTO`, `MERGE` and data-changing CTEs (`WITH x AS (DELETE …)`);
   - `EXEC`/`EXECUTE`, `CALL` and `DO`, because procedures can commit, change data, or run nested batches;
   - SQL Server `OPENQUERY`, `OPENROWSET` and `OPENDATASOURCE`, and PostgreSQL `dblink*` calls, which reach other servers on other connections.
   A request that the parser cannot parse is rejected, never passed through.
2. **The supervisor's transaction wraps the query, and the supervisor checks it afterwards.**

| Database | Wrapping transaction |
|---|---|
| PostgreSQL | The supervisor sends `BEGIN TRANSACTION READ ONLY`, then the query through the extended query protocol (which cannot carry several statements), then `ROLLBACK`. A function called from the query cannot write either, because PostgreSQL refuses writes to ordinary tables inside a read-only transaction, including in `SECURITY DEFINER` functions. |
| SQL Server | T-SQL has no read-only transaction mode, and `ApplicationIntent=ReadOnly` only routes connections in an Availability Group; on an ordinary database it allows writes. The supervisor runs `SET XACT_ABORT ON; BEGIN TRANSACTION`, then the query, then checks that `@@TRANCOUNT` is still 1 and rolls back. If the count has changed, the supervisor discards the connection and reports the request as a guard violation. Rule 1 is what keeps transaction control out; this check catches anything the parser missed. Scalar functions in a `SELECT` cannot modify data in SQL Server, but some effects are still outside any transaction (for example sequence values and extended stored procedures that a function reaches), which is why this is only a guard. |
| SQLite | The supervisor opens the file with `mode=ro` and `PRAGMA query_only = ON`, and rule 1 applies. Because agent code runs as the user, it can still open the file itself and write to it; SQLite has no permissions to enforce read-only access, so a SQLite connection is never more than guarded. |

**Enforced.** This needs an optional **agent credential** in the vault: a second login for the connection that is meant to have only read privileges. Agent runs then use it instead of the main credential, and the database itself refuses writes. The guard still runs on top.

Grant lists and role memberships are not enough to prove this: PostgreSQL grants `EXECUTE` on functions to `PUBLIC` by default, and SQL Server permissions can also come from the `public` role, direct grants, and ownership chains through procedures and views. So CodingPad **labels a credential "enforced" only after checking its effective privileges**, when the credential is saved and again at the start of every MCP session (grants change), on a connection made with that credential:

- **PostgreSQL.** The role must not be a superuser and must not have `CREATEROLE`, `CREATEDB`, `REPLICATION` or `BYPASSRLS`. Using `has_table_privilege`, `has_schema_privilege`, `has_database_privilege`, `has_function_privilege` and `has_foreign_data_wrapper_privilege`/`has_server_privilege` (all of which include privileges inherited through roles and `PUBLIC`), it must have no `INSERT`, `UPDATE`, `DELETE`, `TRUNCATE`, `TRIGGER` or `REFERENCES` on any table, view or sequence (`UPDATE` on a sequence allows `nextval`/`setval`), no `CREATE` on any schema or on the database, must own no objects, must not be a member of a role that owns objects, and must have no `USAGE` on foreign servers. It may hold `EXECUTE` on `SECURITY DEFINER` functions only in `pg_catalog` and `information_schema`; any other `SECURITY DEFINER` function it can execute fails the check, because it runs with its owner's rights. `ALTER ROLE … SET default_transaction_read_only = on` is recommended as a second layer, not relied on.
- **SQL Server.** Using `sys.fn_my_permissions` at server and database level and `HAS_PERMS_BY_NAME` for every schema, table, view, procedure and function (which together include permissions from `public`, role memberships and direct grants), the login must not be in any fixed server role, must not be `db_owner`, `db_datawriter` or `db_ddladmin`, and must have no `INSERT`, `UPDATE`, `DELETE`, `ALTER`, `CONTROL`, `TAKE OWNERSHIP`, `IMPERSONATE` or `CREATE …` permission on anything. It must have no `EXECUTE` on any user procedure or function, because ownership chaining lets a procedure change tables the caller cannot. It must not be the owner of any schema.

If any check fails, the connection is labelled **guarded**, and the settings dialog lists each write-capable path it found, with the statement that removes it. The usual setup the dialog suggests:

- **PostgreSQL:** a role with `CONNECT` on the database, `USAGE` on the schemas, `SELECT` on the tables (plus `ALTER DEFAULT PRIVILEGES … GRANT SELECT` for future tables), and `REVOKE EXECUTE ON ALL FUNCTIONS … FROM PUBLIC` for user-defined `SECURITY DEFINER` functions.
- **SQL Server:** a database user in `db_datareader` only, with `DENY EXECUTE` at the database level.

An agent credential is **required** whenever the user needs a guarantee that agent runs cannot change data, which means any connection to a database they are not willing to have modified. CodingPad cannot tell which databases those are, so it does not force the choice. It shows "Read-only (guarded)" or "Read-only (enforced)" next to each connection that agents can use, and a guarded connection's tooltip states that a script can get past the guard. The privilege check covers the write paths listed above; a path it does not know about (for example a third-party extension) is the database administrator's responsibility, and the dialog says so.

### Previews, then fetch on request (D69)

`run_script` returns stdout and stderr (truncated past a limit), any structured error, and a **dump preview**: the top levels of each tree and the first ~20 rows of each table, plus a `dump_id` and node ids for collapsed parts. `expand_node` and `get_rows` fetch more on request, using the protocol's lazy nodes and paging.

**Kernels follow D11: one warm kernel per script.** Every agent run has a **script key**: the file's path when it runs a script from disk, or for inline source the `session` name the agent passes (default `inline`, one per language). Each script key gets its own warm kernel, just like each open script in the GUI, so:

- `cache()` belongs to that script key only. A run never sees values another script cached, and repeated runs of the same script key keep their cache until it is reset or its kernel is evicted. An agent that wants inline snippets kept apart passes different `session` names.
- MCP kernels are separate processes from the GUI's, so an agent run never shares a cache with the user's open scripts.
- `cpad mcp` keeps at most 4 warm kernels (configurable). Starting a fifth evicts the least recently used one, which ends its cache, as Reset does.

**Keeping dumps pageable.** Kernel handles expire when the next run starts in the same kernel, or when it resets or restarts ([kernel-protocol.md](../kernel-protocol.md)). With one kernel per script key, a run only expires the dumps of the previous run of **the same script key**, which that rerun replaces. Runs of other scripts don't touch them. In addition:

- The server **keeps every page and node it has already returned**, so anything the agent has fetched stays readable after its kernel moves on.
- A request that needs a kernel handle that has since expired (a rerun of the same script key, Reset, or eviction) returns an `expired` error that tells the agent to run the script again. It never returns a page from a different run.
- Dumps live for the MCP session, within an LRU memory budget.

**Unknown totals and the enumeration frontier.** C# and TypeScript iterables can have an unknown length until they are fully enumerated (`totalKnown: false` in the protocol, where `total` includes a placeholder row for the desktop). The kernel enumerates in pages (Bun and C# pull up to 100 items for the first page) and caches what it has read, while the MCP preview shows only ~20 rows. The **frontier** is the number of rows the kernel has enumerated so far.

- The MCP layer never passes the placeholder row through. A preview or page states one of:
  - `total: <n>`, when the length is known;
  - `total: null, rowsSoFar: <frontier>, more: true`, while enumeration is still going;
  - `total: <n>, more: false`, once enumeration ends and the total becomes known.
- `get_rows` can read **any range below the frontier**, in any order, because those rows are already cached; rows 20–99 of a first page are always reachable.
- Only a request that reaches past the frontier advances the iterator, and enumeration only moves forward. One call advances the frontier by at most 1,000 rows (the protocol's page cap), so an endless sequence never hangs a request. If the requested range starts further ahead than that, the call returns no rows with the new `rowsSoFar` and `more: true`, and the agent calls again.

### Library: read the library and pinned folders, write to one folder (D70)

Agents can list and read scripts in the **library** and in the user's **pinned folders**, which are outside the library (see [GLOSSARY.md](../../GLOSSARY.md)). They can save only to an `Agent` folder inside the library, created on first use, and cannot create, overwrite or delete anything anywhere else, in the library or in a pinned folder.

## First tool set

`run_script(language, source | path, session?, connection?)`, `expand_node(dump_id, node_id)`, `get_rows(dump_id, table_id, offset, limit)`, `list_connections()` (only connections allowed for agents, with their read-only level), `get_schema(connection)`, `list_scripts()` (the library and pinned folders), `read_script(path)`, `save_script(name, language, source)` (into `Agent/`). Exact names and shapes are settled in the milestone's first issue.

## Considered options

- **MCP client first.** Useful only once there is an in-app assistant (D15) or a feature that browses MCP servers like data sources. Both come later; the client is not rejected, only deferred.
- **HTTP/SSE transport.** Rejected: it would let remote or hosted agents find and call CodingPad, which breaks the local-only guarantee that justifies running code without approval.
- **Approval prompt for every run.** Rejected: it breaks headless use, duplicates the agent's own permissions, and trains users to click Allow.
- **Passing credentials to the kernel, as GUI runs do.** Rejected for agent runs: the script could read them and use them outside CodingPad's guard.
- **Session settings alone as "read-only".** Rejected as a guarantee: SQL Server's `ApplicationIntent` does not stop writes on an ordinary database, and a PostgreSQL session default can be overridden per transaction. Kept only as part of the guard.
- **Trusting role memberships to mean "enforced".** Rejected: default `PUBLIC` execute grants, `public` role permissions and ownership chains leave write paths open. Only a check of effective privileges earns the label.
- **A per-language kernel pool.** Rejected: reusing a kernel across scripts would leak one script's `cache()` into another and break D11.
- **Full dump in one response, truncated.** Rejected: large dumps flood the agent's context, and anything past the limit is unreachable without rerunning.
- **Read-only or full write library access.** Read-only stops agents handing scripts to the user; full write risks overwriting plain files that may not be in git.

## Consequences

- A new milestone, **MCP server (0.3)**, sits between M5 (TypeScript) and M6 (Rust). Rust becomes available to agents automatically when M6 lands. D33 and D34 are amended to include it.
- D15 is amended: the MCP server is in scope before 1.0; an MCP client arrives with the in-app AI.
- The kernel protocol gains a `db_query` request, from the kernel to the supervisor, and each language's `db` client gains an agent mode that uses it. GUI runs keep their direct connections.
- The supervisor gains a SQL parser for guarded queries, a privilege checker for agent credentials, and a script-keyed kernel cache for MCP.
- The Bun kernel gains a mode with its standard output piped.
- Connection settings gain an "Allow agents" switch, a read-only switch, an optional agent credential, and the privilege-check report.
- Running `ssh host cpad mcp` still works, but only for someone who already has a shell on that host, so it grants nothing new.
