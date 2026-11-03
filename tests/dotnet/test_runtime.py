"""C# package and protocol acceptance checks; requires the .NET 10 SDK."""

import json
import os
import queue
import threading
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

    def exchange(self, lines, reply_count):
        process = subprocess.Popen(
            ["dotnet", str(self.kernel / "CodingPad.Kernel.dll")],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, bufsize=1,
        )
        incoming = queue.Queue()
        def read():
            for line in process.stdout:
                incoming.put(line)
            incoming.put(None)
        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        try:
            process.stdin.write("\n".join(lines) + "\n")
            process.stdin.flush()
            events = []
            while sum("id" in event for event in events) < reply_count:
                line = incoming.get(timeout=30)
                self.assertIsNotNone(line, "Kernel exited before replying")
                events.append(json.loads(line))
            # Keep stdin open until replies arrive: EOF now cancels queued work.
            process.stdin.close()
            self.assertEqual(process.wait(timeout=5), 0, process.stderr.read())
            return events
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            reader.join(timeout=5)
            process.stdin.close()
            process.stdout.close()
            process.stderr.close()

    def test_file_script_imports_helpers_from_package(self):
        script = self.work / "script.cs"
        script.write_text('''#:package CodingPad@0.1.0
var result = Cache("value", () => 42);
if (Cache("value", () => -1) != 42) throw new Exception("Cache missed");
result.Dump();
await Task.Delay(1, Cancel);
if (await Cache("async", async () => { await Task.Delay(1); return 7; }) != 7)
    throw new Exception("Async cache missed");
if (await Cache("async", () => Task.FromResult(-1)) != 7)
    throw new Exception("Async cache recomputed");
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


    def test_direct_consumers_can_disable_helper_imports(self):
        project = self.work / "opt-out"
        project.mkdir()
        (project / "OptOut.csproj").write_text('''<Project Sdk="Microsoft.NET.Sdk">
<PropertyGroup>
<TargetFramework>net10.0</TargetFramework><OutputType>Exe</OutputType>
<ImplicitUsings>disable</ImplicitUsings><CodingPadImplicitUsings>false</CodingPadImplicitUsings>
</PropertyGroup>
<ItemGroup><PackageReference Include="CodingPad" Version="0.1.0" /></ItemGroup>
</Project>''', encoding="utf-8")
        (project / "Program.cs").write_text('''using System;
using System.Timers;
using static LocalHelpers;
using var timer = new Timer();
if (Cache("own", () => 7) != 7) throw new Exception("Wrong helper");
if (CodingPad.ScriptHelpers.Cache("explicit", () => 42) != 42) throw new Exception("Package unavailable");
Console.WriteLine("opted out");
static class LocalHelpers { public static T Cache<T>(string key, Func<T> factory) => factory(); }
''', encoding="utf-8")
        result = subprocess.run(
            ["dotnet", "run", "--project", str(project / "OptOut.csproj")],
            cwd=self.work, env=self.env, capture_output=True, text=True, timeout=120,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.strip(), "opted out")

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
        events = self.exchange(requests, 6)
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
        events = self.exchange([json.dumps({"jsonrpc": "2.0", **message}) for message in messages], 2)
        replies = {event["id"]: event for event in events if "id" in event}
        self.assertFalse(replies[1]["result"]["ok"])
        self.assertTrue(replies[2]["result"]["ok"])
        dumps = [event["params"]["node"] for event in events
                 if event.get("method") == "dump" and event["params"]["runId"] == 2]
        self.assertEqual(len(dumps), 1)
        self.assertIs(dumps[0]["v"], False)

    def test_recursive_type_proofs_do_not_hide_unsafe_mutual_dependencies(self):
        project = self.work / "recursive-proof"
        project.mkdir()
        # A normal, non-collectible consumer supplies recursive storage types;
        # its unsigned friend name permits exercising host scopes in this fixture.
        (project / "Proof.csproj").write_text('''<Project Sdk="Microsoft.NET.Sdk">
<PropertyGroup><TargetFramework>net10.0</TargetFramework><OutputType>Exe</OutputType>
<AssemblyName>CodingPad.Kernel</AssemblyName></PropertyGroup>
<ItemGroup><PackageReference Include="CodingPad" Version="0.1.0" /></ItemGroup>
</Project>''', encoding="utf-8")
        (project / "Program.cs").write_text('''using System.Reflection;
using System.Reflection.Emit;
var assembly = AssemblyBuilder.DefineDynamicAssembly(new AssemblyName("CollectibleValue"), AssemblyBuilderAccess.RunAndCollect);
var type = assembly.DefineDynamicModule("m").DefineType("ScriptValue", TypeAttributes.Public).CreateType()!;
CodingPad.ScriptHelpers.InitializeHost();
using (CodingPad.ScriptHelpers.BeginRun(default, _ => {})) {
    Cache("seed", () => new A { Next = new B() });
    Cache("graph", () => new B { Next = new A { Payload = Activator.CreateInstance(type) } });
}
using (CodingPad.ScriptHelpers.BeginRun(default, _ => {})) {
    Cache<A>("seed", () => throw new Exception("Safe graph was refreshed"));
    int calls = 0;
    Cache("graph", () => { calls++; return new B(); });
    if (calls != 1) throw new Exception("Recursive proof hid a collectible value");
}
Console.WriteLine("recursive graph checked");
sealed class A { public B Next; public object Payload; }
sealed class B { public A Next; }
''', encoding="utf-8")
        result = subprocess.run(
            ["dotnet", "run", "--project", str(project / "Proof.csproj")],
            cwd=self.work, env=self.env, capture_output=True, text=True, timeout=120,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.strip(), "recursive graph checked")

    def test_cancel_request_with_id_receives_a_response(self):
        events = self.exchange([
            '{"jsonrpc":"2.0","id":7,"method":"cancel","params":{"runId":99}}',
            '{"jsonrpc":"2.0","id":8,"method":"run","params":{"source":"42.Dump();"}}',
        ], 2)
        replies = {event["id"]: event for event in events if "id" in event}
        self.assertEqual(replies[7]["error"]["code"], -32601)
        self.assertTrue(replies[8]["result"]["ok"])

    def test_stdin_eof_cancels_a_cooperative_run(self):
        process = subprocess.Popen(
            ["dotnet", str(self.kernel / "CodingPad.Kernel.dll")],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, bufsize=1,
        )
        incoming = queue.Queue()
        def read():
            for line in process.stdout:
                incoming.put(json.loads(line))
        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        try:
            source = 'Console.WriteLine("ready"); await Task.Delay(60000, Cancel);'
            process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "run", "params": {"source": source}}) + "\n")
            process.stdin.flush()
            while True:
                event = incoming.get(timeout=30)
                if event.get("method") == "stream" and "ready" in event["params"]["text"]:
                    break
            process.stdin.close()
            self.assertEqual(process.wait(timeout=5), 0, process.stderr.read())
            reader.join(timeout=5)
            replies = [event for event in list(incoming.queue) if "id" in event]
            self.assertEqual(len(replies), 1)
            self.assertFalse(replies[0]["result"]["ok"])
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            reader.join(timeout=5)
            process.stdin.close()
            process.stdout.close()
            process.stderr.close()

    def test_dispatcher_failure_exits_even_with_stdin_open(self):
        process = subprocess.Popen(
            ["dotnet", str(self.kernel / "CodingPad.Kernel.dll")],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True,
        )
        try:
            # Force an unexpected Send failure without relying on platform-
            # specific broken-pipe behavior. Scripts have ordinary user privileges.
            source = '''var host = AppDomain.CurrentDomain.GetAssemblies().Single(a => a.GetName().Name == "CodingPad.Kernel").GetType("Program")!;
((System.IO.TextWriter)host.GetField("Protocol", System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.NonPublic)!.GetValue(null)!).Dispose();'''
            process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "run", "params": {"source": source}}) + "\n")
            process.stdin.flush()
            self.assertNotEqual(process.wait(timeout=10), 0)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            process.stdin.close()
            process.stdout.close()
            process.stderr.close()


if __name__ == "__main__":
    unittest.main()
