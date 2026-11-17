"""CodingPad protocol v1: newline-delimited JSON-RPC 2.0 on stdio."""

import _thread
import ast
import builtins
import codecs
import contextlib
import importlib
import io
import json
import linecache
import os
import signal
import sys
import threading
import traceback
import types
from pathlib import Path
from queue import SimpleQueue, Queue

# -I isolates us from user site packages and PYTHONPATH. Add only our bundled helper.
sys.path.insert(0, str(Path(__file__).resolve().parent / "runtime"))
import codingpad  # noqa: E402

_protocol_out = os.fdopen(
    os.dup(sys.stdout.fileno()), "w", encoding="utf-8", buffering=1
)
_protocol_in = os.fdopen(os.dup(sys.stdin.fileno()), "r", encoding="utf-8")
_KERNEL_FILE = os.path.realpath(__file__)
_MAX_MESSAGE = 10000


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
_write_lock = threading.RLock()
_io_state = threading.local()
_io_state.busy = False


@contextlib.contextmanager
def io_busy():
    previous = getattr(_io_state, "busy", False)
    _io_state.busy = True
    try:
        yield
    finally:
        _io_state.busy = previous


class IOThread(threading.Thread):
    """Identify output workers before their first allocation can run a finalizer."""


def json_safe(value):
    if isinstance(value, str):
        return value.encode("utf-8", "replace").decode("utf-8")
    if isinstance(value, dict):
        return {json_safe(key): json_safe(item) for key, item in value.items()}
    if isinstance(value, list):
        return [json_safe(item) for item in value]
    return value


def send(message):
    with io_busy(), _write_lock:
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


class Fence:
    """Match each drain to its own acknowledgement, including after timeouts."""

    def __init__(self):
        self.prefix = b"\0codingpad:" + os.urandom(16).hex().encode("ascii") + b":"
        self.issued = 0
        self.acknowledged = 0
        self.condition = threading.Condition()

    def issue(self):
        with self.condition:
            self.issued += 1
            return self.issued, self.prefix + self.issued.to_bytes(8, "big") + b"\0"

    def acknowledge(self, sequence):
        with self.condition:
            self.acknowledged = max(self.acknowledged, sequence)
            self.condition.notify_all()

    def wait(self, sequence):
        with self.condition:
            return self.condition.wait_for(
                lambda: self.acknowledged >= sequence, timeout=0.5
            )


