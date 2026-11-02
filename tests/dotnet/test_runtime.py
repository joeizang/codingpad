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

        (cls.work / "NuGet.Config").write_text(
            '<configuration><packageSources><clear />'
            f'<add key="local" value="{escape(str(cls.feed))}" />'
            '</packageSources></configuration>', encoding="utf-8"
        )
        cls.env = {**os.environ, "NUGET_PACKAGES": str(cls.work / "packages")}

    def test_file_script_imports_helpers_from_package(self):
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
            cwd=self.work, env=self.env,
            capture_output=True, text=True, timeout=120,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.strip(), "42")


    def test_host_api_is_not_public(self):
        script = self.work / "host-api.cs"
        script.write_text('''#:package CodingPad@0.1.0
CodingPad.ScriptHelpers.BeginRun(default, _ => {});
typeof(CodingPad.ScriptHelpers.RunScope).Dump();
''', encoding="utf-8")
        result = subprocess.run(
            ["dotnet", "run", "--file", str(script), "-p:PublishAot=false"],
            cwd=self.work, env=self.env, capture_output=True, text=True, timeout=120,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("BeginRun", result.stdout + result.stderr)
        self.assertIn("RunScope", result.stdout + result.stderr)
        self.assertIn("CS0122", result.stdout + result.stderr)

    def test_imports_do_not_leak_through_an_indirect_package(self):
        bridge = self.work / "bridge"
        bridge.mkdir()
        (bridge / "bridge.csproj").write_text('''<Project Sdk="Microsoft.NET.Sdk">
<PropertyGroup><TargetFramework>net10.0</TargetFramework><PackageId>CodingPad.Bridge</PackageId></PropertyGroup>
<ItemGroup><PackageReference Include="CodingPad" Version="0.1.0" /></ItemGroup>
</Project>''', encoding="utf-8")
        (bridge / "Bridge.cs").write_text(
            'public static class Bridge { public static int Read() => Cache("bridge", () => 42); }',
            encoding="utf-8",
        )
        packed = subprocess.run(
            ["dotnet", "pack", str(bridge / "bridge.csproj"), "-o", str(self.feed)],
            cwd=self.work, env=self.env, capture_output=True, text=True, timeout=120,
        )
        self.assertEqual(packed.returncode, 0, packed.stdout + packed.stderr)
        script = self.work / "indirect.cs"
        script.write_text('''#:package CodingPad.Bridge@1.0.0
using System;
using static LocalHelpers;
if (Bridge.Read() != 42 || Cache("consumer", () => 7) != 7) throw new Exception("Wrong helper");
Console.WriteLine("isolated");
static class LocalHelpers { public static T Cache<T>(string key, Func<T> factory) => factory(); }
''', encoding="utf-8")
        result = subprocess.run(
            ["dotnet", "run", "--file", str(script), "-p:PublishAot=false"],
            cwd=self.work, env=self.env, capture_output=True, text=True, timeout=120,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.strip(), "isolated")

    def test_invalid_requests_recover_after_single_parse_dispatch(self):
        requests = ["{", "[]", '{"jsonrpc":"2.0","id":"bad","method":"run"}',
                    '{"jsonrpc":"1.0","id":3,"method":"run"}',
                    '{"jsonrpc":"2.0","id":4,"method":"missing"}',
                    '{"jsonrpc":"2.0","id":5,"method":"run","params":{"source":"42.Dump();"}}']
        result = subprocess.run(
            ["dotnet", str(self.kernel / "CodingPad.Kernel.dll")],
            input="\n".join(requests) + "\n", capture_output=True, text=True, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        events = [json.loads(line) for line in result.stdout.splitlines()]
        replies = [event for event in events if "id" in event]
        self.assertEqual(len(replies), 6)
        self.assertTrue(all(reply["error"]["code"] == -32600 for reply in replies[:4]))
        self.assertEqual(replies[4]["error"]["code"], -32601)
        self.assertTrue(replies[5]["result"]["ok"])

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
