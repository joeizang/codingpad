"""CodingPad protocol v0: newline-delimited JSON-RPC 2.0 on stdio."""

import ast
import builtins
import codecs
import contextlib
import io
import json
import linecache
import os
import sys
import threading
import traceback
import types
from pathlib import Path

# -I isolates us from user site packages and PYTHONPATH. Add only our bundled helper.
sys.path.insert(0, str(Path(__file__).resolve().parent / "runtime"))
import codingpad  # noqa: E402

_protocol_out = os.fdopen(
    os.dup(sys.stdout.fileno()), "w", encoding="utf-8", buffering=1
)
_protocol_in = sys.stdin
_write_lock = threading.Lock()


def send(message):
    with _write_lock:
        _protocol_out.write(
            json.dumps(
                {"jsonrpc": "2.0", **message}, ensure_ascii=True, allow_nan=False
            )
            + "\n"
        )
        _protocol_out.flush()


def notify(method, params):
    send({"method": method, "params": params})


@contextlib.contextmanager
def capture_native(run_id):
    """Keep subprocess/native writes off the JSON-RPC fd and stream them too."""
    streams = []

    def pump(reader, name, active, gate):
        decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")
        try:
            while data := os.read(reader, 4096):
                with gate:
                    if not active.is_set():
                        break
                    notify(
                        "stream",
                        {"runId": run_id, "stream": name, "text": decoder.decode(data)},
                    )
            tail = decoder.decode(b"", final=True)
            with gate:
                if tail and active.is_set():
                    notify("stream", {"runId": run_id, "stream": name, "text": tail})
        finally:
            os.close(reader)

    try:
        for descriptor, name in [(1, "stdout"), (2, "stderr")]:
            reader, writer = os.pipe()
            original = os.dup(descriptor)
            os.dup2(writer, descriptor)
            os.close(writer)
            active, gate = threading.Event(), threading.Lock()
            active.set()
            thread = threading.Thread(
                target=pump, args=(reader, name, active, gate), daemon=True
            )
            streams.append((descriptor, original, thread, active, gate))
            thread.start()
        yield
    finally:
        for descriptor, original, *_ in streams:
            os.dup2(original, descriptor)
            os.close(original)
        for _, _, thread, active, gate in streams:
            # A background subprocess can retain a pipe. Do not let it delay the
            # response indefinitely or emit output attributed to a later run.
            thread.join(timeout=0.5)
            with gate:
                active.clear()


class Stream(io.TextIOBase):
    def __init__(self, run_id, name):
        self.run_id = run_id
        self.name = name

    @property
    def encoding(self):
        return "utf-8"

    def writable(self):
        return True

    def write(self, text):
        if text:
            notify("stream", {"runId": self.run_id, "stream": self.name, "text": text})
        return len(text)

    def flush(self):
        pass


class NoInput(io.TextIOBase):
    def readline(self, *args):
        raise RuntimeError("Interactive input is not supported in CodingPad yet")


def run(request):
    run_id = request["id"]
    source = request["params"]["source"]
    filename = "<codingpad>"
    linecache.cache[filename] = (
        len(source),
        None,
        source.splitlines(keepends=True),
        filename,
    )
    # A new globals dict and builtins dict each run; imports/process stay warm.
    module = types.ModuleType("__main__")
    scope = module.__dict__
    scope.update(
        {
            "__file__": filename,
            "__builtins__": dict(vars(builtins)),
            "dump": codingpad.dump,
        }
    )
    auto_name = "__codingpad_auto_dump__"
    callable_name = "__codingpad_last_callable__"

    def auto_dump(value, function=None):
        if value is not None and function is not codingpad.dump:
            codingpad.dump(value)
        return value

    scope[auto_name] = auto_dump
    previous_main = sys.modules["__main__"]
    sys.modules["__main__"] = module
    codingpad._sink = lambda node: notify("dump", {"runId": run_id, "node": node})
    previous_input = sys.stdin
    sys.stdin = NoInput()
    ok = False
    try:
        with (
            capture_native(run_id),
            contextlib.redirect_stdout(Stream(run_id, "stdout")),
            contextlib.redirect_stderr(Stream(run_id, "stderr")),
        ):
            try:
                tree = ast.parse(source, filename=filename)
                if tree.body and isinstance(tree.body[-1], ast.Expr):
                    last = tree.body[-1]
                    # Keep calls in the script's own frame (locals/eval/vars
                    # depend on it). Remember the callable without evaluating it
                    # twice, to suppress duplicate output from dump aliases.
                    value = last.value
                    arguments = [value]
                    if isinstance(value, ast.Call):
                        value.func = ast.NamedExpr(
                            ast.Name(callable_name, ast.Store()), value.func
                        )
                        arguments.append(ast.Name(callable_name, ast.Load()))
                    wrapped = ast.Call(ast.Name(auto_name, ast.Load()), arguments, [])
                    tree.body[-1] = ast.copy_location(ast.Expr(wrapped), last)
                    ast.fix_missing_locations(tree)
                exec(compile(tree, filename, "exec"), scope)
                ok = True
            except BaseException:
                # SystemExit/KeyboardInterrupt in a script must not kill the kernel.
                traceback.print_exc()
    finally:
        codingpad._sink = None
        sys.stdin = previous_input
        sys.modules["__main__"] = previous_main
    notify("run_finished", {"runId": run_id, "ok": ok})
    send({"id": run_id, "result": {"ok": ok}})


def main():
    for line in _protocol_in:
        request = None
        try:
            request = json.loads(line)
            if (
                not isinstance(request, dict)
                or request.get("jsonrpc") != "2.0"
                or not isinstance(request.get("id"), int)
            ):
                raise ValueError("Expected a JSON-RPC request with an integer id")
            if request.get("method") != "run":
                send(
                    {
                        "id": request["id"],
                        "error": {"code": -32601, "message": "Unknown method"},
                    }
                )
                continue
            params = request.get("params")
            if not isinstance(params, dict) or not isinstance(
                params.get("source"), str
            ):
                raise ValueError("run requires params.source as a string")
            run(request)
        except (ValueError, TypeError) as error:
            send(
                {
                    "id": request.get("id") if isinstance(request, dict) else None,
                    "error": {"code": -32600, "message": str(error)},
                }
            )


if __name__ == "__main__":
    main()
