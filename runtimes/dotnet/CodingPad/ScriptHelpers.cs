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
        var refreshed = false;
        lock (Gate)
        {
            EnsureActive(scope);
            if (Values.TryGetValue(key, out var entry))
            {
                if (entry.Value is T || (entry.Value is null && default(T) is null))
                    return (T)entry.Value!;
                Values.Remove(key);
            }
            refreshed = Refreshed.Remove(key);
        }
        if (refreshed) scope?.Note("cache refreshed: script types changed\n");
        var value = factory(); // Failed factories are never stored.
        lock (Gate)
        {
            EnsureActive(scope);
            if (Values.TryGetValue(key, out var winner) &&
                (winner.Value is T || (winner.Value is null && default(T) is null)))
                return (T)winner.Value!;
            Values[key] = new(value);
        }
        return value;
    }

    private static void EnsureActive(RunScope? scope)
    {
        if (scope is { Active: false }) throw new InvalidOperationException("The script run has already finished.");
    }

    // Inspect storage, not properties or IEnumerable: those can execute user code.
    // Unknown or oversized graphs are conservatively refreshed on the next run.
    private static bool IsReusable(object? root)
    {
        var pending = new Stack<object>();
        var seen = new HashSet<object>(ReferenceEqualityComparer.Instance);
        if (root is not null) pending.Push(root);
        var budget = 100_000;
        try
        {
            while (pending.TryPop(out var value))
            {
                if (--budget < 0) return false;
                if (!seen.Add(value)) continue;
                var type = value.GetType();
                if (ScriptType(type)) return false;
                if (value is Type reflected)
                {
                    if (ScriptType(reflected)) return false;
                    continue;
                }
                // Delegates and reflection objects can retain code through runtime handles.
                if (value is Delegate or MemberInfo or Assembly or AssemblyLoadContext) return false;
                // Native handles can hide managed targets (weak references, GCHandle,
                // dependent handles). Their storage cannot prove safe reuse.
                if (value is IntPtr or UIntPtr) return false;
                if (type.IsPrimitive || type.IsEnum || value is string or decimal or DateTime or DateTimeOffset or TimeSpan or Guid) continue;
                if (value is Array array)
                {
                    var element = type.GetElementType()!;
                    if (element == typeof(IntPtr) || element == typeof(UIntPtr)) return false;
                    if (element.IsPrimitive || element.IsEnum || element == typeof(string) ||
                        element == typeof(decimal) || element == typeof(DateTime) ||
                        element == typeof(DateTimeOffset) || element == typeof(TimeSpan) || element == typeof(Guid)) continue;
                    if (array.Length > budget) return false;
                    foreach (var item in array)
                        if (item is not null) pending.Push(item);
                    continue;
                }
                for (var parent = type; parent is not null; parent = parent.BaseType)
                    foreach (var field in parent.GetFields(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly))
                    {
                        if (field.FieldType.IsPointer || field.FieldType.IsFunctionPointer) return false;
                        if (field.GetValue(value) is { } child) pending.Push(child);
                    }
            }
            return true;
        }
        catch { return false; }
    }

    private static bool ScriptType(Type type) =>
        AssemblyLoadContext.GetLoadContext(type.Assembly)?.IsCollectible == true ||
        (type.HasElementType && ScriptType(type.GetElementType()!)) ||
        (type.IsGenericType && type.GetGenericArguments().Any(ScriptType));

    /// <summary>Host integration: install a fresh run context without clearing safe cached values.</summary>
    public static RunScope BeginRun(CancellationToken token, Action<string> note)
    {
        var scope = new RunScope(token, note);
        lock (Gate)
        {
            // Recheck mutable containers too: a previously safe List<object> may
            // have acquired a script-defined value since its factory returned.
            foreach (var (key, entry) in Values.ToArray())
                if (!IsReusable(entry.Value))
                {
                    Values.Remove(key);
                    Refreshed.Add(key);
                }
        }
        Current.Value = scope;
        return scope;
    }

    public sealed class RunScope : IDisposable
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
