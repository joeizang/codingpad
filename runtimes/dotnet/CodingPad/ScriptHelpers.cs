using System.Reflection;
using System.Runtime.Loader;

namespace CodingPad;

/// <summary>Helpers shared by hosted scripts and ordinary .NET programs.</summary>
public static class ScriptHelpers
{
    private sealed record Entry(object? Value);
    private static readonly object Gate = new();
    private static readonly Dictionary<string, Entry> Values = new();
    private static readonly AsyncLocal<RunScope?> Current = new();
    private static bool hosted;
    private static readonly MethodInfo AsyncCache = typeof(ScriptHelpers).GetMethod(nameof(CacheTask), BindingFlags.Static | BindingFlags.NonPublic)!;

    public static CancellationToken Cancel => Current.Value?.Token ?? CancellationToken.None;

    /// <summary>The vault integration will be available in M3.</summary>
    public static string Secret(string name) => throw new NotSupportedException(
        "Secret storage is not available yet; vault support is planned for M3.");

    /// <summary>Keep successful results across runs. Factories execute outside the cache lock.</summary>
    public static T Cache<T>(string key, Func<T> factory)
    {
        ArgumentNullException.ThrowIfNull(key);
        ArgumentNullException.ThrowIfNull(factory);
        var scope = Current.Value;
        // An overload with Func<Task<T>> makes existing null/throw-only factories
        // ambiguous. Dispatch Task<T> factories here, preserving the public API.
        if (typeof(T).IsGenericType && typeof(T).GetGenericTypeDefinition() == typeof(Task<>))
            return (T)AsyncCache.MakeGenericMethod(typeof(T).GetGenericArguments()[0]).Invoke(null, [key, factory, scope])!;
        if (TryCached<T>(key, scope, out var cached)) return cached;
        return Store(key, scope, factory()); // Failed factories are never stored.
    }

    private static async Task<T> CacheTask<T>(string key, Func<Task<T>> factory, RunScope? scope)
    {
        if (TryCached<T>(key, scope, out var cached)) return cached;
        var value = await factory().ConfigureAwait(false);
        return Store(key, scope, value);
    }

    private static bool TryCached<T>(string key, RunScope? scope, out T value)
    {
        lock (Gate)
        {
            EnsureActive(scope);
            if (Values.TryGetValue(key, out var entry))
            {
                value = Cached<T>(key, entry);
                return true;
            }
        }
        value = default!;
        return false;
    }

    private static T Store<T>(string key, RunScope? scope, T value)
    {
        RefreshReason? reason = null;
        lock (Gate)
        {
            EnsureActive(scope);
            if (Values.TryGetValue(key, out var winner)) return Cached<T>(key, winner);
            Values[key] = new(value);
            // A failed factory leaves its note pending. Only the first successful
            // store consumes it; a concurrent winner emits the note exactly once.
            if (scope is not null && scope.Refreshed.Remove(key, out var refreshed)) reason = refreshed;
        }
        if (reason is not null) scope?.Note(RefreshMessage(reason.Value));
        return value;
    }

    internal enum RefreshReason { ScriptTypes, OpaqueStorage, Budget, InspectionFailure }

