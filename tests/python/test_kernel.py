import json
import shutil
import subprocess
import sys
import tempfile
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
        self.process = subprocess.Popen(
            [sys.executable, "-I", "-u", "-X", "utf8", str(folder / "kernel.py")],
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

    def run_script(self, source):
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
            self.assertTrue(line, "Kernel exited before responding")
            message = json.loads(line)
            messages.append(message)
            if "id" in message:
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

    def test_malformed_request_can_recover(self):
        self.process.stdin.write("not json\n")
        self.process.stdin.flush()
        self.assertEqual(
            json.loads(self.process.stdout.readline())["error"]["code"], -32600
        )
        self.assertTrue(self.run_script("42")[-1]["result"]["ok"])


if __name__ == "__main__":
    unittest.main()
