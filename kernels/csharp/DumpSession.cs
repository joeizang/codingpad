using System.Collections;
using System.Diagnostics;
using System.Globalization;
using System.Reflection;
using System.Text.Json;
using CodingPad;

// One registry per run. Retains script objects (and their collectible assembly)
// only while the result can still be expanded.
internal sealed class DumpSession(long runId) : IDisposable
{
    private readonly Dictionary<string, object> handles = [];
    private readonly Dictionary<object, string> identities = new(ReferenceEqualityComparer.Instance);
    private readonly Dictionary<object, Sequence> sequences = new(ReferenceEqualityComparer.Instance);
    private readonly List<(LiveValue Value, Action<object?> Handler)> subscriptions = [];
    public Action<string, object?>? Update { get; set; }
    private readonly Dictionary<object, Dictionary<MemberInfo, PropertyValue>> properties = new(ReferenceEqualityComparer.Instance);
    private int serial;
    private const int PageSize = 100;
    private sealed class Budget
    {
        public int Remaining = 1000;
        public int Characters = 200000;
        public int ImageBytes = 4 * 1024 * 1024;
    }
    private sealed class PropertyValue(Func<object?> read)
    {
        private bool resolved;
        private object? value;
        private Exception? failure;
        public object? Read()
        {
            if (!resolved)
            {
                try { value = read(); }
                catch (Exception error) { failure = error; }
                resolved = true;
            }
            if (failure is not null) System.Runtime.ExceptionServices.ExceptionDispatchInfo.Capture(failure).Throw();
            return value;
        }
    }
    private sealed class Sequence(IEnumerator iterator)
    {
        public readonly IEnumerator Iterator = iterator;
        public readonly List<object?> Values = [];
        public MemberInfo[]? Columns;
        public bool Finished;
        public Exception? Failure;
    }

    private string Id() => $"{runId}:n{++serial}";
    private string Handle(object value)
    {
        if (identities.TryGetValue(value, out var handle)) return handle;
        handle = $"{runId}:h{++serial}";
        identities.Add(value, handle);
        handles.Add(handle, value);
        return handle;
    }
    private object Lazy(object value) => new
    {
        id = Id(),
        kind = "lazy",
        type = value.GetType().ToString(),
        preview = value is PropertyValue ? "property (evaluate on expand)" : value.GetType().ToString(),
        handle = Handle(value)
    };

    public object Node(object? value) => Node(value, 0, new Budget(), new HashSet<object>(ReferenceEqualityComparer.Instance));

    private object Node(object? value, int depth, Budget budget, HashSet<object> path, bool cell = false)
    {
        if (value is not null && (depth >= 6 || budget.Remaining <= 1)) return Lazy(value);
        budget.Remaining--;
        if (value is null) return new { id = Id(), kind = "null" };
        if (value is bool || value is byte or sbyte or short or ushort or int or uint or long or ulong or decimal ||
            value is double d && double.IsFinite(d) || value is float f && float.IsFinite(f))
            return new { id = Id(), kind = "primitive", v = value };
        if (value is string or char or Enum or DateTime or DateTimeOffset or TimeSpan or Guid or double or float)
        {
            var text = Convert.ToString(value, CultureInfo.InvariantCulture) ?? "";
            var limit = Math.Min(cell ? 1000 : 10000, Math.Max(0, budget.Characters));
            budget.Characters -= Math.Min(text.Length, limit);
            return new { id = Id(), kind = "string", v = text[..Math.Min(text.Length, limit)], type = value.GetType().ToString(), truncated = text.Length > limit, total = text.Length };
        }
        if (value is Exception exception) return Error(exception, budget);
        if (cell && !IsScalar(value)) return Lazy(value);
        if (value is HtmlValue html)
        {
            var length = Math.Min(html.Html.Length, Math.Max(0, budget.Characters));
            budget.Characters -= length;
            return new { id = Id(), kind = "html", html = html.Html[..length] };
        }
        if (value is ImageValue image)
        {
            if (image.Bytes.Length > 4 * 1024 * 1024) return Error(new ArgumentException("Images must be no larger than 4 MiB."), budget);
            if (image.Bytes.Length > budget.ImageBytes) return Lazy(value);
            budget.ImageBytes -= image.Bytes.Length;
            if (image.Mime is not ("image/png" or "image/jpeg" or "image/gif" or "image/webp" or "image/bmp"))
                return Error(new ArgumentException("Unsupported image MIME type."), budget);
            return new { id = Id(), kind = "image", mime = image.Mime, src = $"data:{image.Mime};base64,{Convert.ToBase64String(image.Bytes)}", name = image.Name };
        }
        if (value is LiveValue live)
        {
            if (!path.Add(live)) return new { id = Id(), kind = "string", v = "<cycle>" };
            try
            {
                var node = Node(live.Value, depth + 1, budget, path);
                var target = JsonSerializer.SerializeToElement(node).GetProperty("id").GetString()!;
                Action<object?> handler = next => Update?.Invoke(target, next);
                live.Changed += handler;
                subscriptions.Add((live, handler));
                return new { id = Id(), kind = "live", node };
            }
            finally { path.Remove(live); }
        }
        if (value is PropertyValue) return Lazy(value);
        if (!path.Add(value)) return new { id = Id(), kind = "string", v = "<cycle>", type = value.GetType().ToString() };
        try
        {
            if (value is IEnumerable enumerable) return Table(enumerable, 0, PageSize, depth, budget, path);
            return Object(value, 0, PageSize, depth, budget, path);
        }
        catch (Exception error) { return Error(error, budget); }
        finally { path.Remove(value); }
    }

