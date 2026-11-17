using System.Collections.Concurrent;
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

    private static CSharpCompilation Compile(SyntaxTree tree, SyntaxTree? generated, ScriptDirectives directives, ScriptDependencies dependencies)
    {
        var paths = dependencies.CompilePaths.ToArray();
        var names = paths.Select(Path.GetFileNameWithoutExtension).ToHashSet(StringComparer.OrdinalIgnoreCase);
        var references = References.Where(reference => !names.Contains(Path.GetFileNameWithoutExtension(reference.Display)))
            .Concat(paths.Select(path => MetadataReference.CreateFromFile(path)));
        SyntaxTree[] trees = generated is null ? [directives.Usings, tree] : [directives.Usings, generated, tree];
        return CSharpCompilation.Create("CodingPadScript_" + Guid.NewGuid().ToString("N"),
            trees, references, directives.CompilationOptions);
    }

    private static (
        IReadOnlyDictionary<string, string> Values,
        IReadOnlyDictionary<string, string> Errors) ParseSecrets(JsonElement parameters) =>
        (ParseSecretMap(parameters, "secrets"), ParseSecretMap(parameters, "secret_errors"));

    private static IReadOnlyDictionary<string, string> ParseSecretMap(
        JsonElement parameters, string property)
    {
        var values = new Dictionary<string, string>();
        if (!parameters.TryGetProperty(property, out var secrets) ||
            secrets.ValueKind is JsonValueKind.Null or JsonValueKind.Undefined)
            return values;
        if (secrets.ValueKind != JsonValueKind.Object)
            throw new JsonException($"run params.{property} must be an object");
        foreach (var secret in secrets.EnumerateObject())
            values[secret.Name] = secret.Value.GetString() ?? "";
        return values;
    }

    private static async Task Run(long id, string source, string? scriptPath,
        string? dataSource, string? connectionJson,
        IReadOnlyDictionary<string, string> secrets,
        IReadOnlyDictionary<string, string> secretErrors,
        JsonElement? databaseClientConfig,
        RunCancellation cancellation)
    {
        var cancel = cancellation.Token;
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
        ScriptHelpers.RunScope? helpers = null;
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
            var directives = ScriptDirectives.Parse(source, cancel);
            SyntaxTree? generated = null;
            if (!string.IsNullOrWhiteSpace(dataSource))
            {
                var kind = "sqlite";
                if (!string.IsNullOrWhiteSpace(connectionJson))
                {
                    using var connection = JsonDocument.Parse(connectionJson);
                    if (connection.RootElement.TryGetProperty("kind", out var kindValue) && kindValue.ValueKind == JsonValueKind.String)
                        kind = kindValue.GetString() ?? "sqlite";
                }
                directives.AddDataPackages(kind, dataSource);
                generated = CSharpSyntaxTree.ParseText(dataSource, directives.ParseOptions, path: "CodingPadDataContext.cs", encoding: Encoding.UTF8, cancellationToken: cancel);
                ScriptHelpers.DatabaseConnection = connectionJson;
            }
            else
            {
                ScriptHelpers.DatabaseConnection = null;
            }
            var directory = scriptPath is null ? Environment.CurrentDirectory : Path.GetDirectoryName(Path.GetFullPath(scriptPath))!;
            var dependencies = await ScriptDependencies.Restore(directives, directory,
                text => Notify(context, "stream", new { runId = id, stream = "stderr", text }), cancel);
            var tree = CSharpSyntaxTree.ParseText(source, directives.ParseOptions, path: "script.cs", encoding: Encoding.UTF8, cancellationToken: cancel);
            var compilation = Compile(tree, generated, directives, dependencies);
            var last = tree.GetRoot(cancel).ChildNodes().OfType<GlobalStatementSyntax>().LastOrDefault();
            var diagnostics = compilation.GetDiagnostics(cancel).ToArray();
            if (generated is not null)
            {
                var generatedErrors = diagnostics
                    .Where(d => d.Severity == DiagnosticSeverity.Error &&
                        d.Location.GetLineSpan().Path == "CodingPadDataContext.cs")
                    .ToArray();
                if (generatedErrors.Length > 0)
                {
                    Notify(context, "stream", new
                    {
                        runId = id,
                        stream = "stderr",
                        text = "Generated C# data context was ignored because it could not compile: " +
                            string.Join("; ", generatedErrors.Select(d => d.Id + ": " + d.GetMessage())) +
                            ". DataContext will not be available in this run.\n"
                    });
                    compilation = Compile(tree, null, directives, dependencies);
                    diagnostics = compilation.GetDiagnostics(cancel).ToArray();
                }
            }
            // Empty/comment/type-only scripts are useful no-ops. Keep every
            // other diagnostic, and preserve explicit Main programs when present.
            var errors = diagnostics
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
            if (last is null && compilation.GetEntryPoint(cancel) is null)
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
                    var type = model.GetTypeInfo(expression, cancel).Type;
                    var symbol = model.GetSymbolInfo(invocation, cancel).Symbol as IMethodSymbol;
                    if (type is not null && type.SpecialType != SpecialType.System_Void &&
                        !type.IsRefLikeType && type.TypeKind is not (TypeKind.Dynamic or TypeKind.Pointer or TypeKind.FunctionPointer) &&
                        symbol?.ContainingType.ToDisplayString() != "CodingPad.DumpExtensions")
                    {
                        var wrapped = SyntaxFactory.ParseExpression($"global::CodingPad.DumpExtensions.Dump({expression.WithoutTrivia()})").WithTriviaFrom(expression);
                        var next = tree.WithRootAndOptions(tree.GetRoot(cancel).ReplaceNode(expression, wrapped), tree.Options);
                        compilation = compilation.ReplaceSyntaxTree(tree, next);
                    }
                }
            }
            using var pe = new MemoryStream();
            using var pdb = new MemoryStream();
            var result = compilation.Emit(pe, pdb, options: new EmitOptions(debugInformationFormat: DebugInformationFormat.PortablePdb), cancellationToken: cancel);
            if (!result.Success)
            {
                Error(context, "Kernel error", "Could not emit the C# script after auto-dump instrumentation. " +
                    string.Join("\n", result.Diagnostics), []);
                return;
            }
            pe.Position = pdb.Position = 0;
            cancel.ThrowIfCancellationRequested();
            // Do not scan cached graphs for source that fails compilation.
            ScriptHelpers.SetSecrets(secrets, secretErrors);
            ScriptHelpers.SetDatabase(databaseClientConfig);
            helpers = ScriptHelpers.BeginRun(cancel, text =>
                Notify(context, "stream", new { runId = id, stream = "stdout", text }));
            cancel.ThrowIfCancellationRequested();
            await Execute(pe, pdb, dependencies);
            ok = true;
        }
        catch (ScriptDirectives.Invalid exception)
        {
            Notify(context, "diagnostics", new { runId = id, diagnostics = new[] { new {
                message = exception.Message, line = exception.Line, column = 1, endLine = exception.Line, endColumn = 2 } } });
            Error(context, "Directive error", exception.Message, [new { file = "script.cs", line = exception.Line, column = 1, function = "<directive>", text = "", script = true }]);
        }
        catch (Exception exception)
        {
            lock (InspectionGate) Notify(context, "dump", new { runId = id, node = session.Error(exception) });
        }
        finally
        {
            // Close the helper context before the host can observe completion.
            helpers?.Dispose();
            ScriptHelpers.ClearDatabase();
            ScriptHelpers.ClearSecrets();
            ScriptHelpers.DatabaseConnection = null;
            cancellation.Request();
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
    private static async Task Execute(Stream pe, Stream pdb, ScriptDependencies dependencies)
    {
        var load = dependencies.CreateScriptContext();
        try
        {
            var assembly = load.LoadFromStream(pe, pdb);
            var entry = assembly.EntryPoint!;
            var result = entry.Invoke(null, entry.GetParameters().Length == 0 ? null : [Array.Empty<string>()]);
            if (result is Task task) await task;
        }
        finally { load.Unload(); }
    }

    private static readonly ConcurrentDictionary<long, RunCancellation> Cancellations = new();
    private sealed record Request(JsonDocument? Document, string? ParseError, RunCancellation? Cancel);
    private static readonly BlockingCollection<Request> Requests = new();

    private static void Main()
    {
        ScriptHelpers.InitializeHost();
        // Neither stdin nor dispatch needs a free thread-pool thread. User code
        // may occupy the pool while waiting for its cancellation token.
        var worker = Task.Factory.StartNew(ProcessRequests, CancellationToken.None,
            TaskCreationOptions.LongRunning, TaskScheduler.Default);
        // A failed dispatcher cannot leave stdin blocked and an undrained queue.
        _ = worker.ContinueWith(failed =>
        {
            _ = failed.Exception;
            Environment.Exit(1);
        }, CancellationToken.None, TaskContinuationOptions.OnlyOnFaulted | TaskContinuationOptions.ExecuteSynchronously, TaskScheduler.Default);
        try
        {
            while (Input.ReadLine() is { } line)
            {
                JsonDocument? document = null;
                RunCancellation? cancel = null;
                string? parseError = null;
                try
                {
                    document = JsonDocument.Parse(line);
                    var root = document.RootElement;
                    if (root.GetProperty("jsonrpc").GetString() == "2.0")
                    {
                        var method = root.GetProperty("method").GetString();
                        if (method == "cancel" && !root.TryGetProperty("id", out _))
                        {
                            var runId = root.GetProperty("params").GetProperty("runId").GetInt64();
                            if (Cancellations.TryGetValue(runId, out var active)) active.Request();
                            document.Dispose();
                            continue;
                        }
                        if (method == "run" && root.TryGetProperty("id", out var id))
                        {
                            var runId = id.GetInt64();
                            cancel = new RunCancellation();
                            if (!Cancellations.TryAdd(runId, cancel))
                            { cancel = null; }
                        }
                    }
                }
                catch (Exception error)
                {
                    // Keep parsed documents alive for validation by the worker.
                    // Invalid JSON has no document and carries its parse error.
                    parseError = error.Message;
                }
                Requests.Add(new(document, parseError, cancel));
            }
        }
        finally
        {
            foreach (var cancellation in Cancellations.Values) cancellation.Request();
            Requests.CompleteAdding();
        }
        // EOF means the host is gone. Cooperative scripts finish first; an
        // uncooperative script must not keep its orphaned kernel alive forever.
        if (!worker.Wait(TimeSpan.FromSeconds(2))) Environment.Exit(1);
        worker.GetAwaiter().GetResult();
    }

    private sealed class RunCancellation
    {
        private readonly CancellationTokenSource source = new();
        private int requested;
        internal CancellationToken Token => source.Token;

        internal void Request()
        {
            if (Interlocked.Exchange(ref requested, 1) == 0) CancellationDispatcher.Enqueue(this);
            // Wait only for the cancellation flag, which is set before callbacks.
            // This makes the token cancelled before run_finished is observable.
            SpinWait.SpinUntil(() => source.IsCancellationRequested);
        }

        internal void CancelCallbacks()
        {
            try { source.Cancel(); }
            catch (Exception) { /* User cancellation callbacks may throw. */ }
        }

        // Do not dispose the source: scripts may retain the token or WaitHandle
        // after completion. Once they release it, GC reclaims its managed handle.
    }

    // Workers are reused across runs. If a user callback blocks, another worker
    // can still signal later runs; excess workers retire once they become idle.
    private static class CancellationDispatcher
    {
        private static readonly object Gate = new();
        private static readonly Queue<RunCancellation> Queue = new();
        private static int workers, busy;
        internal static void Enqueue(RunCancellation cancellation)
        {
            lock (Gate)
            {
                Queue.Enqueue(cancellation);
                if (Queue.Count > workers - busy)
                {
                    workers++;
                    var thread = new Thread(Work) { IsBackground = true, Name = "CodingPad cancellation" };
                    // Persistent workers must not retain a run's AsyncLocals.
                    if (ExecutionContext.IsFlowSuppressed()) thread.Start();
                    else { using var flow = ExecutionContext.SuppressFlow(); thread.Start(); }
                }
                Monitor.PulseAll(Gate);
            }
        }
        private static void Work()
        {
            while (true)
            {
                RunCancellation? item;
                lock (Gate)
                {
                    while (Queue.Count == 0)
                        if (!Monitor.Wait(Gate, TimeSpan.FromSeconds(30)) && Queue.Count == 0 && workers > 1)
                        { workers--; return; }
                    item = Queue.Dequeue();
                    busy++;
                }
                try { item.CancelCallbacks(); }
                finally
                {
                    item = null;
                    lock (Gate) busy--;
                }
            }
        }
    }

    private static void ProcessRequests()
    {
        Console.SetOut(TextWriter.Null);
        Console.SetIn(TextReader.Null);
        foreach (var request in Requests.GetConsumingEnumerable())
        {
            using var document = request.Document;
            var cancel = request.Cancel;
            long? id = null;
            try
            {
                if (document is null) throw new JsonException(request.ParseError);
                var root = document.RootElement;
                if (!root.TryGetProperty("id", out var value)) continue;
                id = value.GetInt64();
                if (root.GetProperty("jsonrpc").GetString() != "2.0") throw new JsonException("Expected JSON-RPC 2.0");
                if (root.GetProperty("method").GetString() == "run")
                {
                    var parameters = root.GetProperty("params");
                    var secrets = ParseSecrets(parameters);
                    string? dataSource = null;
                    string? connectionJson = null;
                    if (parameters.TryGetProperty("data", out var data) && data.ValueKind == JsonValueKind.Object)
                    {
                        if (data.TryGetProperty("source", out var generated) && generated.ValueKind == JsonValueKind.String)
                            dataSource = generated.GetString();
                        if (data.TryGetProperty("connection", out var connection) && connection.ValueKind == JsonValueKind.Object)
                            connectionJson = connection.GetRawText();
                    }
                    Run(id.Value, parameters.GetProperty("source").GetString()!,
                        parameters.TryGetProperty("path", out var path) ? path.GetString() : null,
                        dataSource, connectionJson,
                        secrets.Item1, secrets.Item2,
                        parameters.TryGetProperty("connection", out var databaseClientConfig) && databaseClientConfig.ValueKind == JsonValueKind.Object ? databaseClientConfig : null,
                        cancel ?? new RunCancellation()).GetAwaiter().GetResult();
                }
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
                    cancel.Request();
                }
            }
        }
        Session?.Dispose();
    }
}
