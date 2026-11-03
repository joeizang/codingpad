# M1 measurement and profiling investigation

M1 remains open. The corrected measurements and the first optimizations do not
meet the idle-memory target, and macOS qualification has not been performed.
The original November 3 failure record is retained in
[the milestone verification file](../../milestone-verification-2026-11-03.txt).

## Contract and scenarios

The owner explicitly chose physical footprint for the 300 MiB gate (D61).
Linux sums PSS from `smaps_rollup`; macOS requires `phys_footprint` and never
substitutes RSS. RSS remains informational. The process set now includes
language-server descendants in separate process groups. Those were missing from
the old Linux RSS sum. All these figures include a resident Python language
server; it is not suspended or removed to make the idle measurement pass.

The old two-frame marker could fire before Monaco mounted. The new marker
requires a writable mounted model, connected editor DOM, installed command and
completion/hover handlers, and two animation frames. Language-server readiness
is separate, as it is in the UI. The native smoke test waits for the marker,
types through WebDriver, and executes the edited script.

Completion warms `import os\nos.path.` at line 1, character 8, then measures
changing it to `import os\nos.path.j` and requesting completion at character 9.
Both responses must include `join`; null, empty, malformed, and irrelevant lists
fail. Timing includes didChange and there is no diagnostic barrier or sleep
before the measured request. The original `os.path.join(` scenario requested
argument-position suggestions and accepted even an empty list.

## Environment and reproducibility

Base: PR #32, commit 0657e88. Runtime: native Ubuntu 26.04.1, x86_64,
Linux 7.0.0-42-generic, Intel Core Ultra 7 155H (22 logical CPUs), 60 GiB RAM,
X11 desktop. Node 24.19.0 from the basedpyright wheel; basedpyright 1.40.2
(based on pyright 1.1.414). Frontend built with Node 24.21.0. Desktop release
builds used the existing `codingpad-check-csharp` container because the host
lacks GTK/WebKit development headers. The benchmark and desktop ran natively
on Ubuntu, not under Xvfb or in that container.

Each file records sequential warm-cache launches of a fixed release binary.
No build or test ran concurrently with the instrumented before/after batches.
The desktop session, browsers and Windows QEMU VM remained running; the host
was not dedicated to these tests. CPU pressure was near zero at batch start,
but these observations are not a controlled-hardware qualification or a cold
OS-cache test. Downloads were cached. Neither timings nor footprint values
from this machine establish macOS acceptance.

Commands (the desktop build used `tauri/custom-protocol` to embed assets):

```sh
npm --prefix apps/desktop ci
npm --prefix apps/desktop run build
# Inside the existing build container, with this checkout mounted at /workspace:
cargo build --release --locked -p codingpad-desktop --features tauri/custom-protocol
# Natively on the Ubuntu host:
CARGO_TARGET_DIR=/tmp/codingpad-bench-host cargo build --release --locked -p codingpad-kernel-core --bin codingpad-bench
CODINGPAD_LSP_TRACE=1 CODINGPAD_DESKTOP=/tmp/codingpad-milestones/target/release/codingpad-desktop /tmp/codingpad-bench-host/release/codingpad-bench
```

`just bench` performs the equivalent release builds and defaults to three full
repetitions. All repetitions affect the exit status; no fastest-run or median
selection can mask a failing sample. Use the same conditions on macOS and a
quiet Ubuntu desktop for milestone qualification.

## Findings and changes

[Original benchmark](original-benchmark.txt): the unmodified harness reported
709.6 ms startup, 498.0 MiB summed RSS, and 320.2 ms Python completion. Its
startup and memory values use the old measurement definitions.

[Instrumented before](instrumented-before.txt) is the first three-run batch
with corrected measurements, before the optimizations:

| Repetition | Editor usable | Physical footprint | Python completion |
|---|---:|---:|---:|
| 1 | 1055.2 ms | 458.4 MiB | 329.9 ms |
| 2 | 1135.5 ms | 457.7 MiB | 325.0 ms |
| 3 | 1115.1 ms | 459.5 MiB | 331.9 ms |

