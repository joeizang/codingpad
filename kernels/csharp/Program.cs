using System.Collections;
using System.Diagnostics;
using System.Globalization;
using System.Reflection;
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
    private static readonly AsyncLocal<RunContext?> Current = new();
    private static readonly MetadataReference[] References = ((string)AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES")!)
        .Split(Path.PathSeparator).Append(typeof(DumpExtensions).Assembly.Location)
        .Distinct().Select(path => MetadataReference.CreateFromFile(path)).ToArray();
    private static readonly SyntaxTree Usings = CSharpSyntaxTree.ParseText("""
        global using System;
        global using System.Collections.Generic;
        global using System.Linq;
        global using System.Threading;
        global using System.Threading.Tasks;
        global using CodingPad;
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

    private static object Node(object? value)
    {
        if (value is null) return new { kind = "null" };
        if (value is bool || value is byte or sbyte or short or ushort or int or uint or long or ulong or decimal ||
            value is double d && double.IsFinite(d) || value is float f && float.IsFinite(f))
            return new { kind = "primitive", v = value };
        // Rich object inspection and deferred enumerable paging belong to M2.1.
        // Never consume an arbitrary IEnumerable or execute property getters here.
        var text = value is IEnumerable and not string ? value.GetType().ToString() : Convert.ToString(value, CultureInfo.InvariantCulture) ?? "";
        return new { kind = "string", v = text[..Math.Min(text.Length, 10000)], type = value.GetType().ToString(), truncated = text.Length > 10000 };
    }

    private static void Error(RunContext context, string type, string message, object[] frames) =>
        Notify(context, "dump", new { runId = context.Id, node = new { kind = "error", type, message, traceback = frames } });

    private static CSharpCompilation Compile(SyntaxTree tree) => CSharpCompilation.Create(
        "CodingPadScript_" + Guid.NewGuid().ToString("N"), [Usings, tree], References,
        new CSharpCompilationOptions(OutputKind.ConsoleApplication, optimizationLevel: OptimizationLevel.Debug));

    private static async Task Run(long id, string source)
    {
        var context = new RunContext(id);
        Current.Value = context;
        Console.SetOut(TextWriter.Synchronized(new OutputWriter("stdout")));
        Console.SetError(TextWriter.Synchronized(new OutputWriter("stderr")));
        Console.SetIn(TextReader.Null);
        DumpExtensions.Output.Value = value => Notify(context, "dump", new { runId = id, node = Node(value) });
        var ok = false;
        try
        {
            var tree = CSharpSyntaxTree.ParseText(source, path: "script.cs", encoding: Encoding.UTF8);
            var compilation = Compile(tree);
            var errors = compilation.GetDiagnostics().Where(d => d.Severity == DiagnosticSeverity.Error).ToArray();
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
            var last = tree.GetRoot().ChildNodes().OfType<GlobalStatementSyntax>().LastOrDefault();
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
            if (!result.Success) throw new InvalidOperationException(string.Join("\n", result.Diagnostics));
            pe.Position = pdb.Position = 0;
            await Execute(pe, pdb);
            ok = true;
        }
        catch (Exception exception)
        {
            if (exception is TargetInvocationException { InnerException: { } inner }) exception = inner;
            var frames = new StackTrace(exception, true).GetFrames()
                .Where(frame => frame.GetFileName() == "script.cs")
                .Select(frame => (object)new { file = "script.cs", line = frame.GetFileLineNumber(), column = frame.GetFileColumnNumber(), function = frame.GetMethod()?.Name ?? "", text = "", script = true }).ToArray();
            Error(context, exception.GetType().Name, exception.Message, frames);
        }
        finally
        {
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

    private static async Task Main()
    {
        Console.SetOut(TextWriter.Null);
        Console.SetIn(TextReader.Null);
        while (await Input.ReadLineAsync() is { } line)
        {
            long? id = null;
            try
            {
                using var document = JsonDocument.Parse(line);
                var root = document.RootElement;
                // Cancel is intentionally handled by the host's forced-stop fallback
                // until cooperative C# run control lands in M2.2.
                if (!root.TryGetProperty("id", out var value)) continue;
                id = value.GetInt64();
                if (root.GetProperty("jsonrpc").GetString() != "2.0") throw new JsonException("Expected JSON-RPC 2.0");
                if (root.GetProperty("method").GetString() == "run")
                    await Run(id.Value, root.GetProperty("params").GetProperty("source").GetString()!);
                else Send(new { jsonrpc = "2.0", id, error = new { code = -32601, message = "Unknown method" } });
            }
            catch (Exception error)
            {
                Send(new { jsonrpc = "2.0", id, error = new { code = -32600, message = error.Message } });
            }
        }
    }
}
