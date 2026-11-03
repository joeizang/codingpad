"""Offline feeds, authenticated restore and dotnet-run parity for M2.3."""
import base64
import functools
import http.server
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import tempfile
import threading
import time
import unittest
import zipfile
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parents[2]


class DirectiveTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix="codingpad-directives-")
        cls.addClassCleanup(cls.temp.cleanup)
        # macOS temp folders sit behind a symlink (/var -> /private/var). `dotnet pack`
        # drops ProjectReference dependencies when given a path through one.
        cls.work = Path(cls.temp.name).resolve()
        cls.feed = cls.work / "feed"
        cls.feed.mkdir()
        cls.kernel = cls.work / "kernel"
        cls.env = {**os.environ, "NUGET_PACKAGES": str(cls.work / "packages")}
        cls.command(["dotnet", "publish", str(ROOT / "kernels/csharp"), "-c", "Release", "-o", str(cls.kernel), "-p:RestoreLockedMode=true"])
        # Real transitive dependencies, built locally and restored without nuget.org.
        for name, code, reference in [
            ("Leaf", 'namespace Fixture; public class Leaf { public string Value = "restored"; }', ""),
            ("Root", 'namespace Fixture; public class Root { public Leaf Child = new(); }',
             '<ItemGroup><ProjectReference Include="../Leaf/Leaf.csproj" /></ItemGroup>'),
        ]:
            project = cls.work / name
            project.mkdir()
            (project / f"{name}.csproj").write_text(
                '<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework>'
                f'<PackageId>CodingPad.Test.{name}</PackageId><Version>1.0.0</Version>'
                '</PropertyGroup>' + reference + '</Project>', encoding="utf-8")
            (project / "Value.cs").write_text(code, encoding="utf-8")
            cls.command(["dotnet", "pack", str(project), "-o", str(cls.feed)])
        cls.config = cls.work / "NuGet.Config"
        cls.local_config()

    @classmethod
    def command(cls, args, **kwargs):
        result = subprocess.run(args, capture_output=True, text=True, timeout=120, **kwargs)
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)
        return result

    @classmethod
    def local_config(cls):
        cls.config.write_text('<configuration><packageSources><clear />'
                              f'<add key="local" value="{escape(str(cls.feed))}" />'
                              '</packageSources></configuration>', encoding="utf-8")

    def run_kernel(self, sources, *, env=None, path=None, before_reply=None, after_reply=None):
        process = subprocess.Popen(["dotnet", str(self.kernel / "CodingPad.Kernel.dll")],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, env=env or self.env, cwd=ROOT)
        incoming = queue.Queue()
        def read():
            for line in process.stdout:
                incoming.put(json.loads(line))
            incoming.put(None)
        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        events = []
        try:
            for run_id, source in enumerate(sources, 1):
                params = {"source": source, "path": str(path or self.work / "scripts" / "script.cs")}
                process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": run_id, "method": "run", "params": params}) + "\n")
                process.stdin.flush()
                if before_reply:
                    before_reply(process, run_id)
                while True:
                    event = incoming.get(timeout=40)
                    self.assertIsNotNone(event, "Kernel exited before replying")
                    events.append(event)
                    if event.get("id") == run_id:
                        if after_reply:
                            after_reply(run_id)
                        break
            return events
        finally:
            process.stdin.close()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            reader.join(timeout=5)
            process.stdout.close()
            process.stderr.close()

    def assert_ok(self, events, count=1):
        replies = [event["result"]["ok"] for event in events if "result" in event]
        self.assertEqual(replies, [True] * count, events)

    @staticmethod
    def stdout(events):
        return "".join(e["params"]["text"] for e in events if e.get("method") == "stream" and e["params"]["stream"] == "stdout")

    def test_transitive_local_feed_parent_config_cache_and_package_identity(self):
        # Parent config must be found from the saved script, not the host cwd.
        (self.work / "scripts").mkdir(exist_ok=True)
        source = '''#:package CodingPad.Test.Root@1.0.0
var item = Cache("package", () => new Fixture.Root());
Console.WriteLine(item.Child.Value);
item.Child.Value = "cached";
'''
        events = self.run_kernel([source, source])
        self.assert_ok(events, 2)
        self.assertEqual(self.stdout(events), "restored\ncached\n")
        script = self.work / "scripts" / "parity.cs"
        script.write_text('''#:package CodingPad.Test.Root@1.0.0
Console.WriteLine(new Fixture.Root().Child.Value);
''', encoding="utf-8")
        result = self.command(["dotnet", "run", "--file", str(script), "-p:PublishAot=false"], cwd=self.work, env=self.env)
        self.assertEqual(result.stdout.strip(), "restored")

    def test_directives_only_and_trailing_comments_are_valid(self):
        events = self.run_kernel([
            '#:package CodingPad.Test.Root@1.0.0\n#:package CodingPad.Test.Leaf@1.0.0\n',
            '#:property Nullable=enable\n#:property Optimize=true\n// Write code next.\n',
            '// Leading comment\n#:property Nullable=enable\n#:property Optimize=true\n/* Done */\n',
        ])
        self.assert_ok(events, 3)
        self.assertFalse(any(e.get("method") == "dump" for e in events), events)
        invalid = self.run_kernel(['Console.WriteLine("must not run");\n#:property Optimize=true\n'])
        self.assertFalse(invalid[-1]["result"]["ok"])
        self.assertIn("File directives must appear before C# code", json.dumps(invalid))

    def test_dependency_context_count_stays_constant_on_hits_and_failures(self):
        count = 'Console.WriteLine(System.Runtime.Loader.AssemblyLoadContext.All.Count(c => c.Name == "CodingPad dependencies"));'
        package = '#:package CodingPad.Test.Root@1.0.0\n' + count
        missing = '#:package Missing.CodingPad.Leak@1.0.0\n'
        events = self.run_kernel([count] * 5 + [missing] * 2 + [count] + [package] * 5 + [count])
        self.assertEqual(self.stdout(events).splitlines(), ["1"] * 6 + ["2"] * 6)
        self.assertEqual([e["result"]["ok"] for e in events if "result" in e], [True] * 5 + [False] * 2 + [True] * 7)

    def test_unknown_build_rids_fall_back_to_portable_assets(self):
        # Exercise the internal selector through reflection; scripts still receive
        # only framework/helper compiler references, never private NuGet APIs.
        source = r'''var type = AppDomain.CurrentDomain.GetAssemblies().Single(a => a.GetName().Name == "CodingPad.Kernel").GetType("ScriptDependencies")!;
var select = type.GetMethod("SelectRuntimeIdentifier", System.Reflection.BindingFlags.Static | System.Reflection.BindingFlags.NonPublic)!;
var graphType = select.GetParameters()[0].ParameterType;
var read = graphType.Assembly.GetType("NuGet.RuntimeModel.JsonRuntimeFormat")!.GetMethod("ReadRuntimeGraph", new[] { typeof(string) })!;
var graph = read.Invoke(null, new object[] { Path.Combine(AppContext.BaseDirectory, "PortableRuntimeIdentifierGraph.json") })!;
string Pick(string rid, string os, System.Runtime.InteropServices.Architecture arch, bool musl) =>
    (string)select.Invoke(null, new object[] { graph, rid, os, arch, musl })!;
Console.WriteLine(Pick("ubuntu.999.0-x64", "linux", System.Runtime.InteropServices.Architecture.X64, false));
Console.WriteLine(Pick("alpine.999.0-arm64", "linux", System.Runtime.InteropServices.Architecture.Arm64, true));
Console.WriteLine(Pick("future-windows", "win", System.Runtime.InteropServices.Architecture.X64, false));
Console.WriteLine(Pick("future-macos", "osx", System.Runtime.InteropServices.Architecture.Arm64, false));
Console.WriteLine(Pick("linux-x64", "linux", System.Runtime.InteropServices.Architecture.X64, false));
'''
        events = self.run_kernel([source])
        self.assert_ok(events)
        self.assertEqual(self.stdout(events).splitlines(), ["linux-x64", "linux-musl-arm64", "win-x64", "osx-arm64", "linux-x64"])

    def test_restore_cache_pins_ranges_until_config_changes(self):
        folder = self.work / "range-cache"
        folder.mkdir()
        feed = folder / "feed"
        feed.mkdir()
        for package in self.feed.glob("*.nupkg"):
            (feed / package.name).write_bytes(package.read_bytes())
        config = folder / "NuGet.Config"
        config_text = '<configuration><packageSources><clear />' + f'<add key="range" value="{escape(str(feed))}" />' + '</packageSources></configuration>'
        config.write_text(config_text, encoding="utf-8")
        (folder / "scripts").mkdir()
        source = '#:package CodingPad.Test.Root@1.*\nConsole.WriteLine(typeof(Fixture.Root).Assembly.Location);'
        def update(run_id):
            if run_id in (1, 3):
                # Introduce a newer local version after initial resolution.
                version = "1.1.0" if run_id == 1 else "1.2.0"
                original = next(feed.glob("CodingPad.Test.Root.1.0.0.nupkg"))
                with zipfile.ZipFile(original) as old, zipfile.ZipFile(feed / f"CodingPad.Test.Root.{version}.nupkg", "w") as new:
                    for item in old.infolist():
                        data = old.read(item.filename)
                        if item.filename.endswith(".nuspec"):
                            data = data.replace(b"<version>1.0.0</version>", f"<version>{version}</version>".encode())
                        new.writestr(item, data)
            if run_id == 3:
                (folder / "scripts" / "NuGet.Config").write_text(config_text.replace('key="range"', 'key="child"'), encoding="utf-8")
            if run_id == 2:
                # Same-length edit rules out a size/mtime-only fingerprint.
                config.write_text(config_text.replace('key="range"', 'key="other"'), encoding="utf-8")
        env = {**self.env, "NUGET_PACKAGES": str(folder / "packages")}
        events = self.run_kernel([source] * 4, path=folder / "scripts" / "script.cs", env=env, after_reply=update)
        self.assert_ok(events, 4)
        locations = [line.replace("\\", "/") for line in self.stdout(events).splitlines()]
        self.assertIn("/1.0.0/", locations[0])
        self.assertEqual(locations[0], locations[1])
        self.assertIn("/1.1.0/", locations[2])
        self.assertIn("/1.2.0/", locations[3])
        self.assertEqual([e["params"]["runId"] for e in events if e.get("method") == "stream" and e["params"]["text"] == "Restoring C# packages…\n"], [1, 3, 4])
        self.assertFalse(any(e.get("method") == "stream" and e["params"]["runId"] == 2 and e["params"]["stream"] == "stderr" for e in events), events)

    def test_restore_cache_recovers_when_package_files_are_removed(self):
        folder = self.work / "removed-cache"
        folder.mkdir()
        env = {**self.env, "NUGET_PACKAGES": str(folder / "packages")}
        source = '#:package CodingPad.Test.Root@1.0.0\nConsole.WriteLine("ok");'
        package = folder / "packages" / "codingpad.test.root" / "1.0.0"
        def remove(run_id):
            if run_id == 1:
                shutil.rmtree(package)
        events = self.run_kernel([source] * 3, path=folder / "script.cs", env=env, after_reply=remove)
        self.assert_ok(events, 3)
        self.assertEqual(self.stdout(events), "ok\nok\nok\n")
        self.assertTrue((package / "lib/net10.0/Root.dll").is_file())
        self.assertEqual([e["params"]["runId"] for e in events if e.get("method") == "stream" and e["params"]["text"] == "Restoring C# packages…\n"], [1, 2])
        self.assertFalse(any(e.get("method") == "stream" and e["params"]["runId"] == 3 and e["params"]["stream"] == "stderr" for e in events), events)

    def test_compiler_properties_and_line_numbers(self):
        source = '''#:property AllowUnsafeBlocks=true
#:property CheckForOverflowUnderflow=true
#:property DefineConstants=MY_FLAG;OTHER_FLAG
#:property Nullable=enable
#:property Optimize=true
#:property LangVersion=14
#if MY_FLAG && OTHER_FLAG
unsafe { int value = 42; int* p = &value; Console.WriteLine(*p); }
#endif
try { int n = int.MaxValue; n++; } catch (OverflowException) { Console.WriteLine("checked"); }
'''
        events = self.run_kernel([source])
        self.assert_ok(events)
        self.assertEqual(self.stdout(events), "42\nchecked\n")
        script = self.work / "properties.cs"
        script.write_text(source, encoding="utf-8")
        result = self.command(["dotnet", "run", "--file", str(script), "-p:PublishAot=false"], cwd=self.work, env=self.env)
        self.assertEqual(result.stdout, self.stdout(events))
        failure = self.run_kernel(['#:property TreatWarningsAsErrors=true\nstring value = null;\nConsole.WriteLine(value);'])
        self.assertFalse(failure[-1]["result"]["ok"])
        diagnostics = next(e["params"]["diagnostics"] for e in failure if e.get("method") == "diagnostics")
        self.assertTrue(any(d["line"] == 2 and "CS8600" in d["message"] for d in diagnostics), diagnostics)

    def test_web_sdk_builds_minimal_app_and_dotnet_parity(self):
        source = '''#:sdk Microsoft.NET.Sdk.Web
var builder = WebApplication.CreateBuilder();
await using var app = builder.Build();
app.MapGet("/", () => "hello");
Console.WriteLine(app.GetType().Name);
'''
        events = self.run_kernel([source])
        self.assert_ok(events)
        self.assertEqual(self.stdout(events), "WebApplication\n")
        script = self.work / "web.cs"
        script.write_text(source, encoding="utf-8")
        result = self.command(["dotnet", "run", "--file", str(script), "-p:PublishAot=false"], cwd=self.work, env=self.env)
        self.assertEqual(result.stdout, self.stdout(events))

    def test_directive_errors_block_execution_and_recover(self):
        invalid = [
            '#:project ../Other\nConsole.WriteLine("executed");',
            '#:package invalid\nConsole.WriteLine("executed");',
            '#:property AllowUnsafeBlocks=maybe\nConsole.WriteLine("executed");',
            '#:property Optimize=true\n#:property optimize=false\nConsole.WriteLine("executed");',
            '#:sdk Unknown.Sdk\nConsole.WriteLine("executed");',
            '#:property Unknown=anything\nConsole.WriteLine("executed");',
        ]
        events = self.run_kernel(invalid + ['Console.WriteLine("recovered");'])
        self.assertEqual([e["result"]["ok"] for e in events if "result" in e], [False] * len(invalid) + [True])
        self.assertEqual(self.stdout(events), "recovered\n")
        self.assertIn("not supported in CodingPad yet; runs with `dotnet run`", json.dumps(events))
        # Directive-looking text inside strings is ordinary source.
        self.assert_ok(self.run_kernel(['Console.WriteLine("""\n#:package fake@1.0\n""");']))

    def test_missing_package_fails_without_running_and_warm_recovery(self):
        events = self.run_kernel(['#:package Missing.CodingPad.Package@1.0.0\nConsole.WriteLine("executed");', 'Console.WriteLine("recovered");'])
        self.assertEqual([e["result"]["ok"] for e in events if "result" in e], [False, True])
        self.assertEqual(self.stdout(events), "recovered\n")
        self.assertIn("NU1101", json.dumps(events))
        self.assertFalse(any(e.get("method") == "stream" and "NU1101" in e["params"]["text"] for e in events), events)

    def test_cancel_interrupts_restore_and_preserves_warm_cache(self):
        requested = threading.Event()
        release = threading.Event()
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                requested.set()
                release.wait(timeout=20)
                self.send_response(503)
                self.end_headers()
            def log_message(self, *args):
                pass
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        folder = self.work / "cancel"
        folder.mkdir(exist_ok=True)
        (folder / "NuGet.Config").write_text(
            '<configuration><packageSources><clear />'
            f'<add key="slow" value="http://127.0.0.1:{server.server_port}/index.json" allowInsecureConnections="true" />'
            '</packageSources></configuration>', encoding="utf-8")
        cancelled_at = []
        def cancel(process, run_id):
            if run_id == 2:
                self.assertTrue(requested.wait(timeout=10), "Restore never reached feed")
                cancelled_at.append(time.monotonic())
                process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "cancel", "params": {"runId": run_id}}) + "\n")
                process.stdin.flush()
        try:
            events = self.run_kernel([
                'Cache("warm", () => 7);',
                '#:package Missing.CodingPad.Cancel@1.0.0\nConsole.WriteLine("executed");',
                'Console.WriteLine(Cache("warm", () => 0));',
            ], path=folder / "script.cs", before_reply=cancel)
            self.assertEqual([e["result"]["ok"] for e in events if "result" in e], [True, False, True])
            self.assertEqual(self.stdout(events), "7\n")
            self.assertEqual([e["params"]["runId"] for e in events if e.get("method") == "stream" and e["params"]["text"] == "Restoring C# packages…\n"], [2])
            self.assertLess(time.monotonic() - cancelled_at[0], 5, "Restore did not cancel promptly")
        finally:
            release.set()
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)

    def test_authenticated_feed_and_warm_download_cache(self):
        root = self.work / "http"
        root.mkdir(exist_ok=True)
        # Minimal NuGet v3 flat-container feed, requiring Basic auth for every request.
        for package in self.feed.glob("*.nupkg"):
            with zipfile.ZipFile(package) as archive:
                import xml.etree.ElementTree as ET
                metadata = ET.fromstring(archive.read(next(n for n in archive.namelist() if n.endswith(".nuspec"))))
                ns = {"n": metadata.tag.split("}")[0].lstrip("{")}
                name = metadata.find("n:metadata/n:id", ns).text.lower()
            folder = root / name / "1.0.0"
            folder.mkdir(parents=True, exist_ok=True)
            (folder / f"{name}.1.0.0.nupkg").write_bytes(package.read_bytes())
            (folder.parent / "index.json").write_text('{"versions":["1.0.0"]}', encoding="utf-8")
        requests = []
        expected = "Basic " + base64.b64encode(b"user:token-value").decode()
        class Handler(http.server.SimpleHTTPRequestHandler):
            def do_GET(self):
                requests.append(self.path)
                if self.headers.get("Authorization") != expected:
                    self.send_response(401)
                    self.send_header("WWW-Authenticate", 'Basic realm="packages"')
                    self.end_headers()
                else:
                    super().do_GET()
            def log_message(self, *args):
                pass
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=str(root)))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        url = f"http://127.0.0.1:{server.server_port}/"
        (root / "index.json").write_text(json.dumps({"version": "3.0.0", "resources": [{"@id": url, "@type": "PackageBaseAddress/3.0.0"}]}), encoding="utf-8")
        config_dir = self.work / "authenticated"
        config_dir.mkdir(exist_ok=True)
        (config_dir / "NuGet.Config").write_text(
            '<configuration><packageSources><clear />'
            f'<add key="private" value="{url}index.json" allowInsecureConnections="true" />'
            '</packageSources><packageSourceCredentials><private>'
            '<add key="Username" value="user" /><add key="ClearTextPassword" value="token-value" />'
            '</private></packageSourceCredentials></configuration>', encoding="utf-8")
        env = {**self.env, "NUGET_PACKAGES": str(self.work / "auth-packages")}
        source = '#:package CodingPad.Test.Root@1.0.0\nConsole.WriteLine(new Fixture.Root().Child.Value);'
        try:
            events = self.run_kernel([source], env=env, path=config_dir / "script.cs")
            self.assert_ok(events)
            self.assertTrue(any(p.endswith(".nupkg") for p in requests), requests)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)
        # A fresh kernel can run with the server gone: NuGet's disk cache suffices.
        self.assert_ok(self.run_kernel([source], env=env, path=config_dir / "script.cs"))
        self.assertNotIn("token-value", json.dumps(events))


if __name__ == "__main__":
    unittest.main()