    // Do not invoke ToString, indexers, static members or property getters when
    // discovering an object. A property gets its own explicit lazy handle.
    private static MemberInfo[] Members(Type type) => type.GetMembers(BindingFlags.Instance | BindingFlags.Public)
        .Where(member => member is FieldInfo || member is PropertyInfo { GetMethod.IsPublic: true } property && property.GetIndexParameters().Length == 0)
        .OrderBy(member => member.Name, StringComparer.Ordinal).ToArray();

    private static object? Read(MemberInfo member, object value) => member switch
    {
        FieldInfo field => field.GetValue(value),
        PropertyInfo property => property.GetValue(value),
        _ => null
    };

    private PropertyValue Property(object value, MemberInfo member)
    {
        if (!properties.TryGetValue(value, out var members)) properties.Add(value, members = []);
        if (!members.TryGetValue(member, out var property)) members.Add(member, property = new PropertyValue(() => Read(member, value)));
        return property;
    }

    private object Object(object value, int offset, int limit, int depth, Budget budget, HashSet<object> path)
    {
        var members = Members(value.GetType());
        var fields = new List<object>();
        foreach (var member in members.Skip(offset).Take(limit))
        {
            if (budget.Remaining <= 1 && fields.Count > 0) break;
            fields.Add(new
            {
                name = member.Name,
                value = Node(member is PropertyInfo ? Property(value, member) : Read(member, value), depth + 1, budget, path)
            });
        }
        var end = Math.Min(offset + fields.Count, members.Length);
        return new { id = Id(), kind = "object", type = value.GetType().ToString(), fields, total = members.Length, next = end < members.Length ? end.ToString(CultureInfo.InvariantCulture) : null, handle = Handle(value), truncated = end < members.Length };
    }

    private static void Close(Sequence state)
    {
        state.Finished = true;
        try { (state.Iterator as IDisposable)?.Dispose(); }
        catch (Exception error) { state.Failure ??= error; }
    }

