# Build and test every component locally (.NET 10 SDK required). No CI required.
set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

default:
    @just --list

check:
    npm --prefix apps/desktop ci
    npm --prefix apps/desktop run build
    npm --prefix apps/desktop test
    uv run --offline --no-project --no-config --no-python-downloads python -m unittest discover -s tests/python -v
    uv run --offline --no-project --no-config --no-python-downloads python -m unittest discover -s tests/dotnet -v
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked
    npm --prefix apps/desktop run tauri -- build --debug --no-bundle

run:
    npm --prefix apps/desktop ci
    npm --prefix apps/desktop run tauri -- dev

# Build the installer for this platform (D29, D39). Output lands under
# target/release/bundle: appimage/, deb/ and rpm/ on Linux, nsis/ on Windows,
# dmg/ on macOS. Run this on each platform; nothing is cross-built.
bundle:
    npm --prefix apps/desktop ci
    npm --prefix apps/desktop run tauri -- build

# Measure the M1 and M2 speed targets (D38, D48, D55). Needs a desktop session; Windows skips app metrics.
bench:
    npm --prefix apps/desktop ci
    npm --prefix apps/desktop run tauri -- build --no-bundle
    cargo run --release --locked -p codingpad-kernel-core --bin codingpad-bench

# Optional native UI smoke: install tauri-driver and the platform WebDriver first.
# For a headless Linux desktop, prefix the command with xvfb-run.
smoke application="target/debug/codingpad-desktop":
    uv run --offline --no-project --no-config --no-python-downloads python tools/smoke-desktop.py {{quote(application)}}