@contextlib.contextmanager
def capture_native(run_id):
    """One pipe per stream orders Python/native writes and batches fragments."""
    streams = []
    flush_gate = threading.Lock()
    live = True
    write_gate = threading.Lock()
    last_stream = None
    deferred = SimpleQueue()

    def busy():
        # The role survives across runs if an old reader is still shutting down.
        return getattr(_io_state, "busy", False) or isinstance(
            threading.current_thread(), IOThread
        )

    def deferred_pump():
        while (record := deferred.get()) is not None:
            if isinstance(record, threading.Event):
                record.set()
                continue
            descriptor, data = record
            # Share the lifecycle check and protocol write lock: no deferred
            # notification may escape after this run has finished.
            with _write_lock:
                if live:
                    notify(
                        "stream",
                        {
                            "runId": run_id,
                            "stream": "stdout" if descriptor == 1 else "stderr",
                            "text": data.decode("utf-8", "replace"),
                        },
                    )

    deferred_thread = IOThread(target=deferred_pump, daemon=True)
    deferred_started = False

    def pump(reader, name, active, gate, fence):
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
                    while pending:
                        position = pending.find(fence.prefix)
                        if position >= 0:
                            emit(pending[:position])
                            pending = pending[position:]
                            frame_size = len(fence.prefix) + 9
                            if len(pending) < frame_size:
                                break  # Prefix/sequence may span multiple reads.
                            if pending[frame_size - 1] != 0:
                                emit(pending[:1])
                                pending = pending[1:]
                                continue
                            sequence = int.from_bytes(
                                pending[len(fence.prefix) : frame_size - 1], "big"
                            )
                            fence.acknowledge(sequence)
                            pending = pending[frame_size:]
                            continue
                        # Retain only a possible split prefix, not ordinary text.
                        keep = next(
                            (
                                size
                                for size in range(
                                    min(len(pending), len(fence.prefix) - 1), 0, -1
                                )
                                if pending.endswith(fence.prefix[:size])
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

    def _drain(only_descriptor=None):
        # Never wait for a gate held by another writer/callback. Sequence
        # acknowledgements still protect each fence whenever a drain proceeds.
        if not flush_gate.acquire(blocking=False):
            return
        try:
            if not live:
                return
            waiting = []
            for descriptor, _, _, _, _, fence in streams:
                if only_descriptor is not None and descriptor != only_descriptor:
                    continue
                sequence, marker = fence.issue()
                try:
                    write_all(descriptor, marker)
                except OSError:
                    continue
                waiting.append((fence, sequence))
            for fence, sequence in waiting:
                fence.wait(sequence)
        finally:
            flush_gate.release()

    def drain(only_descriptor=None):
        if busy():
            return
        with io_busy():
            _drain(only_descriptor)

    def ordered_write(descriptor, data):
        nonlocal last_stream
        if not data or not live:
            return
        if busy():
            # CPython's SimpleQueue.put is reentrant, including in __del__ and
            # weakref callbacks. Never write to a pipe its own pump must read.
            deferred.put((descriptor, data))
            return
        with io_busy():
            if not write_gate.acquire(blocking=False):
                deferred.put((descriptor, data))
                return
            try:
                if last_stream is not None and last_stream != descriptor:
                    _drain(last_stream)
                write_all(descriptor, data)
                last_stream = descriptor
            finally:
                write_gate.release()

    try:
        deferred_thread.start()
        deferred_started = True
        for descriptor, name in [(1, "stdout"), (2, "stderr")]:
            reader, writer = os.pipe()
            original = os.dup(descriptor)
            redirect_fd(writer, descriptor)
            os.close(writer)
            active, gate, fence = threading.Event(), threading.Lock(), Fence()
            active.set()
            thread = IOThread(
                target=pump,
                args=(reader, name, active, gate, fence),
                daemon=True,
            )
            streams.append((descriptor, original, thread, active, gate, fence))
            thread.start()
        yield drain, ordered_write
    finally:
        with io_busy():
            try:
                _drain()
                if deferred_started:
                    complete = threading.Event()
                    deferred.put(complete)
                    complete.wait(timeout=0.5)
            finally:
                with _write_lock:
                    live = False
                for descriptor, original, *_ in streams:
                    redirect_fd(original, descriptor)
                    os.close(original)
                for _, _, thread, active, gate, _ in streams:
                    thread.join(timeout=0.5)
                    with gate:
                        active.clear()
                if deferred_started:
                    deferred.put(None)
                    deferred_thread.join(timeout=0.5)


class Stream(io.TextIOBase):
    def __init__(self, descriptor, drain, ordered_write):
        self.descriptor = descriptor
        self.drain = drain
        self.ordered_write = ordered_write

    @property
    def encoding(self):
        return "utf-8"

    def fileno(self):
        return self.descriptor

    def writable(self):
        return True

    def write(self, text):
        self.ordered_write(self.descriptor, text.encode("utf-8", "replace"))
        return len(text)

    def flush(self):
        self.drain()


class NoInput(io.TextIOBase):
    def readline(self, *args):
        raise RuntimeError("Interactive input is not supported in CodingPad yet")


def safe_message(error):
    try:
        return str(error)[:_MAX_MESSAGE]
    except Exception:
        return "<exception str() failed>"


def error_causes(error):
    """Describe the explicit cause or implicit context chain, newest first."""
    causes = []
    seen = {id(error)}
    current = error.__cause__
    if current is None and not getattr(error, "__suppress_context__", False):
        current = error.__context__
    while current is not None and id(current) not in seen and len(causes) < 5:
        seen.add(id(current))
        causes.append(
            {"type": type(current).__name__, "message": safe_message(current)}
        )
        next_error = current.__cause__
        if next_error is None and not getattr(current, "__suppress_context__", False):
            next_error = current.__context__
        current = next_error
    return causes


def error_node(error, filename):
    """Turn a script exception into a dump node with clickable script frames."""
    if isinstance(error, SyntaxError):
        frames = [
            {
                "file": error.filename or filename,
                "line": error.lineno or 0,
                "column": error.offset or 0,
                "function": "<module>",
                "text": (error.text or "").strip(),
                "script": (error.filename or filename) == filename,
            }
        ]
    else:
        frames = []
        for frame in traceback.extract_tb(error.__traceback__)[-30:]:
            if os.path.realpath(frame.filename) == _KERNEL_FILE:
                continue
            frames.append(
                {
                    "file": frame.filename,
                    "line": frame.lineno or 0,
                    "column": 0,
                    "function": frame.name,
                    "text": (frame.line or "").strip(),
                    "script": frame.filename == filename,
                }
            )
    return {
        "id": codingpad._node_id(),
        "kind": "error",
        "type": type(error).__name__,
        "message": safe_message(error),
        "traceback": frames,
        "causes": error_causes(error),
    }


@contextlib.contextmanager
def script_timers():
    # Interval timers otherwise keep invoking user handlers during cleanup and
    # subsequent requests in this warm interpreter. Stop them before restoring
    # the ordinary idle streams; the handler may itself print from any bytecode.
    stop = getattr(signal, "setitimer", None)
    timers = [
        getattr(signal, name)
        for name in ("ITIMER_REAL", "ITIMER_VIRTUAL", "ITIMER_PROF")
        if hasattr(signal, name)
    ]
    try:
        yield
    finally:
        if stop is not None:
            with io_busy():
                for timer in timers:
                    stop(timer, 0)


# No tracing on normal runs. Only an actual cancellation delivers SIGINT.
_cancel_gate = threading.RLock()
_executing_cancel = None
_main_ident = threading.get_ident()


def check_cancelled(event):
    if event.is_set():
        event.clear()
        raise KeyboardInterrupt("Run stopped")


def handle_interrupt(signum, frame):
    with _cancel_gate:
        event = _executing_cancel
        if event is None or not event.is_set():
            return
        # A queued interrupt can arrive after exec returned. Never raise in
        # request teardown or carry an old interrupt into the next run.
        while frame is not None:
            if frame.f_code.co_filename == "<codingpad>":
                check_cancelled(event)
                return
            frame = frame.f_back


@contextlib.contextmanager
def script_cancellation(event):
    global _executing_cancel
    try:
        with _cancel_gate:
            signal.signal(signal.SIGINT, handle_interrupt)
            _executing_cancel = event
        check_cancelled(event)
        yield
    finally:
        with _cancel_gate:
            _executing_cancel = None
            # Scripts may have replaced SIGINT. Ignore late signals while idle.
            signal.signal(signal.SIGINT, handle_interrupt)


def cancel_run(event):
    with _cancel_gate:
        event.set()
        if _executing_cancel is event:
            if os.name == "posix" and hasattr(signal, "pthread_kill"):
                signal.pthread_kill(_main_ident, signal.SIGINT)
            else:
                _thread.interrupt_main()


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
            "cache": codingpad.cache,
            "secret": codingpad.secret,
        }
    )
    database = codingpad._Database(request["params"].get("connection"))
    scope["db"] = database
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
            capture_native(run_id) as (drain, ordered_write),
            contextlib.redirect_stdout(Stream(1, drain, ordered_write)),
            contextlib.redirect_stderr(Stream(2, drain, ordered_write)),
            script_timers(),
        ):

            def emit_dump(node):
                drain()
                notify("dump", {"runId": run_id, "node": node})

            def emit_update(node_id, node):
                drain()
                notify(
                    "dump_update",
                    {"runId": run_id, "nodeId": node_id, "node": node},
                )

            secrets = request["params"].get("secrets") or {}
            errors = request["params"].get("secret_errors") or {}
            if not isinstance(secrets, dict) or not isinstance(errors, dict):
                raise ValueError("run params.secrets and secret_errors must be objects")
            codingpad._set_secrets(secrets, errors)
            codingpad._begin(
                emit_dump,
                emit_update,
                run_id,
                eager=request["params"].get("eager") is True,
            )
            try:
                data = request["params"].get("data")
                if isinstance(data, dict):
                    module_source = data.get("source")
                    if not isinstance(module_source, str) or not module_source.strip():
                        raise ValueError("run params.data.source must contain generated model code")
                    module_path = (
                        Path(__file__).resolve().parent / "runtime" / "codingpad_data.py"
                    )
                    module_path.write_text(module_source, encoding="utf-8")
                    importlib.invalidate_caches()
                    sys.modules.pop("codingpad_data", None)
                    try:
                        import codingpad_data
                    except BaseException as error:
                        sys.modules.pop("codingpad_data", None)
                        notify(
                            "stream",
                            {
                                "runId": run_id,
                                "stream": "stderr",
                                "text": f"Generated data context was ignored: {error}\n",
                            },
                        )
                    else:
                        codingpad._set_database_connection(
                            data.get("connection") or {}
                        )
                        scope["context"] = codingpad_data.DataContext(
                            codingpad._connect_data_context()
                        )
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
                cancelled = request["_cancel"]
                # One entry check closes the gap between enabling cancellation
                # and entering exec. Preserve docstrings and future directives.
                check_name = "__codingpad_check_cancel__"
                scope[check_name] = lambda: check_cancelled(cancelled)
                index = 0
                if (
                    tree.body
                    and isinstance(tree.body[0], ast.Expr)
                    and isinstance(tree.body[0].value, ast.Constant)
                    and isinstance(tree.body[0].value.value, str)
                ):
                    index = 1
                while (
                    index < len(tree.body)
                    and isinstance(tree.body[index], ast.ImportFrom)
                    and tree.body[index].module == "__future__"
                ):
                    index += 1
                check = ast.Expr(ast.Call(ast.Name(check_name, ast.Load()), [], []))
                if index < len(tree.body):
                    ast.copy_location(check, tree.body[index])
                else:
                    check.lineno, check.col_offset = 1, 0
                tree.body.insert(index, check)
                ast.fix_missing_locations(tree)
                code = compile(tree, filename, "exec")
                with script_cancellation(cancelled):
                    exec(code, scope)
                ok = True
            except BaseException as error:
                # Finish preceding output before reporting the script failure.
                # SystemExit/KeyboardInterrupt must not kill the kernel either.
                emit_dump(error_node(error, filename))
    finally:
        database.close()
        codingpad._end()
        sys.stdin = previous_input
        sys.modules["__main__"] = previous_main
    notify("run_finished", {"runId": run_id, "ok": ok})
    send({"id": run_id, "result": {"ok": ok}})


