import json
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class KernelTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        folder = Path(self.directory.name)
        shutil.copy(ROOT / "kernels/python/kernel.py", folder / "kernel.py")
        shutil.copytree(
            ROOT / "runtimes/python/codingpad", folder / "runtime/codingpad"
        )
        self.command = [
            sys.executable,
            "-I",
            "-u",
            "-X",
            "utf8",
            str(folder / "kernel.py"),
        ]
        self.process = subprocess.Popen(
            self.command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )
        self.addCleanup(self.cleanup)
        self.request_id = 0

    def cleanup(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.process.stdout.close()
        self.process.stderr.close()
        self.directory.cleanup()

    def run_script(self, source, timeout=10):
        # Every request is bounded so a kernel deadlock fails the suite instead
        # of hanging the test runner indefinitely.
        watchdog = threading.Timer(timeout, self.process.kill)
        watchdog.daemon = True
        watchdog.start()
        try:
            self.request_id += 1
            self.process.stdin.write(
                json.dumps(
                    {
                        "jsonrpc": "2.0",
                        "id": self.request_id,
                        "method": "run",
                        "params": {"source": source},
                    }
                )
                + "\n"
            )
            self.process.stdin.flush()
            messages = []
            while True:
                line = self.process.stdout.readline()
                self.assertTrue(line, f"Kernel exited or request exceeded {timeout}s")
                message = json.loads(line)
                messages.append(message)
                if "id" in message:
                    return messages
        finally:
            watchdog.cancel()

    def fresh_run_and_check_recovery(self, source, timeout=20):
        scripts = ["import os\nos.getpid()", source, "import os\nos.getpid()"]
        requests = [
            json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": index + 1,
                    "method": "run",
                    "params": {"source": script},
                }
            )
            for index, script in enumerate(scripts)
        ]
        result = subprocess.run(
            self.command,
            input="\n".join(requests) + "\n",
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=timeout,
        )
        self.assertEqual(result.returncode, 0, result.stderr[-2000:])
        messages = [json.loads(line) for line in result.stdout.splitlines()]
        responses = [message for message in messages if "id" in message]
        self.assertEqual(len(responses), 3, responses)
        self.assertTrue(
            all(message.get("result", {}).get("ok") for message in responses), responses
        )
        pids = [
            message["params"]["node"]["v"]
            for message in messages
            if message.get("method") == "dump" and message["params"]["runId"] in (1, 3)
        ]
        self.assertEqual(len(pids), 2)
        self.assertEqual(pids[0], pids[1], "Stress run must preserve the warm kernel")
        return messages

    def test_acceptance_print_dump_and_final_expression(self):
        messages = self.run_script(
            'print("Hello, λ")\ndump({"name": "Ada", "scores": [1, 2]})\n6 * 7'
        )
        self.assertEqual(
            "Hello, λ\n",
            "".join(
                m["params"]["text"] for m in messages if m.get("method") == "stream"
            ),
        )
        dumps = [m["params"]["node"] for m in messages if m.get("method") == "dump"]
        self.assertEqual(dumps[0]["fields"][1]["value"]["items"][1]["v"], 2)
        self.assertEqual(dumps[1], {"kind": "primitive", "v": 42})
        self.assertEqual(messages[-2]["method"], "run_finished")
        self.assertTrue(messages[-1]["result"]["ok"])

    def test_globals_clean_but_process_warm(self):
        first = self.run_script("import os\nx = 99\nos.getpid()")
        second = self.run_script('import os\nassert "x" not in globals()\nos.getpid()')
        self.assertEqual(first[-3]["params"]["node"], second[-3]["params"]["node"])

    def test_errors_stderr_and_system_exit_do_not_kill_kernel(self):
        for source in [
            "1 / 0",
            "def broken(",
            "raise SystemExit(4)",
            'input("Name: ")',
        ]:
            messages = self.run_script(source)
            self.assertFalse(messages[-1]["result"]["ok"])
            self.assertTrue(
                any(
                    m.get("method") == "stream" and m["params"]["stream"] == "stderr"
                    for m in messages
                )
            )
        self.assertTrue(self.run_script('print("still alive")')[-1]["result"]["ok"])

    def test_none_and_explicit_dump_are_not_duplicated(self):
        self.assertEqual(
            sum(m.get("method") == "dump" for m in self.run_script("dump(42)")), 1
        )
        self.assertEqual(
            sum(
                m.get("method") == "dump" for m in self.run_script('print("only text")')
            ),
            0,
        )

    def test_objects_cycles_and_bounded_collections(self):
        messages = self.run_script(
            'class Person:\n    pass\np = Person()\np.name = "Ada"\np.self = p\ndump(p)\nlist(range(1000))'
        )
        nodes = [m["params"]["node"] for m in messages if m.get("method") == "dump"]
        self.assertEqual(nodes[0]["type"], "Person")
        self.assertEqual(nodes[0]["fields"][1]["value"]["v"], "<circular reference>")
        self.assertEqual(len(nodes[1]["items"]), 100)
        self.assertTrue(nodes[1]["truncated"])

    def test_native_and_subprocess_output_does_not_corrupt_protocol(self):
        messages = self.run_script(
            'import os, subprocess, sys\nos.write(1, b"native stdout\\n")\nos.write(2, b"native stderr\\n")\nsubprocess.run([sys.executable, "-c", "print(123)"], check=True)'
        )
        output = "".join(
            m["params"]["text"] for m in messages if m.get("method") == "stream"
        )
        self.assertIn("native stdout", output)
        self.assertIn("native stderr", output)
        self.assertIn("123", output)
        self.assertTrue(messages[-1]["result"]["ok"])

    def test_dump_alias_and_other_dump_methods(self):
        messages = self.run_script("from codingpad import dump as show\nshow(42)")
        self.assertEqual(sum(m.get("method") == "dump" for m in messages), 1)
        messages = self.run_script(
            "class Thing:\n    def dump(self): return 7\nThing().dump()"
        )
        self.assertEqual(messages[-3]["params"]["node"]["v"], 7)

    def test_script_module_supports_dataclasses_with_future_annotations(self):
        messages = self.run_script(
            'from __future__ import annotations\nfrom dataclasses import dataclass\n@dataclass\nclass Person:\n    name: str\nPerson("Ada")'
        )
        self.assertTrue(messages[-1]["result"]["ok"])
        self.assertEqual(
            messages[-3]["params"]["node"]["fields"][0]["value"]["v"], "Ada"
        )

    def test_final_calls_keep_script_frame_and_evaluate_once(self):
        for source in [
            'x = 42\neval("x")',
            'x = 42\nlocals()["x"]',
            'x = 42\nvars()["x"]',
        ]:
            messages = self.run_script(source)
            self.assertTrue(messages[-1]["result"]["ok"])
            self.assertEqual(messages[-3]["params"]["node"]["v"], 42)
        messages = self.run_script(
            "calls = []\ndef f():\n    calls.append(1)\n    return len(calls)\nf()"
        )
        self.assertEqual(messages[-3]["params"]["node"]["v"], 1)

    def test_dump_of_globals_is_bounded(self):
        messages = self.run_script(
            "x = list(range(100))\nshared = [x] * 100\nlarge = [shared] * 100\nglobals()"
        )
        self.assertTrue(messages[-1]["result"]["ok"])
        self.assertLess(len(json.dumps(messages)), 200000)

    def test_json_safe_numbers_and_text(self):
        messages = self.run_script('[float("nan"), float("inf"), 10**100, "\\ud800"]')
        self.assertTrue(messages[-1]["result"]["ok"])

    def test_child_stdin_is_eof_not_the_protocol_pipe(self):
        messages = self.run_script(
            'import subprocess, sys\nresult = subprocess.run([sys.executable, "-c", "import sys; print(repr(sys.stdin.read()))"], timeout=2, check=True)'
        )
        self.assertTrue(messages[-1]["result"]["ok"])
        self.assertTrue(self.run_script("42")[-1]["result"]["ok"])

    def test_python_native_and_dump_order_is_preserved(self):
        messages = self.run_script(
            'import subprocess, sys\nprint("a")\nsubprocess.run([sys.executable, "-c", "print(chr(98))"], check=True)\nprint("c")\ndump(42)\nprint("d", flush=True)'
        )
        index = next(i for i, m in enumerate(messages) if m.get("method") == "dump")

        def text(part):
            return "".join(
                m["params"]["text"] for m in part if m.get("method") == "stream"
            )

        self.assertEqual(text(messages[:index]), "a\nb\nc\n")
        self.assertEqual(text(messages[index:]), "d\n")

    def test_python_stdout_stderr_keep_write_order(self):
        messages = self.run_script(
            'import sys\nfor i in range(30):\n    print("out", i)\n    print("err", i, file=sys.stderr)'
        )
        blocks = []
        for message in messages:
            if message.get("method") == "stream":
                stream, text = message["params"]["stream"], message["params"]["text"]
                if blocks and blocks[-1][0] == stream:
                    blocks[-1][1] += text
                else:
                    blocks.append([stream, text])
        actual = [
            (stream, line) for stream, text in blocks for line in text.splitlines()
        ]
        expected = [
            entry
            for i in range(30)
            for entry in [("stdout", f"out {i}"), ("stderr", f"err {i}")]
        ]
        self.assertEqual(actual, expected)

    def test_traceback_follows_all_preceding_stdout(self):
        messages = self.run_script(
            'for i in range(30): print("line", i)\nraise ValueError("boom")'
        )
        first_error = next(
            i
            for i, message in enumerate(messages)
            if message.get("method") == "stream"
            and message["params"]["stream"] == "stderr"
        )
        preceding = "".join(
            message["params"]["text"]
            for message in messages[:first_error]
            if message.get("method") == "stream"
        )
        self.assertEqual(preceding, "".join(f"line {i}\n" for i in range(30)))
        self.assertFalse(messages[-1]["result"]["ok"])

    def test_expired_fence_cannot_acknowledge_a_new_flush(self):
        # Hold the reader before fence 1 until it times out. Then hold the text
        # between fences 1 and 2: a second flush must not accept fence 1's ack.
        messages = self.run_script(r"""import os, sys, threading
kernel = sys.stdout.drain.__globals__
notify, write = kernel["notify"], kernel["write_all"]
first_blocked, middle_blocked = threading.Event(), threading.Event()
release_first, release_middle = threading.Event(), threading.Event()
second_written, finished = threading.Event(), threading.Event()
fence_count = [0]

def delayed_notify(method, params):
    if method == "stream" and params["stream"] == "stdout":
        if "first" in params["text"]:
            first_blocked.set()
            release_first.wait(3)
        if "middle" in params["text"]:
            middle_blocked.set()
            release_middle.wait(3)
    notify(method, params)

def observed_write(descriptor, data):
    write(descriptor, data)
    if descriptor == 1 and data.startswith(b"\0codingpad:"):
        fence_count[0] += 1
        if fence_count[0] == 2:
            second_written.set()

def second_flush():
    sys.stdout.flush()
    finished.set()

worker = None
kernel["notify"], kernel["write_all"] = delayed_notify, observed_write
try:
    os.write(1, b"first")
    assert first_blocked.wait(2), "reader did not reach first text"
    sys.stdout.flush()
    os.write(1, b"middle")
    worker = threading.Thread(target=second_flush)
    worker.start()
    assert second_written.wait(2), "second fence was not written"
    release_first.set()
    assert middle_blocked.wait(2), "reader did not reach intervening text"
    acknowledged_early = finished.wait(0.1)
finally:
    release_first.set()
    release_middle.set()
    if worker is not None:
        worker.join(3)
    kernel["notify"], kernel["write_all"] = notify, write
assert not acknowledged_early, "old fence acknowledged the second flush"
assert finished.is_set(), "second flush did not complete after its own fence"
""")
        self.assertTrue(messages[-1]["result"]["ok"], messages)

    def test_fence_prefix_and_sequence_can_span_pipe_reads(self):
        messages = self.run_script(r"""import sys, time
kernel = sys.stdout.drain.__globals__
write = kernel["write_all"]

def fragmented_write(descriptor, data):
    if data.startswith(b"\0codingpad:"):
        for chunk in (data[:3], data[3:-5], data[-5:-1], data[-1:]):
            write(descriptor, chunk)
            time.sleep(0.002)
    else:
        write(descriptor, data)

kernel["write_all"] = fragmented_write
try:
    print("before λ", end="")
    sys.stdout.flush()
    dump(42)
    print("after λ", end="")
    sys.stdout.flush()
finally:
    kernel["write_all"] = write
""")
        self.assertTrue(messages[-1]["result"]["ok"], messages)
        ordered = [
            (m["method"], m["params"])
            for m in messages
            if m.get("method") in ("stream", "dump")
        ]
        self.assertEqual([kind for kind, _ in ordered], ["stream", "dump", "stream"])
        self.assertEqual(ordered[0][1]["text"], "before λ")
        self.assertEqual(ordered[1][1]["node"]["v"], 42)
        self.assertEqual(ordered[2][1]["text"], "after λ")

    def test_large_int_and_values_without_instance_dict(self):
        messages = self.run_script(
            'import math\ndump(10**5000)\ndump(math.factorial(2000))\ndump({1, 2, 3})\ndump(frozenset({4, 5}))\ndump(b"abc")\ndump(1+2j)'
        )
        self.assertTrue(messages[-1]["result"]["ok"])
        nodes = [m["params"]["node"] for m in messages if m.get("method") == "dump"]
        self.assertEqual(nodes[0]["type"], "int")
        self.assertTrue(nodes[0]["v"])
        self.assertTrue(nodes[1]["v"])
        self.assertEqual({v["v"] for v in nodes[2]["items"]}, {1, 2, 3})
        self.assertEqual({v["v"] for v in nodes[3]["items"]}, {4, 5})
        self.assertEqual(nodes[4]["v"], "b'abc'")
        self.assertEqual(nodes[5]["v"], "(1+2j)")

    def test_gc_finalizer_printing_does_not_deadlock_readers(self):
        self.fresh_run_and_check_recovery(
            """import gc, sys
old_threshold = gc.get_threshold()
gc.set_threshold(1)
class Noisy:
    def __init__(self): self.me = self
    def __del__(self): print('finalized')
for i in range(2000):
    Noisy()
    print('err', i, file=sys.stderr)
    print('out', i)
gc.collect()
gc.set_threshold(*old_threshold)
""",
            timeout=10,
        )

    def test_finalizers_on_reader_thread_never_write_to_its_own_pipe(self):
        messages = self.run_script(
            r"""import gc, sys, weakref
kernel = sys.stdout.drain.__globals__
notify = kernel["notify"]
collected = [False]
was_enabled = gc.isenabled()
gc.disable()
class Noisy:
    def __init__(self): self.me = self
    def __del__(self): sys.stdout.write("F" * 2048)
for i in range(32):
    obj = Noisy()
    weakref.finalize(obj, sys.stdout.write, "W" * 2048)
del obj

def collect_on_reader(method, params):
    if method == "stream" and not collected[0]:
        collected[0] = True
        gc.collect()
    notify(method, params)

kernel["notify"] = collect_on_reader
try:
    print("trigger", flush=True)
finally:
    kernel["notify"] = notify
    if was_enabled:
        gc.enable()
""",
            timeout=10,
        )
        self.assertTrue(messages[-1]["result"]["ok"], messages[-1])
        text = "".join(
            message["params"]["text"]
            for message in messages
            if message.get("method") == "stream"
        )
        self.assertEqual(text.count("F"), 32 * 2048)
        self.assertEqual(text.count("W"), 32 * 2048)

    @unittest.skipUnless(hasattr(signal, "setitimer"), "interval signals require Unix")
    def test_signal_handler_printing_stress_and_warm_recovery(self):
        source = """import signal, sys
signal.signal(signal.SIGALRM, lambda *a: print('tick', file=sys.stderr))
signal.setitimer(signal.ITIMER_REAL, 0.0005, 0.0005)
for i in range(200000): print(i)
"""
        # Fresh processes exercise startup, run cleanup and interpreter exit as
        # well as the write path. The follow-up request checks warm recovery.
        for _ in range(12):
            messages = self.fresh_run_and_check_recovery(source, timeout=20)
            stdout = "".join(
                message["params"]["text"]
                for message in messages
                if message.get("method") == "stream"
                and message["params"]["runId"] == 2
                and message["params"]["stream"] == "stdout"
            )
            self.assertTrue(stdout.endswith("199999\n"))

    def test_cleanup_exceptions_fail_only_the_request(self):
        for kind in (
            "KeyboardInterrupt",
            "RecursionError",
            "OSError",
            "ValueError",
            "TypeError",
        ):
            messages = self.run_script(f"""import os, sys
kernel = sys.stdout.drain.__globals__
notify = kernel["notify"]
def fail_finished(method, params):
    if method == "run_finished":
        kernel["notify"] = notify
        raise {kind}("injected cleanup failure")
    notify(method, params)
kernel["notify"] = fail_finished
os.getpid()
""")
            self.assertIn("error", messages[-1], messages)
            self.assertEqual(messages[-1]["error"]["code"], -32603)
            self.assertIn(kind, messages[-1]["error"]["message"])
            before = next(
                message["params"]["node"]["v"]
                for message in messages
                if message.get("method") == "dump"
            )
            after = self.run_script("import os\nos.getpid()")
            self.assertTrue(after[-1]["result"]["ok"])
            self.assertEqual(before, after[-3]["params"]["node"]["v"])

    def test_malformed_request_can_recover(self):
        self.process.stdin.write("not json\n")
        self.process.stdin.flush()
        self.assertEqual(
            json.loads(self.process.stdout.readline())["error"]["code"], -32600
        )
        self.assertTrue(self.run_script("42")[-1]["result"]["ok"])


if __name__ == "__main__":
    unittest.main()
