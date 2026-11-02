"""Exercise the built desktop through tauri-driver (Linux/Windows).

Requires tauri-driver on PATH plus WebKitWebDriver on Linux or msedgedriver on
Windows. On a headless Linux machine run under xvfb-run. No Python dependencies.
"""

import argparse
import json
import os
import subprocess
import time
from http.client import HTTPException
from pathlib import Path
from urllib.error import URLError
from urllib.request import Request, urlopen


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("application", type=Path)
    args = parser.parse_args()
    process = subprocess.Popen(["tauri-driver", "--port", "4444"])
    session = None

    def request(method, path, data=None):
        payload = json.dumps(data).encode() if data is not None else None
        with urlopen(
            Request(
                "http://127.0.0.1:4444" + path,
                data=payload,
                method=method,
                headers={"Content-Type": "application/json"},
            ),
            timeout=30,
        ) as response:
            result = json.load(response)["value"]
        if isinstance(result, dict) and "error" in result:
            raise RuntimeError(result)
        return result

    def wait_for(check, description):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if check():
                return
            time.sleep(0.1)
        raise AssertionError(f"Timed out waiting for {description}")

    def execute(script):
        return request(
            "POST", f"/session/{session}/execute/sync", {"script": script, "args": []}
        )

    try:

        def ready():
            try:
                return request("GET", "/status")["ready"]
            except (URLError, HTTPException, ConnectionError, TimeoutError):
                return False

        wait_for(ready, "tauri-driver")
        session = request(
            "POST",
            "/session",
            {
                "capabilities": {
                    "alwaysMatch": {
                        "browserName": "wry",
                        "tauri:options": {
                            "application": str(args.application.resolve())
                        },
                    }
                }
            },
        )["sessionId"]
        wait_for(
            lambda: execute(
                "return !!document.querySelector('.monaco-editor .view-line')"
            ),
            "Monaco editor",
        )
        for _ in range(2):
            execute("document.querySelector('.run-button').click()")
            wait_for(
                lambda: execute(
                    "return document.querySelector('.results-pane [role=status]').textContent === 'Finished'"
                ),
                "Python run",
            )
            result = execute("return document.querySelector('.results').textContent")
            for expected in ["Hello from CodingPad!", "Ada", "languages", "42"]:
                assert expected in result, (expected, result)
        execute("document.querySelector('[aria-label=\"New scratch script\"]').click()")
        assert execute("return document.querySelectorAll('.tab').length") == 2
        execute("document.querySelector('.run-button').click()")
        wait_for(
            lambda: execute(
                "return document.querySelector('.results-pane [role=status]').textContent === 'Finished'"
            ),
            "second script kernel",
        )
        print(
            "Desktop smoke passed: local Monaco, print, nested dump, final expression, rerun, independent tab."
        )
    finally:
        try:
            if session:
                request("DELETE", f"/session/{session}")
        finally:
            if os.name == "nt":
                # Terminating only tauri-driver leaves msedgedriver holding logs open.
                subprocess.run(
                    ["taskkill", "/PID", str(process.pid), "/T", "/F"],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    check=False,
                )
            else:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


if __name__ == "__main__":
    main()
