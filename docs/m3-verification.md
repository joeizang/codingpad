# M3 close verification — 16–17 November 2026

The M3 close gate (#48), run on Linux and in the Windows VM. Per D62 the
platforms are Linux and Windows; macOS is deferred and was not run. The owner's
sign-off is still outstanding (see [Still to do](#still-to-do)).

Code under test: master `7d44a60` (all of M3.0–M3.9 and M3.11 merged), plus
the `vault` test added with this record. M3.10 export (#93), the data-context
follow-ups (#91) and the Roslyn discovery fix (#95) were merged during the close
and are summarised [below](#m3-prs-merged-during-the-close). The Windows run
tested the tree with all of them merged.

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

| #48 item | Result |
|---|---|
| SQL, `db` client and data context paths against real SQLite, PostgreSQL and SQL Server | **Pass**: every live test below |
| Vault-backed secrets work | **Pass** on Linux and Windows; macOS deferred (D62) |
| `just bench` still passes | **Every target passes on Linux**, but idle memory and app usable sit at their limits and fail an occasional repetition (#96); see [`just bench`](#just-bench) |
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

It also passed against Credential Manager on Windows (see [Windows](#windows)).

### `just bench`

**First run, 16 November**: three sequential release-build repetitions, warm OS
caches, desktop in normal use. C# completion could not be measured (finding 1):

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

**Final run, 17 November**, after updating the C# extension to 11.9.41 and
fixing CodingPad's Roslyn discovery (#95), with no `CODINGPAD_ROSLYN_LSP`
override. The Windows VM was paused so the host was quiet:

| Metric | Target | Run 1 | Run 2 | Run 3 | Passed |
|---|---|---|---|---|---|
| App usable | < 1000 ms | 1001.5 ms **FAIL** | 978.9 ms | 986.8 ms | 2/3 |
| Idle physical footprint | < 475 MiB | 466.6 MiB | 478.0 MiB **FAIL** | 469.1 MiB | 2/3 |
| First Python kernel start | < 3000 ms | 51.7 ms | 49.7 ms | 54.9 ms | 3/3 |
| Warm Python rerun | < 200 ms | 0.7 ms | 0.6 ms | 1.9 ms | 3/3 |
| 100k-row dump, first page to host | < 1000 ms | 1.3 ms | 0.9 ms | 1.5 ms | 3/3 |
| Python completion | < 300 ms | 248.9 ms | 253.3 ms | 249.7 ms | 3/3 |
| First C# kernel start | < 3000 ms | 1179.9 ms | 1148.0 ms | 1156.9 ms | 3/3 |
| Warm C# rerun after an edit | < 500 ms | 37.1 ms | 37.5 ms | 37.1 ms | 3/3 |
| 100k-item `IEnumerable`, first page to host | < 1000 ms | 280.8 ms | 305.6 ms | 278.1 ms | 3/3 |
| C# completion | < 300 ms | 36.0 ms | 52.2 ms | 42.3 ms | 3/3 |

Across all 15 quiet-host repetitions on 17 November, app usable ranged
947–1003 ms and idle memory 457–484 MiB. The memory spread is bimodal and comes
entirely from the desktop host process, which idles at either ~81 MiB or
~98 MiB (#96).

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
| Real-vault round trip (Credential Manager) into both kernels | Passed; see below |

**How to run the vault test on Windows.** Start it from the signed-in user's
desktop (for example Win+R or a terminal), not from a scheduled task. On the
resized VM, a scheduled task registered with the user's interactive principal
had no credential set: `CredWriteW` failed with `ERROR_NO_SUCH_LOGON_SESSION`
(1312) for enterprise and local-machine persistence, even when called directly
from PowerShell, while the same call from the desktop session succeeded.
Turning LSA protection off made no difference, and it was restored afterwards.
Run from the desktop, the vault round trip passed (4.7 s). CodingPad itself
behaved correctly in the failing context: it reported the vault as unavailable,
changed nothing, and offered session-only storage.

## Findings

1. **C# completion: fixed (#95).** After the system .NET SDK upgrade on
   2026-11-11, Roslyn from the September C# extension (2.160.4) stopped loading
   the shadow project, so completion never offered `Dump`. The same failure
   reproduced at `fb2dfb2`, so M3 didn't cause it. Updating the extension to
   11.9.41 was not enough on its own: CodingPad sorted extension folders as
   text and kept choosing 2.160.4. #95 orders them by version, and C# completion
   now measures 36–52 ms.
2. **Idle memory and app usable sit at their limits (#96).** On a quiet host,
   app usable ranged 947–1003 ms against 1 s. Idle memory ranged 457–484 MiB
   against 475 MiB, because the desktop host process idles at either ~81 MiB
   or ~98 MiB after the benchmark's warm-up work. Launched on its own, it stays
   at ~81 MiB. With the Windows VM running alongside, app usable reached 1.16 s.
3. **A flaky test in `just check`.** `supervisor::stop_and_close_cancel_package_preparation`
   times out intermittently in full Linux workspace runs, including on plain
   master, and passes alone (#92). It passed in both Windows runs.
4. **Run the Windows vault test from the desktop session.** In a scheduled
   task, Windows gave the process no credential set (see [Windows](#windows)).

## M3 PRs merged during the close

| PR | What its live runs showed |
|---|---|
| #91 (#90): generated columns, conditional NodaTime | All 9 data-context live tests pass on Postgres 17, SQL Server 2022 and SQLite, including generated columns read back after updates |
| #93 (#46): CSV/Excel export | 100k rows export completely to CSV and XLSX from Python (0.81 s) and C# (0.60 s); cancel, failure and SQL export paths covered |
| #95: Roslyn discovery picks the newest C# extension | C# completion 36–52 ms in `just bench` |

## Still to do

- [x] Windows VM: `just check` (function only, D42).
- [x] Windows VM: `cargo test -p codingpad-kernel-core --test vault -- --ignored` (Credential Manager).
- [x] Linux: rerun `just bench` after updating Roslyn (finding 1, #95).
- [x] Merge #91, #93 and #95.
- [ ] Owner sign-off, including whether the benchmark margins in #96 block M3.
