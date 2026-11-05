using System.Reflection;
using System.Runtime.Loader;

namespace CodingPad;

/// <summary>Helpers shared by hosted scripts and ordinary .NET programs.</summary>
public static class ScriptHelpers
{
    private sealed class Entry(object? value, long generation)
    {
        internal readonly object? Value = value;
        internal long Generation = generation;
    }
    private static readonly object Gate = new();
    private static readonly Dictionary<string, Entry> Values = new();
    private static readonly Dictionary<string, Entry> ScriptRoots = new();
    private static readonly AsyncLocal<RunScope?> Current = new();
    private static IReadOnlyDictionary<string, string> Secrets =
        new Dictionary<string, string>();
    private static IReadOnlyDictionary<string, string> SecretErrors =
        new Dictionary<string, string>();
    private static readonly AsyncLocal<DatabaseClient?> Database = new();
    /// <summary>The database attached to this run.</summary>
    public static DatabaseClient db => Database.Value ?? new DatabaseClient(null);
    internal static void SetDatabase(System.Text.Json.JsonElement? config) => Database.Value = new DatabaseClient(config);
    internal static void ClearDatabase() { Database.Value?.Dispose(); Database.Value = null; }
    private static long generation;
    private static bool hosted;

    public static CancellationToken Cancel => Current.Value?.Token ?? CancellationToken.None;

    /// <summary>Return a named secret supplied by the OS password vault.</summary>
    public static string Secret(string name)
    {
        if (string.IsNullOrWhiteSpace(name))
            throw new ArgumentException("A secret name cannot be empty.", nameof(name));
        if (SecretErrors.TryGetValue(name, out var error)) throw new KeyNotFoundException(error);
        if (Secrets.TryGetValue(name, out var value)) return value;
        throw new KeyNotFoundException(
            $"Secret \"{name}\" was not found. Add it in Secrets… or set it for this session.");
    }

    internal static void SetSecrets(
        IReadOnlyDictionary<string, string> secrets,
        IReadOnlyDictionary<string, string> errors)
    {
        Secrets = secrets;
        SecretErrors = errors;
    }

    internal static void ClearSecrets()
    {
        Secrets = new Dictionary<string, string>();
        SecretErrors = new Dictionary<string, string>();
    }

    /// <summary>Keep successful results across runs. Factories execute outside the cache lock.</summary>
    public static T Cache<T>(string key, Func<T> factory)
    {
        ArgumentNullException.ThrowIfNull(key);
        ArgumentNullException.ThrowIfNull(factory);
        var scope = Current.Value;
        if (AsyncFactory<T>.Invoke is { } invoke) return invoke(key, factory, scope);
        if (TryCached<T>(key, scope, out var cached)) return cached;
        return Store(key, scope, factory());
    }

    // One strongly typed adapter per factory return type. No per-call reflection
    // or process-wide Type dictionary retaining collectible generic arguments.
    private static class AsyncFactory<T>
    {
        internal static readonly Func<string, Func<T>, RunScope?, T>? Invoke = Create();
        private static Func<string, Func<T>, RunScope?, T>? Create()
        {
            if (!typeof(T).IsGenericType) return null;
            var definition = typeof(T).GetGenericTypeDefinition();
            var method = definition == typeof(Task<>) ? nameof(CacheTask) :
                definition == typeof(ValueTask<>) ? nameof(CacheValueTask) : null;
            return method is null ? null : typeof(ScriptHelpers).GetMethod(method, BindingFlags.Static | BindingFlags.NonPublic)!
                .MakeGenericMethod(typeof(T).GetGenericArguments()[0])
                .CreateDelegate<Func<string, Func<T>, RunScope?, T>>();
        }
    }

    // Validation and factory invocation happen synchronously, even if the caller
    // ignores the returned awaitable. Only awaiting and storing are asynchronous.
    private static Task<T> CacheTask<T>(string key, Func<Task<T>> factory, RunScope? scope) =>
        TryCached<T>(key, scope, out var cached) ? Task.FromResult(cached) : CompleteTask(key, scope, factory());

    private static async Task<T> CompleteTask<T>(string key, RunScope? scope, Task<T> task) =>
        Store(key, scope, await task.ConfigureAwait(false));

    private static ValueTask<T> CacheValueTask<T>(string key, Func<ValueTask<T>> factory, RunScope? scope) =>
        TryCached<T>(key, scope, out var cached) ? new(cached) : CompleteValueTask(key, scope, factory());

    private static async ValueTask<T> CompleteValueTask<T>(string key, RunScope? scope, ValueTask<T> task) =>
        Store(key, scope, await task.ConfigureAwait(false));

