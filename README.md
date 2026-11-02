# CodingPad

A desktop scratchpad for running plain scripts and inspecting their values. This
build supports Python with a Monaco editor, Python completions and hover,
type diagnostics, streamed stdout/stderr, rich dumps (lazy nodes, paged
virtualised tables, images, sandboxed HTML, live updates), structured errors with
clickable tracebacks, automatic display of the last expression, Stop/Reset, a
library sidebar with pinned folders and plain-file saving, inline Python
packages (PEP 723), and one-click private `uv` installation. The C# kernel adds
in-memory Roslyn compilation, console output, rich `.Dump()` trees and deferred
enumerable tables, top-level `await`, and compiler diagnostics in the editor and
Results.

## Develop locally

Install:

- Node.js 22.12+ (or 24+) and npm
- Stable Rust, including `rustfmt` and `clippy`
- .NET 10 SDK or later on `PATH` (builds the bundled C# kernel and Roslyn compiler)
- [`just`](https://just.systems/man/en/packages.html)
- [`uv`](https://docs.astral.sh/uv/getting-started/installation/) and Python 3.10+
  (`uv python install` if needed)
- [Tauri 2 platform prerequisites](https://v2.tauri.app/start/prerequisites/):
  Xcode Command Line Tools on macOS; GTK/WebKitGTK development packages and a C
  compiler on Linux; MSVC build tools and WebView2 on Windows.

On Ubuntu/Debian, install the native dependencies with:

```sh
sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev \
  libappindicator3-dev librsvg2-dev patchelf
```

From the repository root:

```sh
just check  # Install locked frontend dependencies; build and test every component
just run    # Open the Tauri application with frontend hot reload
```

`just check` checks TypeScript, builds the UI, runs frontend and Python tests,
checks Rust formatting and Clippy, runs Rust integration tests against real Python
processes, and builds the desktop executable. No CI is configured (D39). The same
command is intended for macOS, Linux and Windows; native builds run on their own
platforms.

[Windows verification results](docs/windows-verification.md) record the native
build, tests, and desktop smoke check with Python 3.14.7 and Node.js 26.

CodingPad checks `PATH`, then the usual `~/.local/bin` and `~/.cargo/bin`
locations for `uv`, plus `/opt/homebrew/bin` and `/usr/local/bin` on macOS.
Launch from a terminal if your toolchain lives elsewhere.
Each scratch tab gets its own warm Python process on its first run. If `uv` is
missing, Results offers **Install uv and run**. One click downloads a pinned
official uv release, verifies its SHA-256, installs it privately under the app data
folder (`toolchains/uv/0.13.6`), and retries the script without restarting the app.
The download requires an internet connection; a failed download can be retried.
Code intelligence reconnects automatically after installation. Downloads allow up
to 30 minutes overall, with a 60-second read idle timeout. Retrying an interrupted
installation removes abandoned staging files while holding a process lock, so
another app instance's active download is preserved.
The app prefers your existing `uv` over its private copy and never changes your
shell profiles or `PATH`. Private installation supports x86-64 and ARM64 on macOS,
Linux (GNU and musl), and Windows. Other architectures can use a manually installed
`uv`. No tools are bundled in the application installer.

Python 3.10+ must already be installed. The app uses the installed Python selected
by `uv python find`; a missing Python produces an actionable message in Results.
The uv install action installs only `uv`. Scripts with inline dependencies install
their packages through `uv` when Run is pressed.

The default test suite runs offline installer checks. To also download the real
release into a temporary folder and run Python through it:

```sh
cargo test -p codingpad-kernel-core --lib toolchains::tests::official_release -- --ignored
```

This needs network access and an installed Python 3.10+. It leaves user toolchains
unchanged. Release URLs and SHA-256 digests are pinned in
`crates/kernel-core/src/toolchains.rs`; update that manifest and the documented
version together when upgrading the private `uv` release.

Try the initial script, or paste:

```python
from codingpad import dump, html, live

print("Hello from CodingPad!")
rows = [{"name": name, "score": score} for name, score in [("Ada", 10), ("Grace", 20)]]
dump(rows)                      # a table
dump(html("<b>trusted markup</b>"))
status = live("starting")
dump(status)                    # updates in place when status.value changes
status.value = "done"
6 * 7
```

Click **Run** or press **Ctrl+Enter** (**Cmd+Enter** on macOS). Results should show
the greeting, a table of records, rendered markup, a live value that changes to
`done`, and `42`. Edit and rerun to replace the previous output. New scratch tabs
keep independent source and results; closing a tab terminates its Python process.

## Library and files

The sidebar shows your **library** at `Documents/CodingPad` (created on first
launch and changeable from **Change…**) plus any **pinned folders** you add. Click
a file to open it, or use **Open file…** for any single file. **Save** (or
**Cmd/Ctrl+S**) writes the script back as a plain native file; a scratch tab
prompts for its location the first time and can use **Save as…** afterwards.
CodingPad settings live in the app config folder, never in your scripts.

A script's optional single-line header stays at the top of the file and is shown
in the footer. For Python it is a `#` comment; C#, Rust and TypeScript use `//`:

```python
# codingpad: connection=northwind
```

The line is preserved byte-for-byte when reading, editing and saving. Saves go
through a temporary file and rename, so a crash mid-save cannot truncate a
script; a corrupt settings file is copied to `settings.json.bak` before it is
replaced.

The file commands accept paths from the local webview (the library, pinned
folders and dialog choices); there is no filesystem sandbox between the UI and
your user account. Python (`.py`) and C# (`.cs`) files can run; other file
types remain editable and saveable.

## C# scripts (M2.2)

Click **+ C#** or open a `.cs` file. Install .NET 10 or later first; CodingPad
finds `dotnet` on `PATH`, in `DOTNET_ROOT`, or in standard install locations and
selects the newest installed stable `Microsoft.NETCore.App` runtime, 10 or later.
The first successful C# startup caches runtime discovery and extracts the bundled
assets once for the app session; tabs and kernel restarts reuse them. Restart the
app after installing a newer runtime to select it.
The application bundles its compiler and helper library. Building CodingPad
requires the SDK and restores the locked Roslyn NuGet dependency; running the
installed app does not restore or build a project per script.

```csharp
Console.WriteLine("Hello from C#!");
"explicit dump".Dump();
await Task.Delay(10);
Enumerable.Range(1, 10).Sum(); // automatically dumps 55
```

Each tab keeps a warm process. Each run compiles into a fresh collectible assembly
and requests its unload afterwards, so script static fields start fresh. Background
work and references scripts leave in process-wide state can delay unloading.
A final call returning a value is automatically dumped, including an awaited
value-returning call. A final `.Dump()` is shown once; void calls are not dumped.
Ref-struct and pointer results are left unwrapped so valid calls keep running.
Empty, comment-only, and type-only scripts succeed without output; explicit
`Main` programs still execute.
Bare expressions such as `6 * 7;` remain compile errors. Common `System` namespaces
and `CodingPad` are available without `using` lines. The original source is
validated before auto-dump instrumentation, and compile failures block execution.
Compiler errors have editor squiggles after Run and clickable lines in Results;
editing clears outdated squiggles. Runtime exceptions also show script frames.

C# dumps include reflection-based object trees, lazy properties and nested values,
and virtualised tables for `IEnumerable` values. Each table initially reads at
most 100 items, then advances the same iterator as you scroll. The footer shows
`100+ rows` until the iterator ends; even a collection with a known count follows
this incremental path. Previously read pages are cached without enumerating again.
Each dump (including a live update) starts a fresh inspection cache, so dumping a
mutated collection or record again shows its current values. Object fields are
read at dump time; lazy properties read the value **when first expanded**, which
can reflect changes made after the dump. That getter result or failure is then
cached for that dump. Record-table properties are evaluated for the requested
rows and cached within that table. Indexers, static members and custom `ToString`
methods are not called. Nested objects in cells expand below the table. Exceptions,
including property and iterator failures, have clickable script stack frames and
inner exception summaries.

```csharp
new { Name = "Ada", Scores = new[] { 90, 95 } }.Dump();
Enumerable.Range(0, 100_000).Select(i => new { Index = i, Square = i * (long)i }).Dump();
new HtmlValue("<b>Hello</b>").Dump();
new ImageValue(System.IO.File.ReadAllBytes("plot.png"), "image/png", "plot.png").Dump();
var progress = new LiveValue(0);
progress.Dump();
for (int i = 1; i <= 3; i++) { progress.Value = i; await Task.Delay(100); }
```

Handles and partially read iterators remain available after completion and are
released on the next run, Reset, or close. The next run disposes unfinished
iterators, including their `finally` blocks. Live updates are accepted only during
the originating run. Replacing a live value immediately disposes its old iterators,
expires its old handles, and removes nested live subscriptions; only its latest
contents remain expandable. Independent dumps and sibling values remain available.
Object fields are paged, trees stop after six levels, strings
are bounded, HTML is sandboxed, and images are limited to 4 MiB. Table columns use
the first item's public fields/properties (up to 32); scalar sequences use one
`Value` column. Enumeration runs user code. A blocked preview is cancelled by
Reset or closing the tab, or automatically after ten seconds. Cancelling or timing
out a preview discards the kernel and its cache; Run then starts a fresh process.
Stop also cancels pending previews. Mutating a collection may invalidate an older
dump's retained iterator; the next dump creates a new iterator.

C# scripts have `Cancel`, `Cache` and `Secret` available without `using` lines:

```csharp
var rows = Cache("rows", () => Enumerable.Range(1, 1000).ToArray());
rows.Dump();
await Task.Delay(60_000, Cancel); // Stop cancels this delay without losing the cache
```

Stop cancels the current run's token first. Pass `Cancel` to async operations, or
call `Cancel.ThrowIfCancellationRequested()` in long loops. If the script ignores
cancellation, the supervisor kills its kernel after about two seconds and reports
that the cache was cleared. Cooperative Stop keeps the process and cached values.
Reset discards that tab's process, cache and previews; other tabs are unaffected.

`Cache("key", () => ...)` retains successful values across runs, including null.
Factories run outside the cache lock; concurrent misses may compute more than
once and the first stored value wins. Failed factories are not cached. Each run
rechecks cached object graphs, including mutable containers, without calling
properties or enumerating user sequences. Values containing script-defined types
are discarded before the next compilation; requesting their keys again runs the
factory and shows `cache refreshed: script types changed`. Delegates, reflection/native
handles and graphs that cannot be safely inspected within the inspection budget
are conservatively refreshed too. Standard scalar arrays have no size limit for
this check. Background work must finish within its run; late cache calls fail.
`Secret("name")` throws an explanation that vault support is planned for M3.

The runtime is packable as `CodingPad`; its `buildTransitive` imports make the same
helper names compile in .NET 10 file scripts with `#:package CodingPad@0.1.0`.
The updated package must be published or supplied through a local NuGet feed;
the existing registry reservation is not this implementation. Run
`dotnet pack runtimes/dotnet/CodingPad -o /path/to/feed` to build it locally.
Package directives inside the app, Roslyn completions and SDK installation follow
in later M2 issues. Only managed `Console` output is
captured; direct native writes are outside this increment. Scripts run with your
user privileges.

## Repository layout

- `apps/desktop`: React/TypeScript UI and Tauri shell
- `crates/kernel-core`: Rust kernel supervisor, independent of the window
- `kernels/python`: Python stdio kernel
- `kernels/csharp`: C# stdio kernel and pinned Roslyn dependency
- `runtimes/dotnet/CodingPad`: bundled C# `.Dump()` extension library
- `runtimes/python/codingpad`: bundled `dump()` helper (no registry download)
- `tests/python`: Python kernel protocol and execution tests
- `tests/dotnet`: C# cancellation protocol and standalone package checks
- `docs`: design decisions and [kernel protocol](docs/kernel-protocol.md)
- `packaging/placeholders`: registry name reservations, separate from the runtime

**Stop** requests cancellation first. If the script has not finished after about
two seconds (for example, it is blocked in native code), CodingPad kills its
kernel and reports that the cache was cleared; the next Run starts a fresh kernel. An interruptible
Python loop stops without clearing its cache. **Reset**, available when idle,
discards that script's kernel and clears cached values and old results.

Use `cache(key, factory)` (also importable from `codingpad`) to keep slow results
across reruns. Each script has its own cache; failed computations are not saved:

```python
from codingpad import cache

rows = cache("rows", lambda: expensive_query())
dump(rows)
```

Concurrent cache misses may run the factory more than once; the first stored
value wins. Factories can wait on worker threads that also use `cache()`.

Change the key or Reset to recompute a value. A forced restart or closing the tab
also clears its cache. Globals still start fresh on every run.

Declare Python packages in a [PEP 723](https://peps.python.org/pep-0723/) comment block:

```python
# /// script
# dependencies = ["requests>=2,<3"]
# requires-python = ">=3.10"
# ///

import requests
requests.__version__
```

Run installs missing packages and streams `uv` progress/errors into Results.
Environments are cached by the sorted dependency declarations and Python constraint,
shared across tabs and app restarts; ordinary edits keep the tab's warm process.
Changing or removing metadata restarts that tab's kernel and clears `cache()`.
Reset clears the running kernel but keeps installed packages. Stop also cancels
package preparation. A first install needs network access unless packages are already
cached or local. Use version pins for repeatable resolution.

`requires-python` selects an already installed Python compatible with both the
script and CodingPad's Python 3.10+ minimum. An empty metadata block creates an
isolated environment without additional packages. CodingPad always supplies its
bundled `codingpad` runtime. `[tool]` metadata is accepted but does not configure
package resolution yet; project `uv.toml` and `.env` files are not loaded. Package
launchers live in the user's cache folder under `codingpad/python-packages-v1`;
`uv` manages the environments in its own cache. Relative local dependency paths
are not supported for either saved scripts or scratch scripts; use absolute file
URLs. Saved scripts do not resolve package paths against their containing folder.

Scripts execute with your user privileges, as
specified in the architecture. A run gets fresh globals, while imported modules
and the interpreter remain warm; this is not interpreter isolation.

Dumps show up to 100 children per page, six levels, 1,000 nodes and about 4 MiB
per dump or page, and 100 rows per table page. Table cells cap strings at 1,000
characters (with a truncation marker); images, HTML and strings that do not fit
the page budget become lazy. Larger values show a **Show more** button,
deep values (and table cells that need their own tree) show **Expand**, and big
tables virtualise their rows and fetch pages on scroll. Webviews cap element
height, so a table above roughly 500,000 rows cannot be scrolled to its end.
Handles keep those values alive until the next run, so expand before rerunning. Cycles are marked; properties
and arbitrary iterators are not consumed. HTML renders in a sandboxed frame, and
images render from data URIs. Script errors appear as a structured node with
clickable line links. The results panel retains at most 500 output blocks and the
last 100,000 characters of each adjacent text stream. Stream fragments are
coalesced once per animation frame, with stable output IDs preserving
expanded/collapsed trees. Monaco, its Python syntax definition, and its worker are
bundled locally; there is no editor CDN dependency.

## Code intelligence

Python completions, hover and type diagnostics come from **basedpyright** through
the app's language-server bridge. The first editor start may download basedpyright
with `uv`; later starts reuse the cache. PEP 723 inline dependencies are installed
for the editor environment, so completions work for the packages a script
declares. Editing the inline metadata restarts the language server for the new
dependency set; only wheels are used, so an sdist is never built just by opening
a script.

Type errors appear as squiggles and a count in the editor header, but they never
stop **Run**; Python executes regardless.

## Performance checks

`just bench` builds the release app and measures the M1 targets from D38 on
macOS and Linux:

| Metric | Target |
|---|---|
| App usable | < 1 s |
| Idle memory | < 300 MB |
| First Python kernel start | < 3 s |
| Warm Python rerun | < 200 ms |
| 100k-row dump, first page to host | < 1 s |
| Python completion | < 300 ms |

The first completion run may download basedpyright; later runs reuse the cache.
Idle memory is sampled five seconds after the window is ready, so first-paint
buffers have been released; on macOS it is the sum of physical footprints, and
elsewhere it is resident memory. The dump metric ends when the first page reaches
the Rust host, before the webview renders it. The app metrics need a desktop session and are
skipped on Windows, where a Windows VM only confirms that `just check` passes
(D42, D48). Set `CODINGPAD_DESKTOP` to benchmark a different app binary.

Dual-licensed MIT OR Apache-2.0.

For an optional native UI smoke test on Linux/Windows, install
[`tauri-driver`](https://v2.tauri.app/develop/tests/webdriver/) with
`cargo install tauri-driver --locked`, plus `webkit2gtk-driver` on Linux or a
matching `msedgedriver` on Windows. After `just check`, run:

```sh
just smoke                              # A desktop session is available
xvfb-run just smoke                     # Headless Linux; also needs xvfb + xauth
just smoke target/debug/codingpad-desktop.exe  # Windows
```

This launches the built app and verifies local Monaco loading, printed output,
nested dumps, last-expression output, reruns and a second scratch tab through
real Tauri IPC and Python processes.
