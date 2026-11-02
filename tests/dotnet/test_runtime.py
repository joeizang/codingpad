"""C# package and protocol acceptance checks; requires the .NET 10 SDK."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[2]


class RuntimeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix="codingpad-dotnet-")
        cls.addClassCleanup(cls.temp.cleanup)
        cls.work = Path(cls.temp.name)
        cls.feed = cls.work / "feed"
        cls.kernel = cls.work / "kernel"
        for command in [
            ["dotnet", "pack", str(ROOT / "runtimes/dotnet/CodingPad"), "-c", "Release", "-o", str(cls.feed)],
            ["dotnet", "publish", str(ROOT / "kernels/csharp"), "-c", "Release", "-o", str(cls.kernel), "-p:RestoreLockedMode=true"],
        ]:
            subprocess.run(command, check=True, capture_output=True, text=True, timeout=120)

    def test_file_script_imports_helpers_from_package(self):
        (self.work / "NuGet.Config").write_text(
            '<configuration><packageSources><clear />'
            f'<add key="local" value="{escape(str(self.feed))}" />'
            '</packageSources></configuration>', encoding="utf-8"
        )
        script = self.work / "script.cs"
        script.write_text('''#:package CodingPad@0.1.0
var result = Cache("value", () => 42);
if (Cache("value", () => -1) != 42) throw new Exception("Cache missed");
result.Dump();
await Task.Delay(1, Cancel);
try { Secret("test"); throw new Exception("Secret should fail"); }
catch (NotSupportedException) { }
Console.WriteLine(result);
''', encoding="utf-8")
        # File apps default to AOT; execution needs no AOT compiler download.
        result = subprocess.run(
            ["dotnet", "run", "--file", str(script), "-p:PublishAot=false"],
            cwd=self.work, env={**os.environ, "NUGET_PACKAGES": str(self.work / "packages")},
            capture_output=True, text=True, timeout=120,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.strip(), "42")

    def test_early_and_expired_cancellation_ids(self):
        messages = [
            {"id": 1, "method": "run", "params": {"source": "await Task.Delay(60000, Cancel);"}},
            {"method": "cancel", "params": {"runId": 1}},
            {"id": 2, "method": "run", "params": {"source": "Cancel.IsCancellationRequested.Dump();"}},
            {"method": "cancel", "params": {"runId": 1}},
            {"method": "cancel", "params": {"runId": 999}},
        ]
        result = subprocess.run(
            ["dotnet", str(self.kernel / "CodingPad.Kernel.dll")],
            input="".join(json.dumps({"jsonrpc": "2.0", **message}) + "\n" for message in messages),
            capture_output=True, text=True, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        events = [json.loads(line) for line in result.stdout.splitlines()]
        replies = {event["id"]: event for event in events if "id" in event}
        self.assertFalse(replies[1]["result"]["ok"])
        self.assertTrue(replies[2]["result"]["ok"])
        dumps = [event["params"]["node"] for event in events
                 if event.get("method") == "dump" and event["params"]["runId"] == 2]
        self.assertEqual(len(dumps), 1)
        self.assertIs(dumps[0]["v"], False)


if __name__ == "__main__":
    unittest.main()
