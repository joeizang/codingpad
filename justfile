# Build and test every component locally. No CI required.
set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

default:
    @just --list

check:
    npm --prefix apps/desktop ci
    npm --prefix apps/desktop run build
    npm --prefix apps/desktop test
    uv run --offline --no-project --no-config --no-python-downloads python -m unittest discover -s tests/python -v
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked
    npm --prefix apps/desktop run tauri -- build --debug --no-bundle

run:
    npm --prefix apps/desktop ci
    npm --prefix apps/desktop run tauri -- dev

# Optional native UI smoke: install tauri-driver and the platform WebDriver first.
# For a headless Linux desktop, prefix the command with xvfb-run.
smoke application="target/debug/codingpad-desktop":
    uv run --offline --no-project --no-config --no-python-downloads python tools/smoke-desktop.py {{quote(application)}}
