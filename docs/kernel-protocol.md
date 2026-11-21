# Kernel protocol v1

JSON-RPC 2.0 over UTF-8, one JSON object per line. The Rust supervisor writes
requests to the kernel's stdin. It reads responses from stdout for Python and C#
and from a dedicated loopback connection for TypeScript. Each open Python, C#
or TypeScript script keeps one warm kernel process. Rust starts a fresh process
for each run and the supervisor retains its dump snapshots.
The kernel and its runtime are embedded in the Rust build and extracted to a
private temporary directory for the process lifetime; installed apps do not need
the source checkout or a registry package. Python starts with `-I -u -X utf8` and adds only
the bundled runtime path explicitly. Before starting a kernel, the supervisor
parses PEP 723 metadata and asks `uv` to prepare a cached script environment. Only
the metadata and a fixed interpreter-discovery command are passed to `uv`; user
source still runs through the kernel protocol. Metadata changes replace the kernel.
Preparation progress uses `stream` notifications with `runId: 0`, reserved for
supervisor output before a kernel request exists. Startup failures reject the Run
invocation after streaming diagnostics. Stop/Close also cancel preparation.

## Run

The supervisor allocates monotonically increasing integer request IDs per kernel:

```json
{"jsonrpc":"2.0","id":1,"method":"run","params":{"source":"print('hello')\ndump([1, 2])\n42","secrets":{"token":"value"}}}
```

`params.secrets` is an optional object of named secret values resolved by the
host from the OS password vault before each run. The kernel keeps them in memory;
Python and TypeScript expose them through `secret("name")`, C# through
`Secret("name")`. They
are never written to stdout/stderr or included in dumps unless a script chooses
to do so.

`params.secret_errors` is an optional object of vault read errors keyed by
secret name. Kernels surface the stored error when the script requests that name.
`params.connection` is null or an object with `settings` (the saved connection)
and `password` (a resolved vault password or null). Reserved connection
passwords are excluded from `params.secrets`.

`params.eager` is an optional boolean for hosts without an expand gesture, such
as `cpad run`. When true, kernels materialise values they would otherwise leave
lazy: C# properties are read during the dump (a getter failure becomes an error
node), non-scalar table cells are inspected, and Python's depth limit rises from
six to twenty levels. Cycle detection, node budgets, table pages and path
expansion are unchanged. The desktop omits the flag and keeps deferred dumps.

A new globals dictionary includes `dump`, `cache`, `__name__ = "__main__"`, a virtual
`__file__` and a copy of builtins. Imported modules remain cached in the process.
The script is registered as `__main__` for dataclasses and other module-aware
libraries. The final expression is instrumented through the AST so it is evaluated once,
with original source locations preserved. A final `None` result is suppressed;
an explicit final `dump(...)` is not emitted twice. `input()` raises an actionable
script error rather than consuming protocol input.

Python and native/subprocess writes share one pipe per output stream, preserving
write order within that stream and naturally grouping fragments into read chunks.
Before a dump, explicit flush, or run completion, a private fence is written to
each pipe and acknowledged by its reader after preceding text has been sent.
Each fence includes a monotonically increasing sequence number; a waiter checks
that its own sequence has been acknowledged. A late acknowledgement of a timed-out
fence cannot release a newer flush.