    private object Table(IEnumerable value, int offset, int limit, int depth, Budget budget, HashSet<object> path)
    {
        if (!sequences.TryGetValue(value, out var state))
        {
            state = new Sequence(value.GetEnumerator());
            sequences.Add(value, state);
        }
        // Cursors may revisit cached pages but cannot force a long traversal.
        if (offset > state.Values.Count) throw new ArgumentException("Load the preceding sequence page first.");
        while (!state.Finished && state.Values.Count < offset + limit)
        {
            try
            {
                if (!state.Iterator.MoveNext()) { Close(state); break; }
                state.Values.Add(state.Iterator.Current);
            }
            catch (Exception error) { state.Failure = error; Close(state); }
        }
        // Pick a stable schema from the first value; nested cells remain lazy.
        if (state.Columns is null)
        {
            var first = state.Values.FirstOrDefault();
            state.Columns = first is not null && !IsScalar(first) && first is not IEnumerable && first is not (Exception or HtmlValue or ImageValue or LiveValue)
                ? Members(first.GetType()).Take(32).ToArray() : [];
        }
        var rows = state.Values.Skip(offset).Take(limit).Select(item =>
        {
            if (state.Columns.Length == 0) return new[] { Node(item, depth + 1, budget, path, cell: true) };
            return state.Columns.Select(member =>
            {
                try
                {
                    object? field = null;
                    if (item is not null && member.DeclaringType!.IsInstanceOfType(item))
                        field = member is PropertyInfo ? Property(item, member).Read() : Read(member, item);
                    return Node(field, depth + 1, budget, path, cell: true);
                }
                catch (Exception error) { return Node(error, depth + 1, budget, path, cell: true); }
            }).ToArray();
        }).ToList();
        // A failing enumerator is a visible final row, rather than a lost page.
        if (state.Failure is not null && offset + rows.Count == state.Values.Count)
            rows.Add(Enumerable.Range(0, Math.Max(1, state.Columns.Length)).Select(index => Node(index == 0 ? state.Failure : null, depth + 1, budget, path, cell: true)).ToArray());
        var total = state.Values.Count + (state.Failure is not null ? 1 : state.Finished ? 0 : 1);
        var end = offset + rows.Count;
        return new
        {
            id = Id(),
            kind = "table",
            type = value.GetType().ToString(),
            columns = state.Columns.Length == 0 ? new[] { new { name = "Value" } } : state.Columns.Select(member => new { name = member.Name }).ToArray(),
            rows,
            total,
            totalKnown = state.Finished,
            next = end < total ? end.ToString(CultureInfo.InvariantCulture) : null,
            handle = Handle(value)
        };
    }

    private static bool IsScalar(object value) => value is string or char or bool or byte or sbyte or short or ushort or int or uint or long or ulong or decimal or double or float or Enum or DateTime or DateTimeOffset or TimeSpan or Guid;

    public bool Contains(string handle) => handles.ContainsKey(handle);
    public object Expand(string handle, string? cursor, int limit)
    {
        var value = handles[handle];
        var offset = cursor is null ? 0 : int.Parse(cursor, CultureInfo.InvariantCulture);
        if (offset < 0) throw new ArgumentException("Cursor must be a nonnegative offset.");
        limit = Math.Clamp(limit, 1, PageSize);
        if (value is PropertyValue property) return Node(property.Read());
        if (value is IEnumerable enumerable && value is not string)
            return Table(enumerable, offset, limit, 0, new Budget(), new HashSet<object>(ReferenceEqualityComparer.Instance) { value });
        if (cursor is null) return Node(value);
        return Object(value, offset, limit, 0, new Budget(), new HashSet<object>(ReferenceEqualityComparer.Instance) { value });
    }

    public object Error(Exception exception) => Error(exception, new Budget());

    private object Error(Exception exception, Budget budget)
    {
        while (exception is TargetInvocationException { InnerException: { } inner }) exception = inner;
        var message = Bounded(exception.Message, budget);
        var frames = new StackTrace(exception, true).GetFrames()
            .Where(frame => frame.GetFileName() is not null).Take(Math.Max(0, Math.Min(100, budget.Remaining)))
            .Select(frame => new
            {
                file = Bounded(frame.GetFileName()!, budget),
                line = frame.GetFileLineNumber(),
                column = frame.GetFileColumnNumber(),
                function = Bounded(frame.GetMethod()?.Name ?? "", budget),
                text = "",
                script = frame.GetFileName() == "script.cs"
            }).ToArray();
        budget.Remaining -= frames.Length;
        var causes = new List<object>();
        for (var cause = exception.InnerException; cause is not null && causes.Count < 5; cause = cause.InnerException)
            causes.Add(new { type = cause.GetType().Name, message = Bounded(cause.Message, budget) });
        return new { id = Id(), kind = "error", type = exception.GetType().Name, message, traceback = frames, causes };
    }

    private static string Bounded(string text, Budget budget)
    {
        var length = Math.Min(text.Length, Math.Min(10000, Math.Max(0, budget.Characters)));
        budget.Characters -= length;
        return text[..length];
    }

    public void Dispose()
    {
        foreach (var state in sequences.Values)
            if (!state.Finished) Close(state);
        foreach (var (value, handler) in subscriptions) value.Changed -= handler;
        subscriptions.Clear();
        Update = null;
        sequences.Clear();
        identities.Clear();
        handles.Clear();
        properties.Clear();
    }
}
