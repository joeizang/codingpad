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

Python text output is emitted immediately. Native file-descriptor and subprocess
output is captured through pipes and drained before completion (up to 500 ms per
stream if a background subprocess retains a pipe). Background work that outlives
the run is not supported. The protocol uses a dedicated duplicate of stdout so
native writes cannot corrupt JSON framing. Notifications include:

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
their children before leaving the event loop.

## Basic dump nodes

- `null`: Python `None`
- `primitive`: JSON-safe boolean or number
- `string`: text in `v` (also used for non-finite floats, oversized integers and
  depth/cycle markers)
- `list`: list/tuple with `type`, `items`, `truncated`
- `object`: dictionary or instance attributes with `type`, `fields` containing
  `{name, value}`, and `truncated`

Strings are limited to 10,000 characters, containers to 100 children and depth to
six, with a total budget of 1,000 nodes per dump. Repeated references are allowed; only cycles in the current path are marked.
Properties and arbitrary iterators are not evaluated. Outside the app, `dump()`
falls back to terminal pretty-printing.

This protocol is intentionally small and will evolve in #2 for lazy nodes, tables,
images, HTML, live dumps and clickable tracebacks. Cancellation follows in #3.