    private static bool TryCached<T>(string key, RunScope? scope, out T value)
    {
        while (true)
        {
            Entry entry;
            lock (Gate)
            {
                EnsureActive(scope);
                if (!Values.TryGetValue(key, out entry!)) { value = default!; return false; }
                if (scope is null || entry.Generation == scope.Generation)
                { value = Cached<T>(key, entry); return true; }
            }
            // Inspect just this key, once per run, outside the cache lock. A
            // racing lookup/store may supersede the snapshot while it is checked.
            var reason = scope.Inspect(entry.Value);
            lock (Gate)
            {
                EnsureActive(scope);
                if (!Values.TryGetValue(key, out var current) || !ReferenceEquals(current, entry) ||
                    entry.Generation == scope.Generation) continue;
                if (reason is not null)
                {
                    Values.Remove(key);
                    ScriptRoots.Remove(key);
                    scope.Refreshed[key] = reason.Value;
                    value = default!;
                    return false;
                }
                entry.Generation = scope.Generation;
                value = Cached<T>(key, entry);
                return true;
            }
        }
    }

    private static T Store<T>(string key, RunScope? scope, T value)
    {
        RefreshReason? reason = null;
        var scriptRoot = value is not null && IsScriptType(value.GetType());
        lock (Gate)
        {
            EnsureActive(scope);
            if (Values.TryGetValue(key, out var winner)) return Cached<T>(key, winner);
            var entry = Values[key] = new(value, scope?.Generation ?? 0);
            if (scriptRoot) ScriptRoots[key] = entry;
            if (scope is not null && scope.Refreshed.Remove(key, out var refreshed)) reason = refreshed;
        }
        if (reason is not null) scope?.Note(RefreshMessage(reason.Value));
        return value;
    }

    internal enum RefreshReason { ScriptTypes, OpaqueStorage, InspectionFailure }
    private static string RefreshMessage(RefreshReason reason) => reason switch
    {
        RefreshReason.ScriptTypes => "cache refreshed: script types changed\n",
        RefreshReason.OpaqueStorage => "cache refreshed: value contains opaque handles or callbacks\n",
        _ => "cache refreshed: value could not be inspected safely\n"
    };

    private static T Cached<T>(string key, Entry entry)
    {
        if (entry.Value is T || (entry.Value is null && default(T) is null)) return (T)entry.Value!;
        throw new InvalidOperationException($"Cache key '{key}' contains {entry.Value?.GetType().FullName ?? "null"}, " +
            $"which cannot be used as {typeof(T).FullName}. Use a different key or Reset to recompute it.");
    }

    private static void EnsureActive(RunScope? scope)
    {
        if (scope is { Active: false } || (hosted && scope is null))
            throw new InvalidOperationException("Cache is only available during an active script run.");
    }

    private static bool IsScriptType(Type type) =>
        type.Assembly.IsCollectible ||
        (type.HasElementType && IsScriptType(type.GetElementType()!)) ||
        (type.IsGenericType && type.GetGenericArguments().Any(IsScriptType));

    private static bool IsScalar(Type type) =>
        type != typeof(IntPtr) && type != typeof(UIntPtr) &&
        (type.IsPrimitive || type.IsEnum || type == typeof(string) || type == typeof(decimal) ||
         type == typeof(DateTime) || type == typeof(DateTimeOffset) || type == typeof(TimeSpan) || type == typeof(Guid));

    // Per-run metadata is released with the scope, including collectible types.
    private sealed class CacheInspection(CancellationToken cancel)
    {
        private readonly Dictionary<Type, bool> scriptTypes = new();
        private readonly Dictionary<Type, bool> safeTypes = new();
        private readonly Dictionary<Type, FieldInfo[]> fields = new();
        private bool ScriptType(Type type) => scriptTypes.TryGetValue(type, out var result) ? result :
            scriptTypes[type] = IsScriptType(type);

        private FieldInfo[] Fields(Type type)
        {
            if (fields.TryGetValue(type, out var result)) return result;
            var list = new List<FieldInfo>();
            for (var parent = type; parent is not null; parent = parent.BaseType)
                list.AddRange(parent.GetFields(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly));
            return fields[type] = list.ToArray();
        }

        private static bool Opaque(Type type) =>
            type == typeof(IntPtr) || type == typeof(UIntPtr) || type.IsPointer || type.IsFunctionPointer ||
            typeof(Delegate).IsAssignableFrom(type) || typeof(MemberInfo).IsAssignableFrom(type) ||
            typeof(Assembly).IsAssignableFrom(type) || typeof(AssemblyLoadContext).IsAssignableFrom(type);