The bridge's document-change queueing, completion pipe writes and delivery
cost fractions of a millisecond. Almost all completion delay lies between the
pipe write and response receipt. That interval includes server CPU work and
OS scheduling, so it does not prove an internal CPU duration. An opt-in Node
CPU profile showed tokenization and garbage collection dominating. Inspection
of the installed basedpyright source map also showed eager deprecated-symbol
type evaluation when clients cannot resolve completion tags.

The client now advertises and handles resolved deprecation tags, allowing that
work to wait until selection. The public CodingPad stub avoids implementation
analysis and preserves generic `dump`/`cache` result types. Integration tests
exercise real diagnostics, hover, completions, inline packages and server reuse.

Startup traces showed about 600 ms between frontend evaluation and editor mount,
with several hundred milliseconds before Monaco creation began. Mounting the
already-bundled editor directly removes the asynchronous loader and extra React
commits. Code intelligence starts after the first usable paint, remains resident,
and is included in the five-second idle sample. Source changes retain undo
history; model/editor disposal and StrictMode replay are tested.

[Instrumented after](instrumented-after.txt), same native environment:

| Repetition | Editor usable | Physical footprint | Python completion |
|---|---:|---:|---:|
| 1 | 941.9 ms | 451.4 MiB | 262.5 ms |
| 2 | 969.9 ms | 451.2 MiB | 263.1 ms |
| 3 | 947.5 ms | 452.8 MiB | 277.4 ms |

Every batch still exits nonzero for memory. These changes produced a small
footprint reduction, not a memory fix. The process samples attribute roughly
164 MiB PSS to WebKit's web process and 163 MiB to basedpyright's Node process;
the app itself contributes about 61 MiB, with further WebKit and launcher costs.
The earlier 500 MiB summed-RSS result omitted the language server. Merely
changing the gate to PSS would therefore have hidden the accounting error.

Further memory work needs heap/allocation profiles for both JSC/Monaco and the
resident basedpyright process. The per-process PSS/private breakdown is not a
Monaco allocation profile. No editor functionality, diagnostics, or language
server has been disabled to obtain the reported results. Repeated passing
`just bench` runs on Ubuntu and macOS remain the acceptance requirement; Windows
verification from PR #32 does not close either timing gate.

## Final validation

[Final release repetition batch](final.txt), tracing disabled, after the full
check and smoke had finished:

| Repetition | Editor usable | Physical footprint | Python completion |
|---|---:|---:|---:|
| 1 | 940.1 ms | 450.2 MiB | 255.4 ms |
| 2 | 961.0 ms | 449.8 MiB | 257.5 ms |
| 3 | 952.5 ms | 453.1 MiB | 262.3 ms |

Exit status: 1, as required by the still-failing memory gate. All other measured
Python and C# targets passed each repetition. The final batch ran the release
benchmark binary directly after container builds; a native host `just bench`
build is unavailable without the host GTK/WebKit development headers.

[Full `just check` output](check.txt): passed in the existing Linux build
container, including 61 frontend tests, 50 Python tests, 22 .NET tests,
formatting, workspace Clippy, Rust tests (including 7 benchmark tests and 41 C#
integration tests), and a native Tauri debug build. The optional real
basedpyright integration suite was also run explicitly with
`cargo test --locked -p codingpad-kernel-core --test language_server -- --ignored`:
all three tests passed. Download-only installer tests remain opt-in.

[Native smoke output](smoke.txt): passed against the debug binary under the
container's Xvfb/WebKitWebDriver, checking the marker before typing, execution,
rerun and independent-tab behavior. Use Docker `--init` with `xvfb-run`, whose
readiness signal cannot work normally when it runs as PID 1. The WebDriver
startup connection-refused retry in the log is followed by the passing result.

macOS measurements and a new Windows check were not run in this investigation.
PR #32 remains the recorded Windows verification; the new cross-platform code
still requires normal platform validation before milestone closure.
