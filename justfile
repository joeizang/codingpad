# Build and test every component (.NET 10 SDK required). CI runs these same recipes (D63).
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
    cargo fmt --manifest-path runtimes/rust/codingpad/Cargo.toml -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo clippy --manifest-path runtimes/rust/codingpad/Cargo.toml --all-targets --locked -- -D warnings
    cargo test --workspace --locked
    cargo test --manifest-path runtimes/rust/codingpad/Cargo.toml --locked
    npm --prefix apps/desktop run tauri -- build --debug --no-bundle

run:
    npm --prefix apps/desktop ci
    npm --prefix apps/desktop run tauri -- dev

# Build this platform's installer under target/release/bundle (D29, D39).
bundle:
    npm --prefix apps/desktop ci
    npm --prefix apps/desktop run tauri -- build

# Measure the M1, M2 and M5 speed targets (D38, D48, D55). Needs a desktop session; Windows skips app metrics.
bench:
    npm --prefix apps/desktop ci
    npm --prefix apps/desktop run tauri -- build --no-bundle
    cargo run --release --locked -p codingpad-kernel-core --bin codingpad-bench

# Optional native UI smoke: install tauri-driver and the platform WebDriver first.
# For a headless Linux desktop, prefix the command with xvfb-run.
smoke application="target/debug/codingpad-desktop":
    uv run --offline --no-project --no-config --no-python-downloads python tools/smoke-desktop.py {{quote(application)}}
