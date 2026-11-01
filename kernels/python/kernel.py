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
_protocol_in = os.fdopen(os.dup(sys.stdin.fileno()), "r", encoding="utf-8")


def redirect_fd(source, target):
    os.dup2(source, target)
    if os.name == "nt":
        # CRT descriptors and inherited Win32 standard handles must agree.
        import ctypes
        import msvcrt
        from ctypes import wintypes

        set_handle = ctypes.windll.kernel32.SetStdHandle
        set_handle.argtypes = [wintypes.DWORD, wintypes.HANDLE]
        set_handle.restype = wintypes.BOOL
        if not set_handle(-10 - target, msvcrt.get_osfhandle(target)):
            raise ctypes.WinError()


# Only the private duplicates can access the protocol, including between runs.
with open(os.devnull, "r+b", buffering=0) as sink:
    redirect_fd(sink.fileno(), 0)
    redirect_fd(sink.fileno(), 1)
_write_lock = threading.Lock()


def json_safe(value):
    if isinstance(value, str):
        return value.encode("utf-8", "replace").decode("utf-8")
    if isinstance(value, dict):
        return {json_safe(key): json_safe(item) for key, item in value.items()}
    if isinstance(value, list):
        return [json_safe(item) for item in value]
    return value


def send(message):
    with _write_lock:
        _protocol_out.write(
            json.dumps(
                json_safe({"jsonrpc": "2.0", **message}),
                ensure_ascii=True,
                allow_nan=False,
            )
            + "\n"
        )
        _protocol_out.flush()


def notify(method, params):
    send({"method": method, "params": params})


def write_all(descriptor, data):
    while data:
        written = os.write(descriptor, data)
        data = data[written:]


@contextlib.contextmanager
def capture_native(run_id):
    """One pipe per stream orders Python/native writes and batches fragments."""
    streams = []
    flush_gate = threading.Lock()
    live = True

    def pump(reader, name, active, gate, marker, drained):
        decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")
        pending = b""

        def emit(data, final=False):
            text = decoder.decode(data, final=final)
            if text:
                notify("stream", {"runId": run_id, "stream": name, "text": text})

        try:
            while data := os.read(reader, 8192):
                with gate:
                    if not active.is_set():
                        break
                    pending += data
                    while True:
                        before, separator, after = pending.partition(marker)
                        if separator:
                            emit(before)
                            drained.set()
                            pending = after
                            continue
                        # Retain only a possible split marker, not ordinary text.
                        keep = next(
                            (
                                size
                                for size in range(
                                    min(len(pending), len(marker) - 1), 0, -1
                                )
                                if pending.endswith(marker[:size])
                            ),
                            0,
                        )
                        emit(pending[:-keep] if keep else pending)
                        pending = pending[-keep:] if keep else b""
                        break
            with gate:
                if active.is_set():
                    emit(pending, final=True)
        finally:
            os.close(reader)

    def drain():
        # Fences share the output pipes, so acknowledgement means every earlier
        # write on that stream reached the supervisor before a dump/flush.
        with flush_gate:
            if not live:
                return
            for descriptor, _, _, _, _, marker, drained in streams:
                drained.clear()
                try:
                    write_all(descriptor, marker)
                except OSError:
                    drained.set()  # Script may have closed its own stdout/stderr.
            for *_, drained in streams:
                drained.wait(timeout=0.5)

    try:
        for descriptor, name in [(1, "stdout"), (2, "stderr")]:
            reader, writer = os.pipe()
            original = os.dup(descriptor)
            redirect_fd(writer, descriptor)
            os.close(writer)
            active, gate, drained = (
                threading.Event(),
                threading.Lock(),
                threading.Event(),
            )
            marker = b"\0codingpad:" + os.urandom(16).hex().encode("ascii") + b"\0"
            active.set()
            thread = threading.Thread(
                target=pump,
                args=(reader, name, active, gate, marker, drained),
                daemon=True,
            )
            streams.append(
                (descriptor, original, thread, active, gate, marker, drained)
            )
            thread.start()
        yield drain
    finally:
        drain()
        with flush_gate:
            live = False
        for descriptor, original, *_ in streams:
            redirect_fd(original, descriptor)
            os.close(original)
        for _, _, thread, active, gate, _, _ in streams:
            # Background children may retain a pipe; never wait for them forever.
            thread.join(timeout=0.5)
            with gate:
                active.clear()


class Stream(io.TextIOBase):
    def __init__(self, descriptor, drain):
        self.descriptor = descriptor
        self.drain = drain

    @property
    def encoding(self):
        return "utf-8"

    def fileno(self):
        return self.descriptor

    def writable(self):
        return True

    def write(self, text):
        write_all(self.descriptor, text.encode("utf-8", "replace"))
        return len(text)

    def flush(self):
        self.drain()


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
    previous_input = sys.stdin
    sys.stdin = NoInput()
    ok = False
    try:
        with (
            capture_native(run_id) as drain,
            contextlib.redirect_stdout(Stream(1, drain)),
            contextlib.redirect_stderr(Stream(2, drain)),
        ):

            def emit_dump(node):
                drain()
                notify("dump", {"runId": run_id, "node": node})

            codingpad._sink = emit_dump
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