        private bool AlwaysSafe(Type root)
        {
            if (safeTypes.TryGetValue(root, out var result)) return result;
            // Greatest fixpoint: cycles with no unsafe storage are safe. Propagate
            // every unsafe leaf back through all parents before publishing any
            // verdict, so an optimistic recursive dependency cannot escape early.
            var graph = new Dictionary<Type, Type[]>();
            var bad = new HashSet<Type>();
            var pending = new Stack<Type>();
            pending.Push(root);
            while (pending.TryPop(out var type))
            {
                cancel.ThrowIfCancellationRequested();
                if (graph.ContainsKey(type)) continue;
                graph[type] = [];
                if (safeTypes.TryGetValue(type, out var known))
                { if (!known) bad.Add(type); continue; }
                if (graph.Count > 1024 || ScriptType(type) || Opaque(type)) { bad.Add(type); continue; }
                if (IsScalar(type)) continue;
                if (!type.IsArray && !type.IsValueType && !type.IsSealed) { bad.Add(type); continue; }
                var children = type.IsArray ? new[] { type.GetElementType()! } : Fields(type).Select(field => field.FieldType).ToArray();
                graph[type] = children;
                foreach (var child in children) pending.Push(child);
            }
            var parents = new Dictionary<Type, List<Type>>();
            foreach (var (type, children) in graph)
                foreach (var child in children)
                {
                    if (!parents.TryGetValue(child, out var list)) parents[child] = list = [];
                    list.Add(type);
                }
            var unsafeTypes = new Queue<Type>(bad);
            while (unsafeTypes.TryDequeue(out var type))
                if (parents.TryGetValue(type, out var list))
                    foreach (var parent in list)
                        if (bad.Add(parent)) unsafeTypes.Enqueue(parent);
            foreach (var type in graph.Keys) safeTypes[type] = !bad.Contains(type);
            return safeTypes[root];
        }

        private sealed record ArrayFrame(System.Collections.IEnumerator Items, bool CopiesValues);

        internal RefreshReason? ReasonToRefresh(object? root)
        {
            var pending = new Stack<object>();
            var seen = new HashSet<object>(ReferenceEqualityComparer.Instance);
            RefreshReason? Visit(object? value, bool copiedValue = false)
            {
                cancel.ThrowIfCancellationRequested();
                if (value is null) return null;
                var type = value.GetType();
                if (ScriptType(type)) return RefreshReason.ScriptTypes;
                if (value is Type reflected) return ScriptType(reflected) ? RefreshReason.ScriptTypes : null;
                if (Opaque(type)) return RefreshReason.OpaqueStorage;
                if (!AlwaysSafe(type) && (copiedValue || seen.Add(value))) pending.Push(value);
                return null;
            }
            try
            {
                if (Visit(root) is { } initial) return initial;
                while (pending.TryPop(out var value))
                {
                    if (value is ArrayFrame frame)
                    {
                        if (frame.Items.MoveNext())
                        {
                            pending.Push(frame);
                            if (Visit(frame.Items.Current, frame.CopiesValues) is { } reason) return reason;
                        }
                    }
                    else if (value is Array array)
                    {
                        // Walk storage incrementally, without retaining temporary
                        // boxes for every struct in a large array. No user iterator.
                        pending.Push(new ArrayFrame(array.GetEnumerator(), value.GetType().GetElementType()!.IsValueType));
                    }
                    else foreach (var field in Fields(value.GetType()))
                    {
                        cancel.ThrowIfCancellationRequested();
                        if (AlwaysSafe(field.FieldType)) continue;
                        if (field.FieldType.IsPointer || field.FieldType.IsFunctionPointer) return RefreshReason.OpaqueStorage;
                        if (Visit(field.GetValue(value), field.FieldType.IsValueType) is { } reason) return reason;
                    }
                }
                return null;
            }
            catch (OperationCanceledException) { throw; }
            catch { return RefreshReason.InspectionFailure; }
        }
    }

    internal static void InitializeHost() => hosted = true;
    internal static RunScope BeginRun(CancellationToken token, Action<string> note)
    {
        var scope = new RunScope(Interlocked.Increment(ref generation), token, note);
        lock (Gate)
        {
            // Only known script roots need an eager sweep. Mutable nested graphs
            // are inspected lazily when requested; unused keys pay no graph walk.
            foreach (var (key, entry) in ScriptRoots)
                if (Values.TryGetValue(key, out var current) && ReferenceEquals(current, entry))
                {
                    Values.Remove(key);
                    scope.Refreshed.Add(key, RefreshReason.ScriptTypes);
                }
            ScriptRoots.Clear();
        }
        Current.Value = scope;
        return scope;
    }

    internal sealed class RunScope(long generation, CancellationToken token, Action<string> note) : IDisposable
    {
        internal long Generation { get; } = generation;
        internal CancellationToken Token { get; } = token;
        internal Action<string> Note { get; } = note;
        internal readonly Dictionary<string, RefreshReason> Refreshed = new();
        internal volatile bool Active = true;
        private readonly object inspectionGate = new();
        private CacheInspection? inspection;
        internal RefreshReason? Inspect(object? value)
        {
            lock (inspectionGate)
            {
                try { return (inspection ??= new(Token)).ReasonToRefresh(value); }
                finally { if (!Active) inspection = null; }
            }
        }
        public void Dispose()
        {
            lock (Gate) { Active = false; Refreshed.Clear(); }
            inspection = null;
            Current.Value = null;
        }
    }
}
