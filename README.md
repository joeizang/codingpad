# CodingPad

A desktop scratchpad for running plain scripts and inspecting their values. This
first skeleton supports Python with a Monaco editor, streamed stdout/stderr, basic
object/list/value trees, and automatic display of the last expression.

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
locations for `uv`. Launch from a terminal if your toolchain lives elsewhere.
Each scratch tab gets its own warm Python process on its first run. Missing `uv` or Python produces an actionable
message in Results. The app uses the installed Python selected by `uv python find`
and does not download a toolchain or dependencies automatically.

Try the initial script, or paste:

```python
from codingpad import dump

print("Hello from CodingPad!")
dump({"name": "Ada", "scores": [10, 20, 30]})
6 * 7
```

Click **Run** or press **Ctrl+Enter** (**Cmd+Enter** on macOS). Results should show
the greeting, an expandable dictionary/list tree, and `42`. Edit and rerun to
replace the previous output. New scratch tabs keep independent source and results;
closing a tab terminates its Python process. Scratch scripts are **unsaved** until
the library/files work in #4 lands.

## Repository layout

- `apps/desktop`: React/TypeScript UI and Tauri shell
- `crates/kernel-core`: Rust kernel supervisor, independent of the window
- `kernels/python`: Python stdio kernel
- `runtimes/python/codingpad`: bundled `dump()` helper (no registry download)
- `tests/python`: kernel protocol and execution tests
- `docs`: design decisions and [kernel protocol](docs/kernel-protocol.md)
- `packaging/placeholders`: registry name reservations, separate from the runtime

The skeleton deliberately has no Stop/Reset, file saving, package installation or
language server yet. Stop/Reset arrives in #3; until then, close the application
to terminate a runaway script. Scripts execute with your user privileges, as
specified in the architecture. A run gets fresh globals, while imported modules
and the interpreter remain warm; this is not interpreter isolation.

Basic dumps show up to 100 children per node, six levels and 1,000 nodes per
dump, mark cycles, and do not consume arbitrary iterators or evaluate properties. Lazy/paged dumps arrive
in #2. The results panel retains at most 500 output blocks and the last 100,000
characters of each adjacent text stream. Monaco, its Python syntax definition,
and its worker are bundled locally; there is no editor CDN dependency.

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