def expand(request, export=False):
    params = request["params"]
    handle = params.get("handle")
    if not isinstance(handle, str):
        raise ValueError("expand requires params.handle as a string")
    cursor = params.get("cursor")
    if cursor is not None and not isinstance(cursor, str):
        raise ValueError("params.cursor must be a string or null")
    limit = params.get("limit")
    if limit is not None and not isinstance(limit, int):
        raise ValueError("params.limit must be an integer or null")
    try:
        if export:
            page = codingpad._export_rows(handle, cursor, limit)
        else:
            node = codingpad._expand(handle, cursor, limit)
    except KeyError:
        send(
            {
                "id": request["id"],
                "error": {
                    "code": -32001,
                    "message": "That preview is no longer available. Run the script again.",
                },
            }
        )
        return
    except BaseException as error:
        # User dunders can raise (or exit) while previewing; keep the kernel serving.
        send(
            {
                "id": request["id"],
                "error": {
                    "code": -32603,
                    "message": f"Could not export this table: {type(error).__name__}: {error}"
                    if export
                    else f"Could not expand this value: {type(error).__name__}",
                },
            }
        )
        return
    send({"id": request["id"], "result": page if export else {"node": node}})


def main():
    signal.signal(signal.SIGINT, handle_interrupt)
    requests = Queue()
    cancellations = {}
    gate = threading.Lock()

    def read_requests():
        try:
            for line in _protocol_in:
                cancelled = None
                try:
                    message = json.loads(line)
                    if isinstance(message, dict) and message.get("jsonrpc") == "2.0":
                        if message.get("method") == "cancel":
                            params = message.get("params")
                            if isinstance(params, dict) and isinstance(
                                params.get("runId"), int
                            ):
                                with gate:
                                    event = cancellations.get(params["runId"])
                                    if event is not None:
                                        cancel_run(event)
                            continue
                        if message.get("method") == "run" and isinstance(
                            message.get("id"), int
                        ):
                            cancelled = threading.Event()
                            with gate:
                                cancellations[message["id"]] = cancelled
                except (ValueError, TypeError):
                    pass
                requests.put((line, cancelled))
        finally:
            requests.put(None)

    threading.Thread(target=read_requests, daemon=True).start()
    while (record := requests.get()) is not None:
        line, cancelled = record
        request = None
        try:
            request = json.loads(line)
            if (
                not isinstance(request, dict)
                or request.get("jsonrpc") != "2.0"
                or not isinstance(request.get("id"), int)
            ):
                raise ValueError("Expected a JSON-RPC request with an integer id")
            method = request.get("method")
            if method not in {"run", "expand", "export_rows"}:
                send(
                    {
                        "id": request["id"],
                        "error": {"code": -32601, "message": "Unknown method"},
                    }
                )
                continue
            params = request.get("params")
            if method == "run":
                if not isinstance(params, dict) or not isinstance(
                    params.get("source"), str
                ):
                    raise ValueError("run requires params.source as a string")
                try:
                    request["_cancel"] = cancelled
                    run(request)
                except BaseException as error:
                    # Execution/cleanup failures are request errors, not malformed
                    # input and not a reason to terminate the warm kernel.
                    send(
                        {
                            "id": request["id"],
                            "error": {
                                "code": -32603,
                                "message": f"Kernel run failed: {type(error).__name__}",
                            },
                        }
                    )
            else:
                if not isinstance(params, dict):
                    raise ValueError(f"{method} requires params")
                expand(request, export=method == "export_rows")
        except (ValueError, TypeError) as error:
            send(
                {
                    "id": request.get("id") if isinstance(request, dict) else None,
                    "error": {"code": -32600, "message": str(error)},
                }
            )
        finally:
            if cancelled is not None:
                with gate:
                    cancellations.pop(request["id"], None)


if __name__ == "__main__":
    main()
