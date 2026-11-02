using System.Reflection;
using System.Runtime.Loader;

namespace CodingPad;

/// <summary>Helpers shared by hosted scripts and ordinary .NET programs.</summary>
public static class ScriptHelpers
{
    private sealed record Entry(object? Value);
    private static readonly object Gate = new();
    private static readonly Dictionary<string, Entry> Values = new();
    private static readonly HashSet<string> Refreshed = new();
    private static readonly AsyncLocal<RunScope?> Current = new();
    private static bool hosted;

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
        bool refreshed;
        lock (Gate)
        {
            EnsureActive(scope);
            if (Values.TryGetValue(key, out var entry)) return Cached<T>(key, entry);
            refreshed = Refreshed.Remove(key);
        }
        if (refreshed) scope?.Note("cache refreshed: script types changed\n");
        var value = factory(); // Failed factories are never stored.
        lock (Gate)
        {
            EnsureActive(scope);
            if (Values.TryGetValue(key, out var winner)) return Cached<T>(key, winner);
            Values[key] = new(value);
        }
        return value;
    }

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
        internal bool IsReusable(object? root)
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
                    if (--budget < 0) return false;
                    if (!seen.Add(value)) continue;
                    var type = value.GetType();
                    if (ScriptType(type)) return false;
                    if (value is Type reflected)
                    {
                        if (ScriptType(reflected)) return false;
                        continue;
                    }
                    // Native and reflection handles can hide managed/code targets.
                    if (Opaque(type)) return false;
                    if (AlwaysSafe(type)) continue;
                    if (value is Array array)
                    {
                        if (array.Length > budget) return false;
                        foreach (var item in array)
                            if (item is not null) pending.Push(item);
                        continue;
                    }
                    foreach (var field in Fields(type))
                    {
                        if (AlwaysSafe(field.FieldType)) continue;
                        if (field.FieldType.IsPointer || field.FieldType.IsFunctionPointer) return false;
                        if (field.GetValue(value) is { } child) pending.Push(child);
                    }
                }
                return true;
            }
            catch (OperationCanceledException) { throw; }
            catch { return false; } // Uninspectable storage is conservatively refreshed.
        }
    }

    internal static void InitializeHost() => hosted = true;

    /// <summary>Host integration: install a fresh run context without clearing safe cached values.</summary>
    internal static RunScope BeginRun(CancellationToken token, Action<string> note)
    {
        var scope = new RunScope(token, note);
        var inspection = new CacheInspection(token);
        lock (Gate)
        {
            // Recheck mutable containers too: a previously safe List<object> may
            // have acquired a script-defined value since its factory returned.
            foreach (var (key, entry) in Values.ToArray())
                if (!inspection.IsReusable(entry.Value))
                {
                    Values.Remove(key);
                    Refreshed.Add(key);
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
        internal bool Active = true;
        public void Dispose()
        {
            lock (Gate) Active = false;
            Current.Value = null;
        }
    }
}
