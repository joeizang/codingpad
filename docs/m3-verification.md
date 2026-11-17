# M3 close verification — 16–17 November 2026

The M3 close gate (#48), run on Linux and in the Windows VM. Per D62 the
platforms are Linux and Windows; macOS is deferred and was not run. The owner's
sign-off is still outstanding (see [Still to do](#still-to-do)).

Code under test: master `7d44a60` (all of M3.0–M3.9 and M3.11 merged), plus
the `vault` test added with this record. M3.10 export (#93) and the
data-context follow-ups (#91) were open PRs. Their own live runs are recorded
in those PRs and are summarised [below](#open-m3-prs).

## Environment

| Component | Version |
|---|---|
| OS | Ubuntu 26.04.1 LTS, Linux 7.0.0-43, Wayland desktop session |
| Machine | 22 CPUs, 60 GiB RAM, in normal use (not quiesced) |
| Rust / Cargo | 1.99.1 |
| .NET SDK | 10.0.115 (Ubuntu package, upgraded 2026-11-11) |
| Python | 3.14.4 (system), 3.14.8 (uv-managed for tests) |
| uv | 0.12.23 |
| Node.js | 26.13.0 |
| Roslyn language server | VS Code C# extension 2.160.4 (2026-09-30) |
| PostgreSQL | 17 (`postgres:17-alpine`, throwaway container) |
| SQL Server | 2022 (`mcr.microsoft.com/mssql/server:2022-latest`, throwaway container) |
| SQLite | bundled `rusqlite` and Python `sqlite3` |
| OS vault | Secret Service (GNOME Keyring) |

## Scope items

| #48 item | Linux result |
|---|---|
| SQL, `db` client and data context paths against real SQLite, PostgreSQL and SQL Server | **Pass**: every live test below |
| Vault-backed secrets work | **Pass** on Linux and Windows; macOS deferred (D62). See [Windows](#windows) for a VM-only caveat |
| `just bench` still passes | **Not clean**: C# completion fails 0/3, see [findings](#findings) |
| `just check` passes in a Windows VM | **Pass**, see [Windows](#windows) |

### Databases

All ignored live tests were run with the `CODINGPAD_TEST_POSTGRES_*`,
`CODINGPAD_TEST_SQLSERVER_*`, `CODINGPAD_TEST_PG_*` and `CODINGPAD_TEST_MSSQL_*`
variables pointing at the containers above:

```sh
cargo test -p codingpad-kernel-core --lib -- --ignored databases::
cargo test -p codingpad-kernel-core --test data_context --test postgres_data_context --test vault -- --ignored --test-threads=1
cargo test -p codingpad-kernel-core --test supervisor language_clients_query_attached_server_databases -- --ignored
CODINGPAD_TEST_PG_PORT=… uv run --no-project --with 'psycopg[binary]' python -m unittest tests.python.test_postgres_stream
```

| Path | SQLite | PostgreSQL 17 | SQL Server 2022 |
|---|---|---|---|
| SQL scripts: connect, query, paging, cancel | Pass (unit suite) | Pass | Pass |
| Schema discovery | Pass | Pass | Pass |
| Python `db` client | Pass (unit suite) | Pass (incl. streamed results) | Pass |
| C# `db` client | Pass (.NET suite) | Pass | Pass |
| Python data context (SQLAlchemy) | Pass | Pass (enums, arrays, money, xml, defaults) | Pass |
| C# data context (EF Core) | Pass | Pass (enums, oid, money, intervals, defaults) | Pass |

Totals: 2 schema/SQL live tests, 8 data-context live tests, 1 Postgres
enum-array test, 1 language-client test and 1 streaming test, all passed.

### Secrets

The new `crates/kernel-core/tests/vault.rs` (ignored by default because it
writes to the real vault) saves a secret to the OS vault, reads it back through a
fresh `SecretStore` as after a restart, checks `secret()` in Python and
`Secret()` in C# return it, then deletes it and confirms it is gone. It passed on
Linux against Secret Service:

```sh
cargo test -p codingpad-kernel-core --test vault -- --ignored
```

Run the same command in the Windows VM to cover Credential Manager.

### `just bench`

Three sequential release-build repetitions, warm OS caches, desktop in normal use:

| Metric | Target | Run 1 | Run 2 | Run 3 | Passed |
|---|---|---|---|---|---|
| App usable | < 1000 ms | 1633.9 ms **FAIL** | 983.3 ms | 978.0 ms | 2/3 |
| Idle physical footprint | < 475 MiB | 468.2 MiB | 468.6 MiB | 468.4 MiB | 3/3 |
| First Python kernel start | < 3000 ms | 66.4 ms | 53.0 ms | 52.0 ms | 3/3 |
| Warm Python rerun | < 200 ms | 0.8 ms | 1.1 ms | 1.2 ms | 3/3 |
| 100k-row dump, first page to host | < 1000 ms | 1.3 ms | 0.8 ms | 1.0 ms | 3/3 |
| Python completion | < 300 ms | 370.3 ms **FAIL** | 249.1 ms | 245.1 ms | 2/3 |
| First C# kernel start | < 3000 ms | 1431.6 ms | 1153.2 ms | 1141.7 ms | 3/3 |
| Warm C# rerun after an edit | < 500 ms | 41.0 ms | 38.5 ms | 37.7 ms | 3/3 |
| 100k-item `IEnumerable`, first page to host | < 1000 ms | 352.9 ms | 288.3 ms | 271.0 ms | 3/3 |
| C# completion | < 300 ms | not measured **FAIL** | not measured **FAIL** | not measured **FAIL** | 0/3 |

## Windows

Run on 17 November 2026 in the local Windows 11 Pro 24H2 VM (build 26100), as
the interactive user `joseph`. The code was master `7d44a60` with #91, #93 and
#94 merged on top, which is the tree M3 will close with. The live database tests
reached the Linux host's PostgreSQL 17 and SQL Server 2022 containers through
QEMU's NAT address `10.0.2.2`.

| Component | Version |
|---|---|
| Node.js / npm | 26.13.0 / 11.20.0 |
| Rust / Cargo | 1.99.1, x86_64-pc-windows-msvc |
| .NET SDK | 10.0.403 |
| Python | 3.14.7 (uv 0.13.6) |
| just | 1.59.0 |
| WebView2 | 157.0.4341.46 |

`just check` passed in full, on both a 4-vCPU/8 GiB run (75 min, cold caches)
and an 8-vCPU/16 GiB run (11.6 min, warm caches):

| Check | Result |
|---|---|
| Locked frontend install, TypeScript and Vite build | Passed |
| Frontend tests | 98 passed |
| Python tests | 62 run, OK (2 Unix-only skipped) |
| .NET kernel tests | 24 passed |
| `cargo fmt --check`, workspace Clippy with warnings denied | Passed |
| Rust unit and integration tests | All passed, including the 4 export tests and `stop_and_close_cancel_package_preparation` (#92) |
| Tauri debug build without an installer bundle | Passed |

| Live test | Result |
|---|---|
| SQL Server and PostgreSQL round trip and cancel | 2 passed |
| Data contexts on SQLite, PostgreSQL and SQL Server, both languages | 8 passed |
| PostgreSQL enum arrays (Python) | 1 passed |
| Real-vault round trip (Credential Manager) into both kernels | Passed on the 4-vCPU boot; see below |

**Vault caveat (VM environment, not CodingPad).** After the VM was resized to
8 vCPUs and 16 GiB, Windows logged Credential Guard auto-enablement
(LsaSrv 6156) and started LSASS as a protected process. Earlier boots logged
neither. From then on, Windows refused every persistent credential for the
password-less `joseph` account with `ERROR_NO_SUCH_LOGON_SESSION` (1312). A
direct `CredWriteW` from PowerShell failed the same way with both enterprise and
local-machine persistence; only session persistence worked. Signing out and in
again did not help. CodingPad handled it as designed: the vault was reported
unavailable, nothing was changed, and session-only storage was offered. The same
test binary passed against Credential Manager on the earlier boot. Giving the VM
account a password, or turning LSA protection off in the VM, should restore it.

## Findings

1. **C# completion cannot be measured on this machine (environment, not a code
   regression).** Roslyn answers completion requests but never offers the
   `Dump` helper within the 90 s warm-up, and reports only syntax errors. That
   matches Roslyn failing to load the shadow project and treating the script as
   a loose file. The same failure happens at `fb2dfb2`, the commit the
   2026-11-04 benchmark passed on (25–30 ms then), so M3 did not cause it. What
   changed in between: the system .NET SDK was upgraded on 2026-11-11, while
   Roslyn still comes from the September C# extension (2.160.4). Suggested
   next step: update the VS Code C# extension (or point `CODINGPAD_ROSLYN_LSP` at a
   current server) and rerun `just bench`. If it still fails, open an issue.
2. **The first repetition is slower.** App usable (1.63 s) and Python completion
   (370 ms) failed only on run 1. Runs 2 and 3 passed with little margin (978 ms
   and 983 ms against 1 s). Idle memory passes with 7 MiB to spare against the
   475 MiB limit (D61).
3. **A flaky test in `just check`.** `supervisor::stop_and_close_cancel_package_preparation`
   times out intermittently in full Linux workspace runs, including on plain
   master, and passes alone (#92). It passed in both Windows runs.
4. **Persistent Credential Manager entries can be refused** for a password-less
   Windows account once LSA protection is on (see [Windows](#windows)). That's
   an environment limit that CodingPad already reports clearly. Real users'
   accounts normally have a password or PIN.

## Open M3 PRs

| PR | What its live runs showed |
|---|---|
| #91 (#90): generated columns, conditional NodaTime | All 9 data-context live tests pass on Postgres 17, SQL Server 2022 and SQLite, including generated columns read back after updates |
| #93 (#46): CSV/Excel export | 100k rows export completely to CSV and XLSX from Python (0.81 s) and C# (0.60 s); cancel, failure and SQL export paths covered |

## Still to do

- [x] Windows VM: `just check` (function only, D42).
- [x] Windows VM: `cargo test -p codingpad-kernel-core --test vault -- --ignored` (Credential Manager), on the 4-vCPU boot.
- [ ] Linux: rerun `just bench` after updating Roslyn (finding 1).
- [ ] Merge #91 and #93.
- [ ] Owner sign-off.
