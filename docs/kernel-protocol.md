# Kernel protocol v0 (M1 skeleton)

JSON-RPC 2.0 over UTF-8 stdio, one JSON object per line. The Rust supervisor starts
one warm Python process per open script, writes requests to stdin and reads stdout.
The kernel and its runtime are embedded in the Rust build and extracted to a
private temporary directory for the process lifetime; installed apps do not need
the source checkout or a registry package. Python starts with `-I -u -X utf8` and adds only
the bundled runtime path explicitly.

## Run

The supervisor allocates monotonically increasing integer request IDs per kernel:

```json
{"jsonrpc":"2.0","id":1,"method":"run","params":{"source":"print('hello')\ndump([1, 2])\n42"}}
```

A new globals dictionary includes `dump`, `__name__ = "__main__"`, a virtual
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
relative write order. Both streams are also drained before a script traceback is
reported. Native-only writes on separate streams still have independent readers
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
{"jsonrpc":"2.0","method":"dump","params":{"runId":1,"node":{"kind":"list","type":"list","items":[{"kind":"primitive","v":1},{"kind":"primitive","v":2}],"truncated":false}}}
{"jsonrpc":"2.0","method":"dump","params":{"runId":1,"node":{"kind":"primitive","v":42}}}
{"jsonrpc":"2.0","method":"run_finished","params":{"runId":1,"ok":true}}
{"jsonrpc":"2.0","id":1,"result":{"ok":true}}
```

Python exceptions, including `SystemExit` and `KeyboardInterrupt`, stream a plain
traceback to `stderr`, followed by `run_finished` and a response with `ok: false`.
A script failure does not discard the warm process. A process crash or malformed
protocol discards the kernel; the next run creates a fresh one. Unknown methods
return error `-32601`; invalid requests return `-32600`.

The desktop uses a Tauri IPC channel per invocation so notifications cannot be
missed between listener registration and execution. Each script serializes its
runs; different script kernels can run independently. Closing a tab drops/kills
its child process. Application exit signals all active runs to finish and kills
their children before leaving the event loop. Frontend notifications are batched
per animation frame and flushed at run completion. Stable output IDs are retained
when the 500-block limit evicts old output. Per-run generation checks discard late
notifications from an earlier invocation.

## Basic dump nodes

- `null`: Python `None`
- `primitive`: JSON-safe boolean or number
- `string`: text in `v` (also used for non-finite floats, oversized integers and
  depth/cycle markers, and bounded `repr()` fallbacks for values without an
  instance dictionary)
- `list`: list/tuple/set/frozenset with `type`, `items`, `truncated`
- `object`: dictionary or instance attributes with `type`, `fields` containing
  `{name, value}`, and `truncated`

Strings are limited to 10,000 characters, containers to 100 children and depth to
six, with a total budget of 1,000 nodes per dump. Repeated references are allowed;
only cycles in the current path are marked. Integers exceeding Python's decimal
conversion limit show a bit-count summary instead of failing the run.
Properties and arbitrary iterators are not evaluated. Outside the app, `dump()`
falls back to terminal pretty-printing.

This protocol is intentionally small and will evolve in #2 for lazy nodes, tables,
images, HTML, live dumps and clickable tracebacks. Cancellation follows in #3.
