# CodingPad technical reference

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

## Install a build (M4.5)

GitHub Actions builds installers on their native platforms when a `vX.Y.Z` tag
is pushed (D29, D63). Each [GitHub Release](https://github.com/joeizang/codingpad/releases)
includes macOS disk images for Apple silicon and Intel, an AppImage, a `.deb`
and an `.rpm` for Linux, and an NSIS setup for Windows (D71).

Build the installer for the machine you are on:

```sh
just bundle
```

The installers land in `target/release/bundle/` (`dmg/`, `appimage/`, `deb/`,
`rpm/`, `nsis/`). The app embeds its Python and C# kernels, so an installer is
self-contained; it does not include Python, the .NET SDK, Bun or `uv`. A first
run offers **Install uv and run**, **Install .NET and run** or **Install Bun and
run** when a toolchain is missing, or uses one you already have.

`packaging/app-icon.png` is the icon source. Regenerate the desktop icons with
`npm --prefix apps/desktop run tauri -- icon ../../packaging/app-icon.png`. Keep
`32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.png`, `icon.ico` and
`icon.icns`. Delete the `Square*`/`StoreLogo` files, `64x64.png` and the `android/` and
`ios/` folders that the generator also writes.

### macOS

Open the `.dmg` from GitHub Releases or from `target/release/bundle/dmg/`
if you built it locally. Choose the file ending in `_aarch64.dmg` for Apple
silicon or `_x64.dmg` for Intel. Drag **CodingPad** into **Applications**, then
eject the disk image.

Install CodingPad with [Homebrew](https://brew.sh/):

```sh
brew tap joeizang/codingpad https://github.com/joeizang/codingpad.git
brew install --cask joeizang/codingpad/codingpad
```

Homebrew chooses the disk image for your Mac and verifies its SHA-256 checksum.
After either installation, open **CodingPad** from Applications. Early builds
use an ad hoc signature without Apple notarization (D30). If macOS blocks the first launch, open
**System Settings → Privacy & Security**, scroll to **Security**, click
**Open Anyway**, and confirm the prompt. See [Apple’s first-launch instructions](https://support.apple.com/en-us/102445).

To update, run `brew update` and then
`brew upgrade --cask joeizang/codingpad/codingpad`.
Using `--zap` when uninstalling also removes CodingPad's local app data,
including settings, connection metadata, caches, and privately installed
toolchains. Credentials stored in the macOS Keychain remain there.

### Linux

- **AppImage**: `chmod +x CodingPad_0.2.0_amd64.AppImage`, then run the file.
- **Debian/Ubuntu**: `sudo apt install ./CodingPad_0.2.0_amd64.deb`
- **Fedora/RHEL**: `sudo dnf install ./CodingPad-0.2.0-1.x86_64.rpm`

Use the version and architecture of the file you downloaded. The `.deb` and
`.rpm` install for all users; the AppImage is a single file and needs no install.
The AppImage bundles WebKitGTK and GTK, so it runs on a minimal system; the
`.deb` and `.rpm` list those system packages as dependencies and let your package
manager install them.

### Windows

Run `CodingPad_0.2.0_x64-setup.exe`. The build is unsigned, so SmartScreen may
show **Windows protected your PC**: choose **More info**, then **Run anyway**.
The setup installs for the current user and needs no administrator rights, and
it installs the WebView2 runtime when it is missing (which needs an internet
connection).

### First launch

In the initial Python tab, replace the editor contents with `print(6 * 7)` and
click **Run**. If prompted, choose **Install uv and run**. The script also needs
Python 3.10 or later. If Results reports that Python is missing, run
`uv python install 3.13` in a terminal. On macOS, if you installed uv through
CodingPad, use its private copy:

```sh
"$HOME/Library/Application Support/io.github.joeizang.codingpad/toolchains/uv/0.13.6/uv" python install 3.13
```

Click **Run** again and confirm that **Results** displays `42`.

CodingPad sends no telemetry. Its network use is limited to the release check and
the toolchain or package downloads you trigger, such as **Install uv and run**,
**Install .NET and run**, **Install Bun and run**, and the language servers and
packages a script needs.
There is no auto-updater (D29): see [Update notice](#update-notice-m43), and
install the newest build over the current one to update.

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
just bundle # Build this platform's installer (see Install a build)
```

`just check` checks TypeScript, builds the UI, runs frontend and Python tests,
checks Rust formatting and Clippy, runs Rust integration tests against real Python
processes, and builds the desktop executable. GitHub Actions runs the same command
on Linux and Windows for every pull request (D63). The same command is intended for
Linux and Windows. Release packaging also builds natively on Apple silicon and
Intel Macs (D71).

### CI and releases (D63)

- **Pull requests** to `master` run `just check` on Linux and Windows. Both checks
  must pass on an up-to-date branch before the squash merge; no review approval is
  required. A private repository runs Linux only.
- **Releases** follow a version tag. Bump `version` in `apps/desktop/src-tauri/tauri.conf.json` (and
  the workspace `Cargo.toml`), merge, then push a matching tag:
  `git tag v0.2.0 && git push origin v0.2.0`. The release workflow builds the
  installers and publishes the GitHub Release with generated notes. A tag with a
  suffix such as `v0.2.0-rc.1` becomes a pre-release, which the update notice ignores.
- After each stable release, [update the Homebrew tap](Casks/README.md) with
  the cask generated from the published disk images.
- **Repository protections** (rulesets, merge and Actions settings, secret
  scanning, CodeQL) are applied by `tools/github/configure-repo.sh` on a public
  repository; set **Pull requests → Collaborators only** by hand in Settings.

[M3 verification results](docs/m3-verification.md) record the Linux and Windows database,
vault and benchmark runs for the M3 close.
[Windows verification results](docs/windows-verification.md) record the native
build, tests, and desktop smoke check with Python 3.14.7 and Node.js 26.
[macOS verification results](docs/macos-verification.md) record earlier acceptance
runs on Apple silicon. D71 resumes macOS release packaging while milestone
acceptance runs target Linux and Windows.

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
Windows and Linux (GNU or musl builds). Other architectures can use a manually
installed `uv`. No tools are bundled in the application installer.

Python 3.10+ must already be installed. The app uses the installed Python selected
by `uv python find`; a missing Python produces an actionable message in Results.
The uv install action installs only `uv`. Scripts with inline dependencies install
their packages through `uv` when Run is pressed.

To run TypeScript, choose **TypeScript** in the New Script menu or open a `.ts`
file. CodingPad uses [Bun](https://bun.com/docs/installation) and looks for it on
`PATH`, in `BUN_INSTALL/bin` or `~/.bun/bin`. If Bun is missing, Results offers
**Install Bun and run**. One click downloads a pinned official Bun release,
verifies its SHA-256, installs it privately under the app data folder
(`toolchains/bun/1.4.4`), and retries the script without restarting the app. The
app prefers your own Bun and never changes your shell profiles or `PATH`. Private
installation supports x86-64 and ARM64 on macOS, Windows and Linux (GNU or musl
builds); on x86-64 CPUs without AVX2 it installs Bun's baseline build. Import
`dump` from `codingpad` to
display a value in Results. The last expression is dumped automatically.
Top-level `await` works and the kernel runs scripts with type errors. Import
`cache` from `codingpad` to keep a structured-cloneable result between reruns of
the same tab; factories run synchronously and values are cloned on every run, so
cache plain data rather than class instances. Reset or closing the tab clears
cached values. Globals start fresh on every run. Cooperative Stop keeps the
cache; a forced restart clears it. CI installs Bun to exercise this path.

TypeScript code intelligence uses the TypeScript 7 native language server
(`tsgo`) on the same hidden project that Run prepares for versioned imports.
Install it once with `bun add -g @typescript/native-preview`; CodingPad also
checks its own `typescript` data folder and `PATH`, and `CODINGPAD_TS_LSP` can
point at the executable. Completions, hover and type errors cover `dump`,
`cache`, `db`, `secret()` and other `codingpad` helpers. Versioned imports such
as `zod@3` resolve from the shared project;
changing the import set restarts the server. Type errors show as squiggles and a
count but never block **Run**.

If no .NET 10+ runtime is installed, Results offers **Install .NET and run**. One
click downloads the official `dotnet-install` script pinned to the
`dotnet/install-scripts` tag `v2026.07.21`, verifies its SHA-256, and runs it to
install the current long-term-support SDK privately under the app data folder
(`toolchains/dotnet/lts`). The script needs bash with `curl` or `wget` on Linux
and macOS, or PowerShell on Windows, plus an internet connection. It installs
into a staging folder and is promoted only after `dotnet --list-sdks` succeeds,
so an interrupted download is cleaned up and retried safely. CodingPad never
changes your shell profiles or `PATH`, and it prefers a supported .NET you
installed yourself. Code intelligence restarts with the installed SDK.

The default test suite runs offline installer checks. To also download the real
releases into a temporary folder and run a script through each one:

```sh
cargo test -p codingpad-kernel-core --lib toolchains::tests::official_release -- --ignored
cargo test -p codingpad-kernel-core --lib toolchains::tests::official_dotnet -- --ignored
cargo test -p codingpad-kernel-core --lib toolchains::tests::official_bun -- --ignored
```

This needs network access (and Python 3.10+ for the uv test). It leaves user
toolchains unchanged. Release URLs and SHA-256 digests are pinned in
`crates/kernel-core/src/toolchains.rs`; update that manifest and the documented
versions together when upgrading private toolchains.

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

## Run from the terminal (M4.1)

`cpad run` runs a saved script with the same kernels as the app, without a window:

```sh
cargo run --locked -p cpad -- run hello.py
```

`cargo build --locked -p cpad` produces `target/debug/cpad` (`target/release/cpad`
with `--release`). Python (`.py`), C# (`.cs`) and TypeScript (`.ts`) scripts
use the app's kernels. TypeScript uses Bun on PATH or the private Bun installed
by CodingPad. A `.sql` script runs through the connection named in its header.
Dumps print as text. Short values stay on one line, tables are aligned, and C#
properties and deep Python values are materialised in the kernel so they appear
inline. For a table larger than its first 100-row page, the output shows the
displayed and total row counts. Use the desktop or CSV export for every row.

`cpad run` uses the app's data folder (`io.github.joeizang.codingpad`) for
connections, secret names and private toolchains, so the same saved connection
works in both places. Set `CODINGPAD_DATA_DIR` to use another folder. A script
with a `codingpad: connection=<name>` header gets the same `db` client as the
app, including typed data contexts for Python and C#. `secret()` reads the same
vault values. `cache()` reuses a value for the same key within that run. Each
`cpad run` starts a fresh kernel. Exit codes are 0 for success, 1 for a script
failure, 2 for a usage or script-type error, and 130 when Ctrl-C stops the run.
Output streams as it is produced. Live values print their
current value where the dump appears, and later updates are progress lines on
stderr (`live: 42`), capped at 100 lines. A dump that still contains a value the
kernel could not make eager (a node budget cut-off) is held until the run
finishes, expanded within a 10-second budget, and printed in its original
position. Held output is capped at 500 events; reaching the cap prints the
previews as they are rather than delaying a long run further.

`cpad run --html report.py > report.html` writes one standalone page instead of
text. Tables stay tables, object fields become description lists and tracebacks
keep their frames. Strings and streams are escaped; only `html` dump nodes pass
markup through, and `<meta>`/`<base>` navigation tags are removed from them. The
page's strict Content Security Policy blocks scripts, frames, forms and network
access. HTML output waits for the run to finish so live values replace their
dumps in place.

## Library and files

The sidebar shows your **library** at `Documents/CodingPad` (created on first
launch and changeable from **Change…**) plus any **pinned folders** you add. Click
a file to open it, or use **Open file…** for any single file. **Save** (or
**Cmd/Ctrl+S**) writes the script back as a plain native file; a scratch tab
prompts for its location the first time and can use **Save as…** afterwards.
CodingPad settings live in the app config folder, never in your scripts.

The footer shows the optional single-line script header. Put the header at
the top of the file unless it follows cargo-script frontmatter in Rust. For Python it is a `#` comment; C#, Rust and TypeScript use `//`;
SQL uses `--`:

```python
# codingpad: connection=northwind
```

For Rust scripts using cargo-script frontmatter, put the header comment after
the closing fence. A shebang, if present, comes before the frontmatter.

The line is preserved byte-for-byte when reading, editing and saving. Saves go
through a temporary file and rename, so a crash mid-save cannot truncate a
script; a corrupt settings file is copied to `settings.json.bak` before it is
replaced.

The file commands accept paths from the local webview (the library, pinned
folders and dialog choices); there is no filesystem sandbox between the UI and
your user account. Python (`.py`) and C# (`.cs`) files can run; other file
types remain editable and saveable.

## Samples and the sample database (M4.2)

The sidebar's **Samples** section lists built-in Python, C#, TypeScript and SQL
scripts. They cover values and tables, collections and LINQ, async work,
`cache()`, live values, HTML output and queries against the bundled database.
Clicking a sample opens a fresh unsaved copy. The embedded original stays intact
when you edit or save the copy. Saving writes an ordinary native file to the
folder you choose, and the tab then behaves like any other saved script.

CodingPad ships a small SQLite shop database (customers, orders, products and
order items) and prepares it in the app data folder on first use as the reserved
**codingpad-sample** connection. SQL samples name it in their header, and
Python and TypeScript samples use `db.query`, while C# samples use `db.Query`.
No connection setup is needed. The app protects the built-in connection from
removal and reserves its name against user connections. `cpad` leaves
`connections.json` untouched. The database file is written once, so rows you
add through the sample connection stay there. A new bundled version uses a new
file name and removes the previous bundled copy. The terminal runner
prepares the same database, so `cpad run samples/typescript/query_orders.ts`
works from a checkout. `samples/sample.sqlite`
is generated by `python3 samples/build_sample_database.py`; the test suite runs
every SQL and TypeScript sample. Install Bun before running the TypeScript
sample tests. An ignored acceptance test also runs the Python and C# samples,
which may need network access for their packages.

## Update notice (M4.3)

CodingPad has no auto-updater. Once per launch it asks the GitHub Releases API
for the newest published release. When that release is newer than the running
build, a banner above the tabs names it and links to its release page, which
opens in the system browser. A current build shows no banner, and every failure
— offline, a rate limit, no release published yet, or an unexpected answer — is
silent: the app never reports the check and nothing else changes. Only plain
numbered releases are offered; drafts and pre-releases are ignored, and only
release pages in this repository are linked.

This is the only request CodingPad makes on its own (D29, D31), and it never
downloads or installs anything. To update, download the newest installer from
GitHub Releases and install it over the current build.

## Report a crash (M4.4)

CodingPad has no telemetry (D31). A fatal panic in the app, or an error that
stops the interface from drawing, shows a crash screen with **Report on
GitHub**. The button opens a pre-filled issue in the system browser; nothing is
submitted until you review it there and press Submit. Nothing is sent
automatically.

The issue holds the app version, the platform and the error details: the
message and the position. The crash screen also shows a backtrace locally, but
that stays out of the issue because it is the likeliest place for source paths
and production values. The user's home folder becomes `~`, credentials in URLs
and values after keys such as `password=` or `token=` become `[redacted]`, and
long details are cut so the link stays usable. Scripts and connection details
are never read for a report. Review the issue before submitting.

A fatal panic is written to `pending-crash.json` in the app data folder and
offered on the next launch, because the process that panicked may be gone before
it can draw anything. Only main-thread panics are recorded, and a clean exit
discards the file, so a worker panic the app recovered from does not reappear.
To see the screen on purpose, start a debug build once with
`CODINGPAD_FORCE_CRASH=1`; it crashes, and the next launch offers the button:

```sh
CODINGPAD_FORCE_CRASH=1 target/debug/codingpad-desktop
target/debug/codingpad-desktop
```

On Windows PowerShell, set `$env:CODINGPAD_FORCE_CRASH = "1"` for the first
command, then clear it with `Remove-Item Env:CODINGPAD_FORCE_CRASH`.

Event-handler and promise errors keep the app running and do not open the crash
screen, and `cpad` is a terminal command with its own exit status; neither is
covered by this report in the 0.1 preview.

## C# scripts (M2.3)

Click **+ C#** or open a `.cs` file. CodingPad finds `dotnet` on `PATH`, in
`DOTNET_ROOT`, or in standard install locations, then in its private copy, and
selects the newest installed stable `Microsoft.NETCore.App` runtime, 10 or later.
If none is available, Results offers **Install .NET and run** (see above).
The first successful C# startup caches runtime discovery and extracts the bundled
assets once for the app session; tabs and kernel restarts reuse them. Restart the
app after installing a newer runtime outside CodingPad to select it.
The application bundles its compiler and helper library. Building CodingPad
requires the SDK and restores the locked compiler and NuGet client dependencies; running the
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
The token is also cancelled when a run ends, including after failures, so
cooperative background work can exit. Captured tokens and their wait handles stay
usable after completion; cancellation callbacks run separately and are not awaited.
Reset discards that tab's process, cache and previews; other tabs are unaffected.

`Cache("key", () => ...)` retains successful values across runs, including null.
Async factories work with the same call: `await Cache("data", () => LoadAsync())`
caches the awaited result for both `Task<T>` and `ValueTask<T>`, consuming pooled
sources only once. Failed or cancelled factories are not cached. Concurrent
misses may compute more than once; the first successful stored result wins.
Incompatible types and inactive-scope access throw immediately, including when
the caller ignores the returned awaitable.

After compilation succeeds, known script-defined roots are discarded. Other
cached graphs are checked lazily on their first lookup in a run, outside the
cache lock, without calling properties or user iterators. Recursive safe storage
can bypass traversal; large ordinary collections are not evicted just for their
size. Stop can interrupt an inspection. Unused nested graphs can retain old
script objects until their next lookup or Reset. A successful recompute reports
`cache refreshed: script types changed` for script values; opaque handles or
callbacks and inspection failures have their own reasons. A failed factory keeps
its note pending for a retry in that run. Unused notes expire at completion.
Late cache calls and cache calls from idle previews fail.
`Secret("name")` reads a named secret from the OS password vault (see [Secrets](#secrets)).

The runtime is packable as `CodingPad`; its direct-consumer `build` imports make
helper names available in .NET 10 file scripts with `#:package CodingPad@0.1.0`.
Set `<CodingPadImplicitUsings>false</CodingPadImplicitUsings>` in a project, or
`#:property CodingPadImplicitUsings=false` in a file script, to choose imports
explicitly. The updated package must be published or supplied through a local
NuGet feed; the existing registry reservation is not this implementation. Run
`dotnet pack runtimes/dotnet/CodingPad -o /path/to/feed` to build it locally.
C# file directives are supported inside the app:

```csharp
#:package Newtonsoft.Json@13.0.3
#:property Nullable=enable
#:property DefineConstants=MY_FEATURE
Newtonsoft.Json.JsonConvert.SerializeObject(new { Answer = 42 }).Dump();
```

Use `#:package Name@Version` with an explicit NuGet version, range or floating
version. The kernel restores transitive dependencies using the bundled NuGet
client, with the standard global packages/HTTP caches. Restore uses user-wide
`NuGet.config` and configurations in the saved script's folder and its ancestors,
including source mapping, disabled feeds, fallback folders and configured
username/password or token credentials (tokens go in the password field).
Unsaved tabs use the app's working directory. Credential-provider plugins are
not supported yet. Restore failures appear in Results and prevent execution;
Stop cancels restore too. Successful restores are reused silently while package
directives, Web SDK choice, configuration contents/environment credentials and
resolved files remain unchanged. Floating versions and ranges stay pinned in a
warm kernel until those inputs change or Reset is used. A restore-cache miss
immediately shows “Restoring C# packages…” while resolution or downloads are
pending; cache hits stay silent. Actual installs and restore warnings also
produce progress output; restore errors appear once.
Package assembly identities are reused for unchanged resolved graphs, so `Cache`
can retain package values. Reset clears these loaded assemblies along with the
process. Unknown distro runtime identifiers fall back to the matching portable
RID, including musl on Linux. A declared `CodingPad` package uses the app's
built-in helper rather than downloading another copy.

Common `#:property Name=Value` compiler options are `AllowUnsafeBlocks`,
`CheckForOverflowUnderflow`, `Optimize`, `Nullable`, `DefineConstants` (semicolon
or comma separated), `LangVersion`, `ImplicitUsings`, `TreatWarningsAsErrors`,
`WarningLevel`, `NoWarn`, `WarningsAsErrors`, and `WarningsNotAsErrors`.
`CodingPadImplicitUsings` controls the helper imports. `TargetFramework` must
match the selected runtime (`net10.0` for .NET 10). `PublishAot` and `PackAsTool`
are accepted deployment switches with no effect on in-memory execution.
Other properties receive an explicit unsupported-property error; arbitrary
MSBuild targets, package build targets, analyzers and source generators are not
executed by this kernel. Put directives before code; duplicates are errors.

`#:sdk Microsoft.NET.Sdk.Web` adds ASP.NET Core references and Web SDK implicit
usings, allowing `WebApplication.CreateBuilder()`, routes and `RunAsync(Cancel)`.
Install the ASP.NET Core runtime matching the selected .NET major version (the
.NET SDK includes it). A long-running web app stays active until Stop. Project
references report: "not supported in CodingPad yet; runs with `dotnet run`".
C# code intelligence uses Roslyn against a generated hidden shadow project: the
script's directives become the project's packages, properties and SDK, and the
built-in `CodingPad` sources provide `.Dump()`, `Cache` and helper completions.
Diagnostics are pulled from Roslyn after edits and match the kernel's compiler.
Roslyn ships with the VS Code C# extension; CodingPad finds it there, in its own
`roslyn` data folder, on `PATH`, or at `CODINGPAD_ROSLYN_LSP`. A one-click SDK
install follows in a later M2 issue. Only managed `Console` output is
captured; direct native writes are outside this increment. Scripts run with your
user privileges.

## Connection schema

An attached connection (the `connection` setting in any script header) shows a
schema panel with tables and views, declared column types, and foreign-key targets.
Expand a table to inspect its columns. Filter by table name and use **More tables**
or **More columns** to browse large schemas in pages of 100. **Refresh schema**
loads current metadata; a failed refresh keeps the last snapshot visible. Editing
connections and closing the Connections dialog also reloads the schema.

Discovery uses SQLite's read-only catalog and PostgreSQL / SQL Server system
catalogs through the saved connection's existing authentication and TLS settings.
Only objects visible to that connection are shown; discovery never runs a script
or starts a kernel. SQLite shows the saved file's `main` schema (temporary objects
and script-local `ATTACH` databases are not persistent connection metadata).

SQL scripts use the loaded schema for local table/column completion and bundled
SQL keywords. Type a name, use **Ctrl+Space**, or type an alias followed by `.`.
Column suggestions follow tables in the current statement, including joins,
aliases, and enclosing correlated-query scopes. Schema-qualified names disambiguate
same-named tables. Inserted identifiers use the connection's quoting rules.

Completion requests perform no IPC, network access, or kernel startup. Snapshots
remain in memory across script switches, including offline use; **Refresh schema**
updates them after DDL changes. At most 200 suggestions are returned per request;
typing more narrows the list. The lexical resolver handles common SQL statements,
not full dialect grammars: CTE/derived-table output inference, table-valued function
outputs, server search-path/collation rules, and procedural SQL are not inferred.
Ambiguous unqualified table names offer columns from each matching schema.

## SQL scripts (M3.0)

Python scripts can query an attached connection with the run-scoped `db` client:

```python
# codingpad: connection=codingpad-sample
db.query("SELECT * FROM customers")
```

`db.query(sql, parameters)` returns rows as dictionaries and displays them as a
table. `db.execute(sql, parameters)` executes a statement and returns the
number of affected rows. Queries return at most 10,000 rows; a truncated dump is
marked in the results panel. PostgreSQL results are streamed; remaining rows
are consumed and discarded so statements with `RETURNING` finish normally.
Duplicate column names get unique suffixes such as `id_2`. Parameters use the selected driver's syntax (SQLite
and SQL Server use `?`, PostgreSQL uses `%s`). CodingPad installs the server
driver through uv on first use. The connection is closed at the end of each run.

C# scripts use the same attached connection with `db.Query` and `db.Execute`:

```csharp
// codingpad: connection=codingpad-sample
#:package CodingPad@0.1.0
db.Query("SELECT * FROM customers").Dump();
```

C# parameters can be supplied as a dictionary keyed by the database parameter
name. Queries return at most 10,000 rows and show when results are truncated. The `CodingPad` package supplies the same API for `dotnet run`; without
an attached CodingPad connection, calling `db` reports how to attach one.

TypeScript scripts can attach the same connections. Use `db` directly or import
it from `codingpad`. Await the query and pass the rows to `dump()`, or leave the
awaited query as the last expression to show its table automatically:

```ts
// codingpad: connection=codingpad-sample
await db.query("SELECT * FROM customers LIMIT ?", [10])
```

Bind values as an array and use the database's placeholders (`?` for SQLite,
`$1`, `$2`, … for PostgreSQL, and `@p1`, `@p2`, … for SQL Server).
`db.query(sql, parameters)` returns at most 10,000 rows and exposes `columns`
and `truncated` on the result. Results preserve headers for empty queries and
use suffixes such as `id_2` for duplicate column names.
`db.execute(sql, parameters)` returns the number of affected rows. For a SQL
Server batch, the count is the sum reported across statements.

The first database call opens the connection using the saved settings and vault
password. CodingPad closes it when the run ends. SQLite and PostgreSQL use Bun's
built-in clients. PostgreSQL requires Bun 1.4.4 or later for column metadata.
The first run with a SQL Server connection installs the `mssql` driver through Bun.
Relative SQLite connection paths resolve against the CodingPad process's
working directory across script languages.

Click **Connections…** to add a SQLite, PostgreSQL or SQL Server database, then
**+ SQL** or open a `.sql` file. The first line names the connection; the rest is plain SQL:

```sql
-- codingpad: connection=northwind

SELECT CustomerId, CompanyName FROM Customers LIMIT 100;
```

Results appear as a paged table in Results. Statement failures and missing
connections show a clear error. **Stop** interrupts a running query. Connections
live in the app settings. PostgreSQL (port 5432, TLS preferred) and SQL Server
(port 1433) connections use host, database and user; their passwords are written
only to the OS password vault and referenced by name, never stored in connection
settings or scripts. TLS defaults to **Prefer**; choose **Require** for remote
hosts. Prefer does not protect against an attacker who strips TLS, so it is not a
security guarantee. Enable **Trust the server certificate** for a self-signed or
private-CA server. PostgreSQL decimals, dates and timestamps use their exact server
text; other columns in the same result keep their integer, boolean or float values.
Multi-statement PostgreSQL batches use text for every column (including `t`/`f`
for booleans). SQL scripts keep connection passwords in the Rust host. A script with a generated data context receives its connection password in memory for that run only; it is never written to the generated files, settings or logs. SQL Server
connections can opt in to trusting a self-signed development certificate, and SQL
Server scripts do not accept the `GO` client-side batch separator; run each batch
separately. SQL Server statements without a result set show `Statement completed.`
rather than a rows-affected count. `money` and `smallmoney` are displayed with four
decimal places; the driver decodes them as floating point, so very large `money`
values may lose precision.

## Data contexts (M3.8/M3.9)

A Python or C# script with a connection header also gets a typed **data context**
generated from that connection's schema. Tables become model classes, columns
become typed properties, and foreign keys become relationships or navigation
properties. The context is regenerated for every run and when the schema panel is
refreshed, so editing the database schema is picked up without maintaining model
files by hand. Model sources are generated into a hidden workspace, never into
your script.

```python
# codingpad: connection=northwind
from sqlalchemy import select
from codingpad_data import Customer, DataContext

context = DataContext.connect()
context.session.scalars(select(Customer)).all()
```

```csharp
// codingpad: connection=northwind
var context = new DataContext();
context.Customers.Where(customer => customer.Name.StartsWith("A")).ToList().Dump();
```

Python models use SQLAlchemy 2.0 `Mapped[]` annotations and `relationship()`;
C# models use EF Core entity classes and a `DbContext`. The kernel installs the
language's database packages on first use. basedpyright and Roslyn see the
generated models, so column, table and relationship completions work while you
edit. Passwords stay in the OS vault and are read only while a run or schema
refresh needs them; generated files contain only the schema, never a password.

PostgreSQL C# `interval` and `interval[]` columns map to `NodaTime.Period` and
`NodaTime.Period[]`, which keep months and years, so write
`NodaTime.Period.FromHours(1)` in a filter. Python reads intervals through
psycopg as `timedelta`, which has no months: `1 month` reads as 30 days. C#
columns without a supported EF mapping are omitted from the generated class,
with a comment naming the column, so unavailable data cannot appear as a real
`null` value. A table left with no columns is omitted too. Columns with a
database default or generated value are left out of an insert when you don't set
them, so setting one to `null` also takes the default. Generated and computed columns, and SQL Server
`rowversion`, are read back after every insert and update; the database rejects
writes to them. PostgreSQL `timestamp` columns
(without time zone) take a local or unspecified `DateTime` in C#; Npgsql rejects
a `DateTime` with `Kind=Utc` there, so use a `timestamptz` column for UTC values.

## Exporting tables (M3.10)

Every table dump, whether from Python, C# or a SQL script, has **Export CSV** and
**Export Excel** buttons in its footer. An export writes every row, not just the
pages you have scrolled to, and never shortens text the way the Results panel does.
Rows are read in pages by the app and written straight to the file, so the
interface stays responsive. The footer shows progress and a **Cancel** button.
A cancelled or failed export leaves no file behind; the file appears only when the
export finishes.

- Columns: a Python list of records exports every key found in any row, and a C#
  sequence exports every public property and field of the first item's type.
  Neither is limited to the 32 columns the Results panel shows.
- Values: numbers and booleans stay numeric in Excel, and so do exact decimals
  (`Decimal`, `decimal`, PostgreSQL `numeric`/`money`). Dates and times are ISO
  8601 text; bytes are `0x…` hex; nested lists and objects are JSON.
- CSV files are UTF-8 with a byte-order mark, so Excel on Windows reads accents
  correctly. Fields are not escaped against spreadsheet formulas: a cell starting
  with `=` is exported as it is.
- Excel limits: a sheet holds 1,048,575 data rows, so larger tables must use CSV.
  A cell holds 32,767 characters; longer text is cut, and the footer says how many
  cells were cut.
- C# sequences are read to the end, so an endless sequence exports until you
  cancel. If a sequence throws partway through, the export fails with that error
  instead of producing a file that ends early. SQL results keep their 10,000-row
  limit; a truncated result exports those rows.
- Rerunning or closing the script cancels a running export from that tab.

## Secrets

Click **Secrets…** to add, update or remove named secrets. Values are stored in
the OS password vault (Keychain on macOS, Credential Manager on Windows, Secret
Service on Linux); scripts and settings never contain them. A failed vault write
returns an error instead of silently replacing a value. If the vault is
unavailable, the dialog offers an explicit **Store for this session only** action;
those values disappear when CodingPad exits.

Python and TypeScript scripts read a secret with `secret("name")` (also importable
from `codingpad`). C# scripts use `Secret("name")`, and Rust scripts use
`codingpad::secret("name")`.

```python
from codingpad import secret

api_key = secret("api-key")
```

```ts
const apiKey = secret("api-key");
```

```csharp
var password = Secret("database-password");
```

Names starting with `connection:` are reserved for database connection
passwords. A missing secret raises an actionable error. Secret values are not written to
logs, script files or connection settings. Each run reads only the secrets its
source names. Imports and dynamic uses fall back to every non-connection secret,
because imported modules may call `secret()`. Scripts that neither import anything
nor reference `secret` do not wait on the vault. Changing a secret takes
effect on the next run of each script.

## Repository layout

- `apps/desktop`: React/TypeScript UI and Tauri shell
- `apps/cli`: `cpad run`, the terminal runner over the same Rust core
- `crates/kernel-core`: Rust kernel supervisor, independent of the window
- `kernels/python`: Python stdio kernel
- `kernels/csharp`: C# stdio kernel and pinned Roslyn dependency
- `kernels/typescript`: Bun kernel, worker and dump renderer
- `runtimes/dotnet/CodingPad`: bundled C# `.Dump()` extension library
- `runtimes/python/codingpad`: bundled `dump()` helper (no registry download)
- `runtimes/typescript/codingpad`: bundled TypeScript helper and its declarations
- `samples`: built-in Python, C#, TypeScript and SQL samples plus the bundled SQLite database
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

TypeScript completions, hover and type diagnostics come from the **TypeScript 7
native language server** (`tsgo`) through the same bridge and the same shared
hidden project Run uses. The project maps the bundled `codingpad` helper
declarations and keeps installed versioned imports (`zod@3`) available to the
language server. The server restarts when the prepared project changes. Install
it with `bun add -g @typescript/native-preview`, or point `CODINGPAD_TS_LSP` at
`tsgo`. The hidden project is temporary and never touches your saved script.
The shared project prefers real `@types/bun` types and falls back to a small
checked-in declaration file when it cannot install them.

Rust completions, hover and diagnostics come from **rust-analyzer**. Install
the `rust-analyzer` and `rust-src` toolchain components with rustup.
`CODINGPAD_RUST_LSP` can point to a different rust-analyzer executable.
CodingPad translates diagnostics and completion edits from the editor shadow
project back to the script.

Type errors appear as squiggles and a count in the editor header. Python and
TypeScript execute regardless of editor warnings. Rust must compile before
it can run.

## Performance checks

`just bench` builds the release app and measures performance targets from
D38 on Linux:

| Metric | Target |
|---|---|
| App usable | < 1 s |
| Idle physical footprint | < 475 MiB |
| First Python kernel start | < 3 s |
| Warm Python rerun | < 200 ms |
| 100k-row dump, first page to host | < 1 s |
| Python completion | < 300 ms |
| First C# kernel start | < 3 s |
| Warm C# rerun after an edit | < 500 ms |
| 100k-item `IEnumerable`, first page to host | < 1 s |
| C# completion | < 300 ms |
| TypeScript completion | < 300 ms |
| Warm Rust rerun after an edit | < 3 s |
| Rust completion | < 300 ms |

The first completion run may download basedpyright; later runs reuse the cache.
The C# completion metric needs Roslyn, from the VS Code C# extension or
`CODINGPAD_ROSLYN_LSP`. It waits for Roslyn to load the script, then times the
first completion after an edit. The `IEnumerable` metric runs in a warm kernel
and is timed from the start of the run, so it includes compilation.
The TypeScript completion metric needs the TypeScript 7 native server and Bun;
install `@typescript/native-preview` or set `CODINGPAD_TS_LSP`. It is skipped
when the server is not found.
C# runs use no NuGet packages, so the measurements exclude package downloads.
Rust reruns use the shared target directory. A successful Rust run schedules
idle precompilation of selected common crates already available to Cargo.
Temporary hosts use their own shared target directory.
App readiness requires a mounted, writable Monaco model and registered editor
commands/providers after a paint. Process entry, Tauri setup, WebView loading,
frontend evaluation and editor creation/mount timestamps are printed separately.
Code intelligence starts after the editor paints and remains resident during the
idle sample. The public CodingPad helper stub keeps basedpyright from analyzing
the dump renderer implementation just to understand `dump`.

Idle memory is sampled five seconds after readiness. The 475 MiB gate uses
summed Linux PSS or macOS `phys_footprint`, per D61. The app, WebKit helpers, and
language-server descendants (including separate process groups) are included.
Summed RSS and Linux private memory are printed alongside the gate. Missing
physical-memory data fails the measurement; it never falls back to RSS. On
macOS, close other WebKit apps before testing: XPC helpers are attributed by
comparing their PIDs before and after launch.
The gate was raised from 300 MiB to 475 MiB on 2026-11-05 (D61); the
[2026-11-04 performance report](docs/performance/2026-11-04/README.md) predates
that change.

Python completion warms `os.path.` and times typing `j` followed immediately by
a completion request, including the document-change call. Both responses must
contain `join`. No diagnostic barrier or delay precedes the measured request.
Set `CODINGPAD_LSP_TRACE=1` to log request IDs, document versions and timestamps
at bridge queueing, pipe writes, response receipt and caller delivery. The pipe
write-to-receive interval includes server work and OS scheduling; it is not an
internal server CPU timer. Trace output excludes document and result payloads.

`just bench` runs three sequential repetitions and fails if any measured target
fails in any repetition; `CODINGPAD_BENCH_RUNS` overrides the count for profiling.
Use a release build, a quiet desktop, fixed power/display settings, cached tools
and packages, and no concurrent builds/tests for qualification. These are warm
OS-cache launches, not a disk-cache-flushed cold-start test. Save the full output
and environment with each run; retain earlier failures when recording new results.
The dump metric ends when the first page reaches the Rust host, before rendering.
The app metrics need a desktop session and are skipped on Windows, where a
Windows CI confirms that `just check` passes (D48, D63). Set `CODINGPAD_DESKTOP`
to benchmark a different app binary. The owner has closed M1 and M2.

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
