# CodingPad

A desktop scratchpad for running plain scripts and inspecting their values. This
M1 build supports Python with a Monaco editor, streamed stdout/stderr, rich dumps
(lazy nodes, paged virtualised tables, images, sandboxed HTML, live updates),
structured errors with clickable tracebacks, automatic display of the last
expression, Stop/Reset, a library sidebar with pinned folders and plain-file
saving, and inline Python packages (PEP 723).

## Develop locally

Install:

- Node.js 22.12+ (or 24+) and npm
- Stable Rust, including `rustfmt` and `clippy`
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

CodingPad checks `PATH`, then the usual `~/.local/bin` and `~/.cargo/bin`
locations for `uv`, plus `/opt/homebrew/bin` and `/usr/local/bin` on macOS.
Launch from a terminal if your toolchain lives elsewhere.
Each scratch tab gets its own warm Python process on its first run. Missing `uv` or Python produces an actionable
message in Results. The app uses the installed Python selected by `uv python find`
and does not download a toolchain automatically. Scripts with inline dependencies
install their packages through `uv` when Run is pressed.

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
your user account. This build runs Python only, so **Run** is disabled for other
file types while they remain editable and saveable.

## Repository layout

- `apps/desktop`: React/TypeScript UI and Tauri shell
- `crates/kernel-core`: Rust kernel supervisor, independent of the window
- `kernels/python`: Python stdio kernel
- `runtimes/python/codingpad`: bundled `dump()` helper (no registry download)
- `tests/python`: kernel protocol and execution tests
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

Language servers arrive later in M1.
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