Python-level writes are serialized. On a switch between stdout and stderr, the
previous stream is drained before writing to the next one, preserving their
relative write order. Both streams are also drained before a structured error is
reported. Ordering gates are acquired without waiting. Reentrant writes (including
signal handlers and finalizers) and writes on kernel I/O workers use a reentrant
[`SimpleQueue`](https://docs.python.org/3/library/queue.html#queue.SimpleQueue.put)
and a separate dispatcher instead of taking gates or writing to a
pipe the caller might itself be responsible for reading. A thread-local busy
guard also covers protocol serialization, fence waits and run cleanup. Queued
callback/concurrent output has best-effort ordering; ordinary sequential script
writes retain their ordering checks. Worker roles are identifiable before thread
startup and remain identifiable across run boundaries. Deferred sends are stopped
under the protocol lock before the run ends, so they cannot leak stale run IDs.

Native-only writes on separate streams still have independent readers
and no total ordering guarantee. Drain acknowledgements and reader joins each
wait at most 500 ms per stream if the reader stalls or a script changes descriptors
or leaves background work; ordering is best-effort after a timeout. Background
work that outlives a run is not supported.

Both protocol input and output use private, non-inheritable duplicate descriptors.
Fds 0 and 1 are permanently pointed at the null device outside capture, so child
stdin immediately sees EOF and delayed stdout cannot corrupt protocol framing.
During a run, fds 1 and 2 are redirected to capture pipes; Windows standard handles
are updated too. All message strings (including dump labels) replace lone
surrogates before JSON encoding so Rust can decode them. Notifications include:

```json
{"jsonrpc":"2.0","method":"stream","params":{"runId":1,"stream":"stdout","text":"hello"}}
{"jsonrpc":"2.0","method":"stream","params":{"runId":1,"stream":"stdout","text":"\n"}}
{"jsonrpc":"2.0","method":"dump","params":{"runId":1,"node":{"id":"n1","kind":"list","type":"list","items":[{"id":"n2","kind":"primitive","v":1},{"id":"n3","kind":"primitive","v":2}],"total":2,"next":null,"truncated":false}}}
{"jsonrpc":"2.0","method":"dump_update","params":{"runId":1,"nodeId":"n4","node":{"id":"n5","kind":"primitive","v":2}}}
{"jsonrpc":"2.0","method":"dump","params":{"runId":1,"node":{"id":"n6","kind":"primitive","v":42}}}
{"jsonrpc":"2.0","method":"run_finished","params":{"runId":1,"ok":true}}
{"jsonrpc":"2.0","id":1,"result":{"ok":true}}
```

Python exceptions, including `SystemExit` and `KeyboardInterrupt`, produce a
structured `error` dump node (see below), followed by `run_finished` and a response
with `ok: false`. A script failure does not discard the warm process. Unhandled
execution or cleanup exceptions are contained at the request boundary and return
error `-32603` with the exception type. The Rust supervisor displays the failure
and completes the run with `ok: false`, retaining the same Python process for the
next request. Interval timers are stopped before restoring idle streams so a
script's timer handlers cannot continue firing during later requests. Other
imported/module state remains warm.

A process crash or malformed protocol discards the kernel; the next run creates a
fresh one. Unknown methods return error `-32601`; invalid requests return `-32600`.

The desktop uses a Tauri IPC channel per invocation so notifications cannot be
missed between listener registration and execution. Each script serializes its
runs; different script kernels can run independently. Closing a tab drops/kills
its child process. Application exit signals all active runs to finish and kills
their children before leaving the event loop. Frontend notifications are batched
per animation frame and flushed at run completion. Stable output IDs are retained
when the 500-block limit evicts old output. Per-run generation checks discard late
notifications from an earlier invocation.

## Stop, Reset and cache

The supervisor sends cancellation as a notification targeting the active run ID:

```json
{"jsonrpc":"2.0","method":"cancel","params":{"runId":1}}
```

A dedicated Python input thread receives notifications while the main thread
executes a script. Cancellation sets an event registered when the run request is
received, so a Stop arriving before execution starts is retained. Wrong or
expired run IDs are ignored. While a script executes, cancellation signals the
main thread with `pthread_kill(SIGINT)` on Unix, or `interrupt_main()` elsewhere.
The SIGINT handler raises `KeyboardInterrupt("Run stopped")` only when the active
run's cancellation is pending and the stack still contains a script frame. Late
interrupts during cleanup are ignored. One entry check handles cancellation that
arrives before `exec` begins. Ordinary runs install no trace hook, preserving user
tracers and avoiding per-call/per-line overhead. Imported Python code is covered;
Unix signals also interrupt `sleep()`. Scripts may catch or ignore SIGINT, and
some native calls (including Windows sleep) may delay signal handling.

After two seconds without a run response, the supervisor kills the process and
discards it; the next Run starts a fresh kernel. The desktop `run_script` result
adds `stopped` and `cacheCleared` booleans to the kernel's `ok` result. A forced
stop sets `stopped` and retains output already received. It also sets
`cacheCleared` when it discards a warm kernel or clears a Rust disk cache.
Cooperative Stop sets only `stopped`, retaining the cache. A successful response remains
Finished even when Stop was requested concurrently. Idle Stop cannot cancel a
later run because new watch subscribers start with the current version consumed.

`reset_script` stops any active run and discards the kernel, clearing all handles,
imports and cached values. Kernel startup is deferred until the next Run, so a
missing toolchain cannot turn a completed Stop or Reset into a startup error.
The desktop offers Reset when idle and clears old output and pending updates. Stop and Reset operate on one script only.

The built-in `cache(key, factory)` helper stores successful factory results by
hashable key in the warm process. Cached `None` values count as hits. Exceptions
are never cached. Factories run outside the shared lock; concurrent misses may
compute more than once and the first stored result wins. Each script's process
owns its cache, which survives clean execution scopes and cooperative Stop, but ends on Reset, close or restart.

## Expanding values

Lazy nodes and pages use handles created by the kernel. Expand when the user
clicks or scrolls:

```json
{"jsonrpc":"2.0","id":9,"method":"expand","params":{"handle":"h2","cursor":"100","limit":100}}
{"jsonrpc":"2.0","id":9,"result":{"node":{"id":"n90","kind":"table","type":"list","columns":[{"name":"name"}],"rows":[[{"id":"n91","kind":"string","v":"row 100"}]],"total":100000,"next":"200","handle":"h2"}}}
```

- `handle` is required. `cursor` is an opaque string; for the current Python kernel
  it is a decimal row/child offset. `limit` is capped at 1,000 and defaults to 100.
- The result is a node of the same kind as the container, containing the requested
  page. Merge `items`, `fields` or `rows` and continue with its `next` cursor.
- A lazy node expands at depth zero, so a deep object shows another six useful
  levels with another lazy node at the end.
- Handles stay valid until the next run begins or the kernel is reset/restarted.
  C# live contents also expire when replaced by a live update.
  An unknown or expired handle returns error `-32001` with a message telling the
  user to run the script again. The desktop shows that message in place of the
  preview. Python caches handles per value within a run. C# caches them per dump,
  so paging and repeated cells share a registry entry without later dumps reusing
  earlier inspection results.
- If user code raises while a preview is built (for example a broken `__repr__`),
  the kernel answers `-32603` ("Could not expand this value: RuntimeError") and
  keeps serving. `BaseException` is contained too, so a preview cannot `sys.exit`
  the kernel.
- Expansion requests are answered when no run is active; the frontend only offers
  expansion for output from the most recent run.

## Dump nodes

Every node has an `id` string. IDs are prefixed with the run number (`3:n12`) so
values kept between runs cannot collide; the examples below abbreviate the
prefix. The desktop uses IDs to apply live updates without rebuilding the rest of
the tree.

- `null`: Python `None`.
- `primitive`: JSON-safe boolean or number.
- `string`: text in `v` (also used for non-finite floats, oversized integers,
  depth/cycle markers, and bounded `repr()` fallbacks for values without an
  instance dictionary).
- `list`: list/tuple/set/frozenset with `type`, `items`, `total`, `next` and
  `truncated`. A page holds at most 100 children; `next` (and `handle`) is present
  when more remain.
- `object`: dictionary or instance attributes with `type`, `fields` containing
  `{name, value}`, `total`, `next` and `truncated`. Objects with more than 100
  fields are paged the same way as lists.
- `table`: a sequence of records. `columns` is the union of the first 100 rows'
  keys (at most 32), `rows` contains up to 100 rows of cell nodes, `total` is the
  sequence length, and `next`/`handle` page through the rest. A record is a
  `dict` or an object with `vars()`. Cells are scalars, or `lazy` nodes when a
  value needs its own tree (for example a list column).
- `lazy`: `type`, `preview` and `handle`. Emitted when a value is deeper than six
  levels or the node budget runs out; expanding starts a fresh six-level view.
- `live`: wraps the current node of a `live()` value. `dump_update` notifications
  target the wrapped node's ID (not the wrapper), so the live badge and styling
  stay in place while the inner value changes.
- `image`: `mime`, `name` when it came from a file, and a `src` data URI. The
  desktop renders it with a bounded height.
- `html`: markup in `html`. The desktop renders it in an iframe with an empty
  `sandbox` attribute, so scripts and same-origin access are blocked.
- `error`: `type`, `message`, `traceback` and `causes`. `traceback` is a list of
  `{file, line, column, function, text, script}` frames. `script` frames point
  into the current script; clicking one moves the editor to that line. Imported
  library frames are shown but are not clickable. `causes` lists up to five
  `{type, message}` entries from `raise ... from ...` or an implicit context.

Strings are limited to 10,000 characters (1,000 in table cells), container pages
to 100 children, tables to 100 rows and 32 columns, HTML to 200,000 characters,
images to 4 MiB, nesting to six levels, and each dump or page to 1,000 nodes and
about 4 MiB of JSON text and image data (ASCII counts 1, other BMP characters 6
and astral characters 12, matching `ensure_ascii`). Images, HTML and strings that
do not fit become lazy nodes, so expanding starts with a fresh budget. Strings
cut at the 10,000-character (or 1,000 in a table cell) limit carry
`truncated: true` and `total`, and the desktop shows the marker. Repeated references
are allowed; only cycles in the current path are marked. The Python kernel does not evaluate properties or arbitrary
iterators; C# inspection and iterator rules are described below. Outside the app, `dump()` falls back to terminal
pretty-printing, and `html()` prints its markup.

The runtime library provides four helpers:

```python
from codingpad import dump, html, image, live

dump({"name": "Ada"})
dump(html("<b>trusted markup</b>"))
dump(image("plot.png"))          # or bytes/bytearray
status = live("starting")
dump(status)                     # the results panel shows "starting"
status.value = "done"            # updates that same block
```

A 100,000-row table sends only its first page, so the first rows appear well under
a second; scrolling fetches pages by offset. Browsers cap element height in the
tens of millions of pixels, so tables beyond roughly 500,000 rows cannot be
scrolled to the end in one webview; split or filter larger data. Cancellation
arrives in #3.


## C# kernel (M2.0–M2.3)

C# reuses `run`, `stream`, `dump`, `run_finished` and the final response. The host
selects the language explicitly and replaces the kernel when a tab changes
language. C# runs use fresh collectible assemblies inside a warm .NET process.
The compiler and `CodingPad` library are embedded in the Rust executable at build
time. At startup the host discovers and explicitly selects the newest installed
stable .NET runtime, 10 or later. Successful discovery and the extracted C# assets
are shared for the supervisor lifetime, including across kernel restarts. Failed
or cancelled preparation can be retried.

Each compile sends a `diagnostics` notification, including an empty array on
success, before any script output:

```json
{"jsonrpc":"2.0","method":"diagnostics","params":{"runId":1,"diagnostics":[{"message":"CS0103: The name 'missing' does not exist in the current context","line":2,"column":1,"endLine":2,"endColumn":8}]}}
```

Locations are one-based (end exclusive). Diagnostics also appear as `error` dump
nodes with script frames. Invalid source never executes; runtime errors produce
an error node and `ok: false`. Compile diagnostics belong to the submitted source
snapshot and must be discarded in the editor when the text changes. Python does
not emit this notification. The final value-returning top-level invocation is
instrumented only after the unmodified C# has passed validation; final `.Dump()`
calls, void calls, ref-struct results, and pointer results are excluded. Top-level
await is supported. Empty or type-only scripts are validated and complete as
successful no-ops when there is no explicit entry point. Script references contain
only the selected shared framework and the public CodingPad helper, excluding the
kernel and its private Roslyn dependencies.

C# supports lazy `expand` requests using the same run-scoped handles and expired
handle error (`-32001`). A new run disposes unfinished iterators and clears all
handles, even if compilation fails. Reset/close terminate the process and release
its resources. Each dump/live update starts an independent cache for handles,
sequences and property reads; paging or expanding a child reuses that dump's cache.
Lazy property handles cache both their first value and evaluation failure;
expansion failures are returned as `error` nodes so script stack frames remain
clickable. Object fields are paged in groups of at most 100, with six levels per
expansion. Public instance fields and readable non-indexed properties are inspected;
fields are read at dump time, while properties evaluate when first expanded,
possibly after the script has mutated the object. Static members and custom `ToString` are
not invoked. Cycles in the current field path become a string marker.

Every C# `IEnumerable` (except strings) becomes a table. At most 100 items are read
per request, without a lookahead item. The same enumerator and already-read items
are retained until the next run. Repeated pages do not advance it again; cursors
beyond the cached frontier are rejected as error nodes rather than traversing an
arbitrary number of items. Iterator failures are appended as a final error row,
and the iterator is disposed on completion, failure or the next run. A record
table uses up to 32 public members from the first item as its stable columns;
scalar/nested-sequence tables use a single `Value` column. Record properties are
evaluated and cached for requested rows within that dump; nested cells use lazy
handles. A new dump of the same enumerable creates a fresh iterator. Mutations
can invalidate an older dump's retained iterator, but not the fresh dump's cache.

The supervisor races each expansion request against Stop, Close (including
shutdown), and a ten-second timeout. Reset signals Stop before taking the kernel
lock. Cancellation or timeout drops the process, releases waiting operations, and
returns an actionable preview error; the next Run starts a fresh kernel. Queued
previews also subscribe to cancellation before acquiring the kernel lock.

For tables with an unknown length, C# adds `totalKnown: false`: `total` is the
number of cached rows **plus one placeholder row**. The viewer uses that row to
request the next page when scrolling and grows the viewport as new pages arrive.
When enumeration finishes, `totalKnown: true` and `total` is the exact row count.
An exact multiple of 100 requires one final empty request to discover completion.
The existing Python tables omit `totalKnown` and retain their known-length and
random-access behavior. Table page offsets remain aligned to 100 in the desktop.

C# strings are limited to 10,000 characters (1,000 in cells), with a shared
200,000-character text budget per dump/page. Object traversal stops near 1,000
nodes and remains pageable; a table can contain up to 100 × 32 bounded cell
nodes. Scalars and null remain visible even when the depth/node budget is
exhausted; the cut-off applies only to expandable values. Error messages and each
of five inner-exception summaries are capped at 10,000 characters; tracebacks
retain up to 100 script frames, using `script.cs` and one-based line numbers.
Internal kernel/runtime frames and their build paths are omitted.

The `CodingPad` library also provides `HtmlValue(markup)`,
`ImageValue(bytes, mime = "image/png", name = null)`, and `LiveValue(value)`.
Dump these with `.Dump()`. HTML uses the shared text budget and the viewer's
sandboxed iframe. Images accept PNG, JPEG, GIF, WebP and BMP with a 4 MiB limit
per image and per dump/page. Assigning `LiveValue.Value` emits `dump_update` for
each displayed instance during its originating run. Each displayed live target
owns a separate view for its current contents. Replacement disposes that view's
unfinished iterators, drops its page/property caches and handles, and unsubscribes
nested live values. Only the latest view can be expanded. Sibling values and
independent dumps retain their own handles and iterators. Iterator cleanup may
itself trigger an update; only the newest replacement is emitted. Remaining
subscriptions and views are released with the session. Background writes/updates are discarded after run completion.
C# `run.params.path` is an optional absolute saved-file path. The host forwards
it on each run, including after Save As; the kernel uses its parent for standard
NuGet configuration discovery. An absent/null path uses the host working directory.
Directives are parsed before restore/compile, using a shared model intended for
M2.4's shadow project. Invalid directives send line-based `diagnostics` and an
error node without executing source. NuGet restore uses the bundled client
libraries, standard settings/credentials/caches, and cancellation. Cache misses
immediately emit “Restoring C# packages…” in a stderr `stream` event, before
resolution or downloads. Installs/warnings also use stderr `stream` events;
failures are error nodes with
`ok: false` without duplicating restore errors in stderr. Successful restores
are memoised for the kernel lifetime by packages, Web SDK choice, directory,
configuration contents, environment credentials, and runtime. Hits validate that
resolved files still exist and emit no restore progress; failures/cancellation
are never memoised. Ranges/floating versions stay pinned until invalidation or
Reset. Unknown distro RIDs use portable platform/architecture (including musl)
fallbacks from the bundled graph. Dependency load contexts are allocated only
after successful asset discovery and only for previously unseen resolved graphs.
No `dotnet restore` or MSBuild process is launched for a script.
Resolved compile assets become Roslyn references; runtime/native/resource assets
load in a dependency context shared across unchanged graphs. Script assemblies
remain collectible. Web SDK scripts load the installed matching ASP.NET Core
shared framework without making it a prerequisite for ordinary C# scripts.

C# uses a synchronous stdin reader on the main thread and a serial dispatcher on
a dedicated worker thread, independent of the thread pool. Parsed JSON documents
are transferred to the worker and disposed after dispatch. The reader registers
a cancellation source as each run arrives. Notifications without an `id` are
handled immediately; wrong/expired run IDs are ignored. A `cancel` request with
an `id` goes through normal dispatch and receives error `-32601`. Roslyn parsing,
diagnostics, semantic queries and emit receive the same token as scripts' `Cancel`.
The supervisor retains its two-second forced-stop fallback.

Cancellation uses reusable dedicated workers, outside stdin and the thread pool.
A blocked callback can occupy one worker while another signals a later run;
excess workers retire after 30 seconds idle. New workers suppress execution-context
flow so they cannot retain the originating run's AsyncLocals. At completion,
including failures, the helper scope closes and the token cancels before
`run_finished` and its response. The request worker waits only for the cancellation
flag, never for callback completion. Sources are not explicitly disposed while
scripts may retain tokens or wait handles; GC reclaims released resources.
On stdin EOF all queued/active tokens cancel, dispatch drains for up to two seconds,
and an uncooperative dispatcher causes process exit. Dispatcher faults are observed
and terminate the process even when stdin is still open.

`Cache(key, factory)` lives in the shared CodingPad assembly. After compilation
succeeds, a small index of known collectible root types is purged; failed compiles
leave entries untouched. All other entries are inspected on first lookup in the
new run, never merely because an unrelated script executes. A generation marker
avoids repeated checks within that run. Inspection runs outside the cache lock;
results are applied only if the entry still matches. Previously unused nested
script values can keep their old assembly alive until lookup, Reset or close.

Storage is inspected without invoking properties or user iterators. Type proofs
propagate unsafe leaves through all parents before publishing their verdicts;
recursive sealed storage with no unsafe leaf is safe. This prevents an optimistic
cycle verdict from hiding an unsafe mutually recursive dependency. Type proofs
that grow beyond 1,024 nodes fall back to examining actual values. Per-run metadata
is released with the scope. There is no object-count cutoff: arrays are walked
incrementally, scalar/proven-safe values need no traversal, copies of struct
storage do not fill the identity registry, and reference cycles are tracked.
Every traversal honors cancellation. Collectible assemblies, including emitted
collectible assemblies, are recognized as script types.

Evicted keys record their reason in the active scope: script types, opaque
handles/callbacks or inspection failure. A successful store consumes and emits
its note exactly once. Failed factories leave it pending for a retry; scope
disposal clears unused notes. Concurrent misses run factories outside the lock,
with the first store winning; incompatible types never evict that winner.

`Task<T>` and `ValueTask<T>` factories use a strongly typed adapter cached per
factory return type. Reflection builds that delegate once, with no per-call
argument array or invocation. Scope and type checks run synchronously before an
awaitable is returned. Only a successful awaited payload is stored; a pooled
ValueTask source is consumed once, and hits wrap its retained result afresh.
Keeping the original `Cache<T>(string, Func<T>)` signature avoids ambiguous null
or throw-only factories. Pending completions must still pass the originating
scope's active check before storing. Hosted calls without an active scope fail,
including idle previews and late callbacks; standalone consumers need no scope.
Reset and forced Stop discard all cached values.

The package's `build/CodingPad.targets` adds global namespace/static imports for
direct consumers by default. Setting `CodingPadImplicitUsings=false` opts out,
including when declared in the consumer project itself: targets are evaluated
after its properties. Indirect dependents receive no imports. Host integration
APIs are internal, with unsigned friend access keyed by the assembly name
`CodingPad.Kernel`; this organizes the API and does not authenticate assemblies
using that name. Outside the kernel, `Cancel` is `CancellationToken.None` and
`Cache` lasts for that process. `Secret(name)` throws a clear
`KeyNotFoundException` when no value was supplied. Package acceptance checks build a local feed
and use `#:package CodingPad@0.1.0`, disabling AOT publishing to avoid downloading
an AOT compiler. Registry publication is a separate release action.

## TypeScript kernel

The Bun kernel sends the same `stream`, `dump`, `dump_update`, and
`run_finished` notifications and run response. On startup the supervisor passes
Bun a loopback port and a one-time token over stdin. Bun authenticates the response connection with the
token. Raw writes to file descriptors 1 and 2 go to the application's inherited
stdout and stderr and cannot corrupt the protocol.

Each open script keeps a warm Bun process. Each run gets a fresh worker in that
process. Since M5.5, the supervisor resolves Bun from `PATH`, `BUN_INSTALL/bin`
and `~/.bun/bin`, then from CodingPad's private copy under the app data folder.
The user's own Bun always wins, a one-click install is picked up by the next run
without restarting the app, and an interrupted private install is replaced on
retry. On x86-64 CPUs without AVX2 the installer chooses Bun's baseline build,
and a private copy that cannot start is removed so the next run can reinstall it.
The run completes after the top-level module settles and the initial
pages of any dumped iterables are ready. A worker with preview handles stays
available for `expand` and `export_rows`. The next run asks unfinished iterators
to close, then terminates the worker with a bounded wait and starts a fresh
one. Handles also expire on Reset and Close. Background work such as timers
and open sockets can remain active in a retained worker until the next run,
Reset or Close. Results receive output and live updates during the run, so
await asynchronous work whose output you want to see. Stop targets the active
worker and waits for it to exit. If the worker cannot exit within two seconds,
the Rust supervisor kills the kernel. Reset and Close also discard the process. An overlapping run request receives
error `-32000` ("Run already in progress").

`cache(key, factory)` is importable from `codingpad`. The Bun kernel, not the
per-run worker, owns the cache, so a value survives reruns while globals start
fresh each run. Keys and values must be structured-cloneable, and values are
cloned on every run including the first, so class prototypes, methods and
accessors are not preserved. Factory calls must return synchronously, failed
factories are not cached, and a cached `undefined` counts as a hit. Each worker
asks the kernel for a key over a `MessageChannel` and blocks on `Atomics.wait`
until the reply is queued. Cooperative Stop keeps the kernel and its cache;
Reset, Close, and a forced restart clear both.

The bundled `codingpad` module is available inside each worker. `run.params.path`
sets the base for file-relative imports from a saved tab. Unsaved tabs resolve
relative to a private scratch subdirectory. Run uses the editor snapshot without
writing to the script file.

The worker exposes the run's `db` and `secret()` as globals and exports from
`codingpad`. It opens the attached database on first use and closes it when the
run finishes. For SQL Server, `run.params.sql_server_driver` names the private
project directory from which the worker resolves `mssql`. The field is null for
other connections. The worker removes the credential fields from `workerData`
before loading user modules. `run.params.final_expression` supplies the optional
source range for the automatic dump.

Query dumps retain column headers for empty results. Duplicate names gain the
first unused suffix starting at `_2`, including collisions with earlier
suffixed names. Results keep 10,000 rows and mark the table `truncated` when an
additional row is present. SQLite iterates rows and PostgreSQL fetches up to
10,001 rows through a cursor. PostgreSQL statements that cannot run in a
cursor, such as `INSERT … RETURNING`, may still buffer their result. SQL Server
streams its result sets and discards rows beyond the limit while the request
finishes.

Since M5.3, scripts can use Bun's versioned npm imports, such as
`import { z } from "zod@3"`, with the same syntax in CodingPad and standalone Bun.
CodingPad reads imports, re-exports, and literal `import()` and `require()`
specifiers in the script snapshot. Declare any versioned dependencies used by
local helper modules in the main script too. CodingPad installs these packages
into a per-script shadow project when the editor opens the script or runs it. The
package names retain their versions so several versions can coexist and the
TypeScript language server can resolve their declarations. Bun uses its shared
package cache across projects and prefers cached packages when installing. A
rerun or Reset with unchanged dependencies reuses the installed packages.
Changing dependencies creates a new shadow project and restarts the kernel on
Run. Runs report when package installation starts. Install errors identify the
requested packages and include Bun's error output. Package lifecycle scripts
are skipped during installation. Use version strings that work across platforms,
such as `3`, `3.x`, `^3.20.0`, `~3.20.0` or `next`.
A versioned subpath defined only in a package's exports map may fail to
resolve in Bun workers.

Bun executes TypeScript with top-level `await` and runs scripts with type errors.
M5.4 attaches the TypeScript 7 native language server (`tsgo`) through the same
language-server bridge and pull-diagnostics loop as Roslyn. It opens the staged
document from the shared M5.3 shadow project, maps the bundled `codingpad`
declarations, and resolves the installed versioned packages directly. A new
shared project (a changed import set) restarts the server. A package install
failure falls back to a package-free project so helper types and plain
TypeScript still work. Type errors are editor-only and never block a run.
Console output and JavaScript writes to
`process.stdout` and
`process.stderr` become stream notifications. The host identifies the final
top-level expression, which the worker evaluates once. Automatic dumps skip
`undefined`. Syntax and runtime failures produce structured `error` nodes and
`ok: false`. Stack frames in the editor snapshot use the original source
locations, including for exceptions in the automatically dumped expression.

Arrays of records become tables with up to 32 columns inferred from the first
100 rows. Other arrays and object fields page in groups of at most 100, with
lazy nodes for deeper values and nested table cells. Object fields are read
from property descriptors. Accessor fields show an `[Accessor]` marker.
Built-in values such as dates and maps use bounded text previews.

Custom synchronous and asynchronous iterables become tables. Each dump reserves
its place in Results and sends a `dump_update` when the first page is ready.
Each preview pulls at most 100 items without lookahead, retains the iterator
and caches the items already read so a repeated page reuses those items.
Columns are fixed from the first page. Export uses those columns for iterables and discovers all
columns for arrays. The table uses the same unknown-total placeholder as C# until
the iterator finishes. Iterator failures appear as a final error row.

```typescript
import { dump, html, image, live } from "codingpad";

dump(html("<strong>Ready</strong>"));
dump(image("plot.png")); // Also accepts a Uint8Array or ArrayBuffer.
const status = live("Starting");
dump(status);
status.value = "Ready";
```

## Rust kernel

CodingPad builds the editor snapshot in a fresh Cargo shadow project. It uses
the installed stable Rust toolchain (1.85 or later for edition 2024) and a
bundled, dependency-free `codingpad` crate. Each run uses an offline debug
build to retain source information. Shared build caching is scoped to M6.4. Saved
scripts run with their directory as the working directory. The script on disk
is untouched.

```rust
use codingpad::dump;

println!("Hello from Rust");
dump("An explicit dump");
6 * 7
```

Declare crates in Cargo single-file frontmatter at the top of the script.
CodingPad fetches missing crates before building offline. Cargo reuses
downloaded crates in its local package cache. Relative path
dependencies are resolved from the saved script's directory.

```rust
---cargo
[package]
edition = "2024"

[dependencies]
serde_json = "1"
---

fn main() {
    println!("{}", serde_json::json!({ "ready": true }));
}
```

Save this example as `example.rs` and run it with
`cargo +nightly -Zscript example.rs` outside CodingPad.

An explicit `fn main()` runs as a normal Rust program. In a top-level script,
CodingPad automatically dumps a final expression without a semicolon if it
returns a non-unit value. Dumped values need Rust's `Debug` implementation.
Compiler errors block execution and are sent as `diagnostics` and error dumps
with ranges mapped to the original editor snapshot. Stdout and stderr use
`stream` notifications. Dump messages travel over a separate authenticated
loopback connection, so native output can contain arbitrary text.

Use `codingpad::cache("key", || value)` to reuse a value across runs of
this script. The `codingpad` runtime library supports numeric types, booleans,
strings, vectors and options, and custom types can implement `CacheValue`.
Each script keeps up to 32 stored keys of at most 96 UTF-8 bytes, with up to
1 MiB per entry. A vector holds at most 1,048,576 elements. The limits bound storage even when scripts generate new keys
on each run. Oversize keys fail the script and report the actual size and limit.
Oversize values, encoding failures, and filesystem errors include the key.
Reset removes these values, and closing the script attempts the same cleanup.

Desktop cache entries belong to the open tab and survive reruns in that tab.
Reopening a saved script or restarting CodingPad starts with an empty cache.
Hosts configured with a data directory can keep entries across host launches
when they reuse the same script ID. These hosts manage cleanup for retired
script IDs.

Stop makes `codingpad::is_cancelled()` return true so scripts can release
resources and exit. A script that completes without acknowledging Stop keeps
its exit result. After two seconds, CodingPad forcibly stops the process tree
and detaches that script's cache when it exists. Detached entries are no longer
available to the script. A filesystem error during final deletion is reported
in Results.

Derived `Debug` values render as structured data. Structs and data-bearing enum
variants show their fields, `None` appears as null, and collections of records
become paged tables. Other collections appear as lists or maps. Custom
`Debug` output that has no recognizable structure appears as text. The host
keeps captured values available for expansion and table export after the process
exits. Live updates appear during the run. Expansion and paging wait for the
run to finish. Handles expire on rerun, Reset, or Close. Large previews are bounded,
and a table or list shows a truncation marker if only a prefix could be captured.

```rust
use codingpad::{dump, html, image, live};

dump(html("<strong>Ready</strong>"));
dump(image("plot.png").unwrap()); // Use image_bytes for encoded image data.
let status = live("Starting");
dump(&status);
status.set("Ready");
```

`html` uses the existing sandboxed viewer. Images accept PNG, JPEG, GIF, WebP,
BMP and SVG up to 4 MiB. To update a Results block during the run, dump a
`Live` value by reference as its own block, then call `set`. Updates keep the
original block and replace its contents, including its preview handles.
Panics that fail the run appear as structured errors with clickable script
frames mapped back to the editor snapshot. Panics from worker threads and
recovered main-thread panics are visible on stderr.