    private static string RefreshMessage(RefreshReason reason) => reason switch
    {
        RefreshReason.ScriptTypes => "cache refreshed: script types changed\n",
        RefreshReason.OpaqueStorage => "cache refreshed: value contains opaque handles or callbacks\n",
        RefreshReason.Budget => "cache refreshed: inspection budget exceeded\n",
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

    private static bool IsScalar(Type type) =>
        type != typeof(IntPtr) && type != typeof(UIntPtr) &&
        (type.IsPrimitive || type.IsEnum || type == typeof(string) || type == typeof(decimal) ||
         type == typeof(DateTime) || type == typeof(DateTimeOffset) || type == typeof(TimeSpan) || type == typeof(Guid));

    // Metadata is shared across entries for this inspection only. A process-wide
    // Type dictionary would keep collectible script assemblies alive forever.
    private sealed class CacheInspection(CancellationToken cancel)
    {
        private readonly Dictionary<Type, bool> scriptTypes = new();
        private readonly Dictionary<Type, bool> safeTypes = new();
        private readonly Dictionary<Type, FieldInfo[]> fields = new();

        private bool ScriptType(Type type)
        {
            if (scriptTypes.TryGetValue(type, out var result)) return result;
            return scriptTypes[type] = AssemblyLoadContext.GetLoadContext(type.Assembly)?.IsCollectible == true ||
                (type.HasElementType && ScriptType(type.GetElementType()!)) ||
                (type.IsGenericType && type.GetGenericArguments().Any(ScriptType));
        }

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

        // Prove safety from storage types when possible. Sealed reference types
        // cannot conceal a script subclass; object/interface fields need a walk.
        // Mark recursive shapes uncertain while computing their verdict.
        private bool AlwaysSafe(Type type)
        {
            cancel.ThrowIfCancellationRequested();
            if (safeTypes.TryGetValue(type, out var result)) return result;
            safeTypes[type] = false;
            if (ScriptType(type) || Opaque(type)) return false;
            if (IsScalar(type)) return safeTypes[type] = true;
            if (type.IsArray) return safeTypes[type] = AlwaysSafe(type.GetElementType()!);
            if (!type.IsValueType && !type.IsSealed) return false;
            return safeTypes[type] = Fields(type).All(field => AlwaysSafe(field.FieldType));
        }

        // Inspect storage, not properties or IEnumerable: those can execute user code.
        internal RefreshReason? ReasonToRefresh(object? root)
        {
            var pending = new Stack<object>();
            var seen = new HashSet<object>(ReferenceEqualityComparer.Instance);
            if (root is not null) pending.Push(root);
            var budget = 100_000;
            try
            {
                while (pending.TryPop(out var value))
                {
                    cancel.ThrowIfCancellationRequested();
                    if (--budget < 0) return RefreshReason.Budget;
                    if (!seen.Add(value)) continue;
                    var type = value.GetType();
                    if (ScriptType(type)) return RefreshReason.ScriptTypes;
                    if (value is Type reflected)
                    {
                        if (ScriptType(reflected)) return RefreshReason.ScriptTypes;
                        continue;
                    }
                    // Native and reflection handles can hide managed/code targets.
                    if (Opaque(type)) return RefreshReason.OpaqueStorage;
                    if (AlwaysSafe(type)) continue;
                    if (value is Array array)
                    {
                        if (array.Length > budget) return RefreshReason.Budget;
                        foreach (var item in array)
                            if (item is not null) pending.Push(item);
                        continue;
                    }
                    foreach (var field in Fields(type))
                    {
                        if (AlwaysSafe(field.FieldType)) continue;
                        if (field.FieldType.IsPointer || field.FieldType.IsFunctionPointer) return RefreshReason.OpaqueStorage;
                        if (field.GetValue(value) is { } child) pending.Push(child);
                    }
                }
                return null;
            }
            catch (OperationCanceledException) { throw; }
            catch { return RefreshReason.InspectionFailure; }
        }
    }

    internal static void InitializeHost() => hosted = true;

    /// <summary>Host integration: install a fresh run context without clearing safe cached values.</summary>
    internal static RunScope BeginRun(CancellationToken token, Action<string> note)
    {
        var scope = new RunScope(token, note);
        var inspection = new CacheInspection(token);
        KeyValuePair<string, Entry>[] snapshot;
        lock (Gate) snapshot = Values.ToArray();
        var stale = new List<(string Key, Entry Entry, RefreshReason Reason)>();
        // Reflection runs outside the cache lock. Metadata is reused across this
        // snapshot, and only unchanged entries are removed when applying results.
        foreach (var (key, entry) in snapshot)
            if (inspection.ReasonToRefresh(entry.Value) is { } reason) stale.Add((key, entry, reason));
        token.ThrowIfCancellationRequested();
        lock (Gate)
        {
            foreach (var (key, entry, reason) in stale)
                if (Values.TryGetValue(key, out var current) && ReferenceEquals(current, entry))
                {
                    Values.Remove(key);
                    scope.Refreshed.Add(key, reason);
                }
        }
        Current.Value = scope;
        return scope;
    }

    internal sealed class RunScope : IDisposable
    {
        internal RunScope(CancellationToken token, Action<string> note)
        { Token = token; Note = note; }
        internal CancellationToken Token { get; }
        internal Action<string> Note { get; }
        internal readonly Dictionary<string, RefreshReason> Refreshed = new();
        internal bool Active = true;
        public void Dispose()
        {
            lock (Gate)
            {
                Active = false;
                Refreshed.Clear(); // Unused notes live only for this run.
            }
            Current.Value = null;
        }
    }
}
