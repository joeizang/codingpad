# Kernel protocol v1 (M1 full dump)

JSON-RPC 2.0 over UTF-8 stdio, one JSON object per line. The Rust supervisor starts
one warm Python process per open script, writes requests to stdin and reads stdout.
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
{"jsonrpc":"2.0","id":1,"method":"run","params":{"source":"print('hello')\ndump([1, 2])\n42"}}
```

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
adds `stopped` and `cacheCleared` booleans to the kernel's `ok` result. A forced stop sets both flags,
reports the cleared cache, and retains output already received. Cooperative Stop
sets only `stopped`, retaining the kernel and cache. A successful response remains
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
  An unknown or expired handle returns error `-32001` with a message telling the
  user to run the script again. The desktop shows that message in place of the
  preview. A handle is cached per value within a run, so paging and repeated
  cells reuse one registry entry.
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
are allowed; only cycles in the current path are marked. Properties and arbitrary
iterators are not evaluated. Outside the app, `dump()` falls back to terminal
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
