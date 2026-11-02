# Windows verification — 2 November 2026

Verified the x64 debug desktop executable on Windows 11 Pro 24H2
(build 26100.1742), using master `1ab2b00` plus the fixes in this change.
The VM had 4 virtual CPUs and 8 GiB RAM. Builds and tests ran inside Windows
from an elevated Windows PowerShell 5.1 terminal with process-scoped execution
policy `Bypass`; the desktop smoke test ran unelevated.

## Environment

| Component | Version |
|---|---|
| Python | 3.14.7 |
| Node.js | 26.13.0 |
| npm | 11.20.0 |
| Rust / Cargo | 1.99.1, x86_64-pc-windows-msvc |
| Microsoft C++ compiler | 14.44.35229.0 |
| uv | 0.13.6 |
| just | 1.59.0 |
| WebView2 / Edge WebDriver | 156.0.4314.58 |
| tauri-driver | 2.1.0 |

## Results

`just check` passed in full:

| Check | Result |
|---|---|
| Locked frontend dependency installation | Passed |
| TypeScript and Vite build | Passed |
| Frontend tests | 52 passed |
| Python tests | 48 passed; 1 Unix-only signal test skipped |
| Rust formatting | Passed |
| Workspace Clippy, all targets, warnings denied | Passed |
| Rust unit and integration tests | 64 passed; 5 network acceptance tests ignored by default |
| Native Tauri debug build, without an installer bundle | Passed |

The five optional acceptance tests were then run explicitly and all passed:

```powershell
cargo test -p codingpad-kernel-core --lib toolchains::tests::official_release -- --ignored
cargo test -p codingpad-kernel-core --test language_server -- --ignored
cargo test -p codingpad-kernel-core --test supervisor -- --ignored
```

These cover the official private uv download and Python execution, diagnostics
and completions, inline packages in the language server, warm server reuse and
restart, and cached package environments after a kernel restart. The first cold
language-server dependency download hit the startup timeout; final acceptance
runs used the populated dependency cache.

`just smoke target/debug/codingpad-desktop.exe` also passed from a normal,
unelevated Windows user session. It verified local Monaco loading, printed
output, nested dumps, the final expression, reruns, and an independent second
scratch tab through real Tauri IPC and Python processes. No test driver or
desktop app processes remained after the run.

## Fixes found by verification

- Accept equivalent Windows file URIs in diagnostics (`file:///c%3A/...` and
  `file:///C:/...`), then publish the editor's canonical URI. Keep the existing
  document and version filters, with Windows and real-server regression checks.
- Remove an unused Windows process-extension import and compile the benchmark's
  Unix-only desktop helpers only on Unix, so strict Windows Clippy succeeds.
- Add context to a failed kernel-request flush, including Windows pipe-close
  errors, while preserving recovery through a fresh kernel.
- Expect native child-process line endings in the Python ordering test.
- Wait for the Results status in the desktop smoke test and terminate the
  Windows WebDriver process tree during cleanup.

Linux cross-checks also passed for kernel Clippy, kernel unit/integration tests,
and the focused Python output-ordering regression. Full Linux workspace Clippy
was blocked by the host's missing `libdbus-1` development package.
