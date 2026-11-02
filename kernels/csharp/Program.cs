using System.Collections.Concurrent;
using System.Threading.Channels;
using System.Runtime.CompilerServices;
using System.Runtime.Loader;
using System.Text;
using System.Text.Json;
using CodingPad;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Emit;

internal static class Program
{
    private static readonly TextWriter Protocol = new StreamWriter(Console.OpenStandardOutput(), new UTF8Encoding(false)) { AutoFlush = true };
    private static readonly TextReader Input = new StreamReader(Console.OpenStandardInput(), Encoding.UTF8);
    private static readonly object Gate = new();
    private static readonly object InspectionGate = new();
    private static readonly AsyncLocal<RunContext?> Current = new();
    private static readonly MetadataReference[] References = ((string)AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES")!)
        .Split(Path.PathSeparator)
        // The kernel's TPA also contains Roslyn and the host itself. Scripts get
        // only framework assemblies and their public CodingPad helper.
        .Where(path => string.Equals(Path.GetDirectoryName(path), Path.GetDirectoryName(typeof(object).Assembly.Location),
            OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal))
        .Append(typeof(DumpExtensions).Assembly.Location)
        .Distinct().Select(path => MetadataReference.CreateFromFile(path)).ToArray();
    private static readonly SyntaxTree Usings = CSharpSyntaxTree.ParseText("""
        global using System;
        global using System.Collections.Generic;
        global using System.Linq;
        global using System.Threading;
        global using System.Threading.Tasks;
        global using CodingPad;
        global using static CodingPad.ScriptHelpers;
        """);

    private sealed class RunContext(long id)
    {
        public long Id { get; } = id;
        public bool Active = true;
    }

    private static void Send(object message)
    {
        lock (Gate) Protocol.WriteLine(JsonSerializer.Serialize(message));
    }

    private static void Notify(RunContext context, string method, object parameters)
    {
        lock (Gate)
            if (context.Active) Send(new { jsonrpc = "2.0", method, @params = parameters });
    }

    private sealed class OutputWriter(string name) : TextWriter
    {
        public override Encoding Encoding => Encoding.UTF8;
        public override void Write(char value) => Write(value.ToString());
        public override void Write(string? value)
        {
            if (value is null || Current.Value is not { } context) return;
            for (var start = 0; start < value.Length; start += 8192)
                Notify(context, "stream", new { runId = context.Id, stream = name, text = value.Substring(start, Math.Min(8192, value.Length - start)) });
        }
        public override void WriteLine(string? value) => Write(value + NewLine);
    }

    private static DumpSession? Session;

    private static void Error(RunContext context, string type, string message, object[] frames) =>
        Notify(context, "dump", new { runId = context.Id, node = new { kind = "error", type, message, traceback = frames } });

    private static CSharpCompilation Compile(SyntaxTree tree) => CSharpCompilation.Create(
        "CodingPadScript_" + Guid.NewGuid().ToString("N"), [Usings, tree], References,
        new CSharpCompilationOptions(OutputKind.ConsoleApplication, optimizationLevel: OptimizationLevel.Debug));

    private static async Task Run(long id, string source, CancellationToken cancel)
    {
        Session?.Dispose();
        var session = Session = new DumpSession(id);
        var context = new RunContext(id);
        session.Update = (nodeId, value) =>
        {
            lock (InspectionGate)
                if (context.Active && session.UpdateNode(nodeId, value) is { } node)
                    Notify(context, "dump_update", new { runId = id, nodeId, node });
        };
        Current.Value = context;
        using var helpers = ScriptHelpers.BeginRun(cancel, text =>
            Notify(context, "stream", new { runId = id, stream = "stdout", text }));
        Console.SetOut(TextWriter.Synchronized(new OutputWriter("stdout")));
        Console.SetError(TextWriter.Synchronized(new OutputWriter("stderr")));
        Console.SetIn(TextReader.Null);
        DumpExtensions.Output.Value = value =>
        {
            lock (InspectionGate)
                if (context.Active) Notify(context, "dump", new { runId = id, node = session.Node(value) });
        };
        var ok = false;
        try
        {
            cancel.ThrowIfCancellationRequested();
            var tree = CSharpSyntaxTree.ParseText(source, path: "script.cs", encoding: Encoding.UTF8);
            var compilation = Compile(tree);
            var last = tree.GetRoot().ChildNodes().OfType<GlobalStatementSyntax>().LastOrDefault();
            // Empty/comment/type-only scripts are useful no-ops. Keep every
            // other diagnostic, and preserve explicit Main programs when present.
            var errors = compilation.GetDiagnostics()
                .Where(d => d.Severity == DiagnosticSeverity.Error && !(last is null && d.Id == "CS5001")).ToArray();
            Notify(context, "diagnostics", new
            {
                runId = id,
                diagnostics = errors.Select(d =>
                {
                    var span = d.Location.GetLineSpan();
                    return new
                    {
                        message = $"{d.Id}: {d.GetMessage()}",
                        line = span.StartLinePosition.Line + 1,
                        column = span.StartLinePosition.Character + 1,
                        endLine = span.EndLinePosition.Line + 1,
                        endColumn = span.EndLinePosition.Character + 1
                    };
                }).ToArray()
            });
            if (errors.Length > 0)
            {
                foreach (var error in errors)
                {
                    var start = error.Location.GetLineSpan().StartLinePosition;
                    Error(context, "Compile error", $"{error.Id}: {error.GetMessage()}", [new { file = "script.cs", line = start.Line + 1, column = start.Character + 1, function = "<compile>", text = "", script = true }]);
                }
                return;
            }
            // First validate the original C#: an invalid bare final expression must
            // never become valid as a side effect of auto-dump instrumentation.
            if (last is null && compilation.GetEntryPoint(default) is null)
            {
                ok = true;
                return;
            }
            if (last?.Statement is ExpressionStatementSyntax statement)
            {
                var expression = statement.Expression;
                var call = expression is AwaitExpressionSyntax awaited ? awaited.Expression : expression;
                if (call is InvocationExpressionSyntax invocation)
                {
                    var model = compilation.GetSemanticModel(tree);
                    var type = model.GetTypeInfo(expression).Type;
                    var symbol = model.GetSymbolInfo(invocation).Symbol as IMethodSymbol;
                    if (type is not null && type.SpecialType != SpecialType.System_Void &&
                        !type.IsRefLikeType && type.TypeKind is not (TypeKind.Dynamic or TypeKind.Pointer or TypeKind.FunctionPointer) &&
                        symbol?.ContainingType.ToDisplayString() != "CodingPad.DumpExtensions")
                    {
                        var wrapped = SyntaxFactory.ParseExpression($"global::CodingPad.DumpExtensions.Dump({expression.WithoutTrivia()})").WithTriviaFrom(expression);
                        var next = tree.WithRootAndOptions(tree.GetRoot().ReplaceNode(expression, wrapped), tree.Options);
                        compilation = compilation.ReplaceSyntaxTree(tree, next);
                    }
                }
            }
            using var pe = new MemoryStream();
            using var pdb = new MemoryStream();
            var result = compilation.Emit(pe, pdb, options: new EmitOptions(debugInformationFormat: DebugInformationFormat.PortablePdb));
            if (!result.Success)
            {
                Error(context, "Kernel error", "Could not emit the C# script after auto-dump instrumentation. " +
                    string.Join("\n", result.Diagnostics), []);
                return;
            }
            pe.Position = pdb.Position = 0;
            cancel.ThrowIfCancellationRequested();
            await Execute(pe, pdb);
            ok = true;
        }
        catch (Exception exception)
        {
            lock (InspectionGate) Notify(context, "dump", new { runId = id, node = session.Error(exception) });
        }
        finally
        {
            lock (InspectionGate)
            lock (Gate)
            {
                context.Active = false;
                Send(new { jsonrpc = "2.0", method = "run_finished", @params = new { runId = id, ok } });
                Send(new { jsonrpc = "2.0", id, result = new { ok } });
            }
            DumpExtensions.Output.Value = null;
            Current.Value = null;
        }
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private static async Task Execute(Stream pe, Stream pdb)
    {
        var load = new AssemblyLoadContext("script", isCollectible: true);
        try
        {
            var assembly = load.LoadFromStream(pe, pdb);
            var entry = assembly.EntryPoint!;
            var result = entry.Invoke(null, entry.GetParameters().Length == 0 ? null : [Array.Empty<string>()]);
            if (result is Task task) await task;
        }
        finally { load.Unload(); }
    }

    private static readonly ConcurrentDictionary<long, CancellationTokenSource> Cancellations = new();
    private static readonly Channel<(string Line, CancellationTokenSource? Cancel)> Requests = Channel.CreateUnbounded<(string, CancellationTokenSource?)>();

    private static async Task Main()
    {
        var worker = Task.Run(ProcessRequests);
        while (await Input.ReadLineAsync() is { } line)
        {
            CancellationTokenSource? cancel = null;
            try
            {
                using var document = JsonDocument.Parse(line);
                var root = document.RootElement;
                if (root.GetProperty("jsonrpc").GetString() == "2.0")
                {
                    var method = root.GetProperty("method").GetString();
                    if (method == "cancel")
                    {
                        var runId = root.GetProperty("params").GetProperty("runId").GetInt64();
                        if (Cancellations.TryGetValue(runId, out var active))
                            _ = CancelSafely(active);
                        continue;
                    }
                    if (method == "run" && root.TryGetProperty("id", out var id))
                    {
                        cancel = new CancellationTokenSource();
                        if (!Cancellations.TryAdd(id.GetInt64(), cancel))
                        { cancel.Dispose(); cancel = null; }
                    }
                }
            }
            catch (Exception) { /* Request validation belongs to the worker. */ }
            await Requests.Writer.WriteAsync((line, cancel));
        }
        Requests.Writer.Complete();
        await worker;
    }

    private static async Task CancelSafely(CancellationTokenSource source)
    {
        try { await source.CancelAsync(); }
        catch (Exception) { /* User cancellation callbacks may throw. */ }
    }

    private static async Task ProcessRequests()
    {
        Console.SetOut(TextWriter.Null);
        Console.SetIn(TextReader.Null);
        await foreach (var (line, cancel) in Requests.Reader.ReadAllAsync())
        {
            long? id = null;
            try
            {
                using var document = JsonDocument.Parse(line);
                var root = document.RootElement;
                if (!root.TryGetProperty("id", out var value)) continue;
                id = value.GetInt64();
                if (root.GetProperty("jsonrpc").GetString() != "2.0") throw new JsonException("Expected JSON-RPC 2.0");
                if (root.GetProperty("method").GetString() == "run")
                    await Run(id.Value, root.GetProperty("params").GetProperty("source").GetString()!, cancel?.Token ?? CancellationToken.None);
                else if (root.GetProperty("method").GetString() == "expand")
                {
                    var parameters = root.GetProperty("params");
                    var handle = parameters.GetProperty("handle").GetString()!;
                    if (Session is null || !Session.Contains(handle))
                        Send(new { jsonrpc = "2.0", id, error = new { code = -32001, message = "That preview is no longer available. Run the script again." } });
                    else
                    {
                        try
                        {
                            var cursor = parameters.TryGetProperty("cursor", out var c) ? c.GetString() : null;
                            var limit = parameters.TryGetProperty("limit", out var l) && l.ValueKind != JsonValueKind.Null ? l.GetInt32() : 100;
                            Send(new { jsonrpc = "2.0", id, result = new { node = Session.Expand(handle, cursor, limit) } });
                        }
                        catch (Exception error)
                        {
                            Send(new { jsonrpc = "2.0", id, result = new { node = Session.Error(error) } });
                        }
                    }
                }
                else Send(new { jsonrpc = "2.0", id, error = new { code = -32601, message = "Unknown method" } });
            }
            catch (Exception error)
            {
                Send(new { jsonrpc = "2.0", id, error = new { code = -32600, message = error.Message } });
            }
            finally
            {
                if (cancel is not null)
                {
                    if (id is { } runId) Cancellations.TryRemove(runId, out _);
                    cancel.Dispose();
                }
            }
        }
        Session?.Dispose();
    }
}
