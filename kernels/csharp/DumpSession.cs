using System.Collections;
using System.Diagnostics;
using System.Globalization;
using System.Reflection;
using CodingPad;

// One registry per run. Retains script objects (and their collectible assembly)
// only while the result can still be expanded.
internal sealed class DumpSession(long runId) : IDisposable
{
    private readonly Dictionary<string, (object Value, View View)> handles = [];
    private readonly HashSet<View> views = [];
    private readonly Dictionary<string, (View Owner, View Current)> liveViews = [];
    // Each Dump (including a live update) owns its own inspection cache. Pages
    // and lazy children reuse that view without sharing stale reads with later dumps.
    private sealed class View
    {
        public readonly Dictionary<object, string> Identities = new(ReferenceEqualityComparer.Instance);
        public readonly Dictionary<object, Sequence> Sequences = new(ReferenceEqualityComparer.Instance);
        public readonly Dictionary<object, Dictionary<MemberInfo, PropertyValue>> Properties = new(ReferenceEqualityComparer.Instance);
        public readonly HashSet<View> Children = [];
        public readonly List<(LiveValue Value, Action<object?> Handler, string Target)> Subscriptions = [];
        public bool Released;
    }
    public Action<string, object?>? Update { get; set; }
    private int serial;
    private const int PageSize = 100;
    private sealed class Budget(View view)
    {
        public View View = view;
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
    private string Handle(object value, View view)
    {
        if (view.Released) throw new InvalidOperationException("That live preview has been replaced.");
        views.Add(view);
        if (view.Identities.TryGetValue(value, out var handle)) return handle;
        handle = $"{runId}:h{++serial}";
        view.Identities.Add(value, handle);
        handles.Add(handle, (value, view));
        return handle;
    }
    private object Lazy(object value, View view, string? nodeId = null) => new
    {
        id = nodeId ?? Id(),
        kind = "lazy",
        type = value.GetType().ToString(),
        preview = value is PropertyValue ? "property (evaluate on expand)" : value.GetType().ToString(),
        handle = Handle(value, view)
    };

    public object Node(object? value)
    {
        var view = new View();
        return Node(value, 0, new Budget(view), new HashSet<object>(ReferenceEqualityComparer.Instance));
    }

    private object Node(object? value, int depth, Budget budget, HashSet<object> path, bool cell = false, string? nodeId = null)
    {
        var id = nodeId ?? Id();
        if (value is not null && !IsScalar(value) && (depth >= 6 || budget.Remaining <= 1))
            return Lazy(value, budget.View, id);
        budget.Remaining--;
        if (value is null) return new { id, kind = "null" };
        if (value is bool || value is byte or sbyte or short or ushort or int or uint or long or ulong or decimal ||
            value is double d && double.IsFinite(d) || value is float f && float.IsFinite(f))
            return new { id, kind = "primitive", v = value };
        if (IsTextual(value))
        {
            var text = Convert.ToString(value, CultureInfo.InvariantCulture) ?? "";
            var limit = Math.Min(cell ? 1000 : 10000, Math.Max(0, budget.Characters));
            budget.Characters -= Math.Min(text.Length, limit);
            return new { id, kind = "string", v = text[..Math.Min(text.Length, limit)], type = value.GetType().ToString(), truncated = text.Length > limit, total = text.Length };
        }
        if (value is Exception exception) return Error(exception, budget, id);
        if (cell && !IsScalar(value)) return Lazy(value, budget.View, id);
        if (value is HtmlValue html)
        {
            var length = Math.Min(html.Html.Length, Math.Max(0, budget.Characters));
            budget.Characters -= length;
            return new { id, kind = "html", html = html.Html[..length] };
        }
        if (value is ImageValue image)
        {
            if (image.Bytes.Length > 4 * 1024 * 1024) return Error(new ArgumentException("Images must be no larger than 4 MiB."), budget, id);
            if (image.Bytes.Length > budget.ImageBytes) return Lazy(value, budget.View, id);
            budget.ImageBytes -= image.Bytes.Length;
            if (image.Mime is not ("image/png" or "image/jpeg" or "image/gif" or "image/webp" or "image/bmp"))
                return Error(new ArgumentException("Unsupported image MIME type."), budget, id);
            return new { id, kind = "image", mime = image.Mime, src = $"data:{image.Mime};base64,{Convert.ToBase64String(image.Bytes)}", name = image.Name };
        }
        if (value is LiveValue live)
        {
            if (!path.Add(live)) return new { id, kind = "string", v = "<cycle>" };
            try
            {
                var target = Id();
                var owner = budget.View;
                var current = new View();
                views.Add(owner);
                owner.Children.Add(current);
                liveViews.Add(target, (owner, current));
                Action<object?> handler = next => Update?.Invoke(target, next);
                live.Changed += handler;
                owner.Subscriptions.Add((live, handler, target));
                var node = ViewNode(live.Value, current, depth + 1, budget, path, target);
                return new { id, kind = "live", node };
            }
            finally { path.Remove(live); }
        }
        if (value is DatabaseRows databaseRows) return DatabaseTable(databaseRows, 0, PageSize, depth, budget, path, id);
        if (value is PropertyValue) return Lazy(value, budget.View, id);
        if (!path.Add(value)) return new { id, kind = "string", v = "<cycle>", type = value.GetType().ToString() };
        try
        {
            if (value is IEnumerable enumerable) return Table(enumerable, 0, PageSize, depth, budget, path, id);
            return Object(value, 0, PageSize, depth, budget, path, id);
        }
        catch (Exception error) { return Error(error, budget, id); }
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

    private PropertyValue Property(object value, MemberInfo member, View view)
    {
        var properties = view.Properties;
        if (!properties.TryGetValue(value, out var members)) properties.Add(value, members = []);
        if (!members.TryGetValue(member, out var property)) members.Add(member, property = new PropertyValue(() => Read(member, value)));
        return property;
    }

    private object Object(object value, int offset, int limit, int depth, Budget budget, HashSet<object> path, string? nodeId = null)
    {
        var members = Members(value.GetType());
        var fields = new List<object>();
        foreach (var member in members.Skip(offset).Take(limit))
        {
            if (budget.Remaining <= 1 && fields.Count > 0) break;
            fields.Add(new
            {
                name = member.Name,
                value = Node(member is PropertyInfo ? Property(value, member, budget.View) : Read(member, value), depth + 1, budget, path)
            });
        }
        var end = Math.Min(offset + fields.Count, members.Length);
        return new { id = nodeId ?? Id(), kind = "object", type = value.GetType().ToString(), fields, total = members.Length, next = end < members.Length ? end.ToString(CultureInfo.InvariantCulture) : null, handle = Handle(value, budget.View), truncated = end < members.Length };
    }

    private static void Close(Sequence state)
    {
        state.Finished = true;
        try { (state.Iterator as IDisposable)?.Dispose(); }
        catch (Exception error) { state.Failure ??= error; }
    }

    private object Table(IEnumerable value, int offset, int limit, int depth, Budget budget, HashSet<object> path, string? nodeId = null)
    {
        var sequences = budget.View.Sequences;
        if (!sequences.TryGetValue(value, out var state))
        {
            if (budget.View.Released) throw new InvalidOperationException("That live preview has been replaced.");
            state = new Sequence(value.GetEnumerator());
            if (budget.View.Released)
            {
                Close(state);
                throw new InvalidOperationException("That live preview has been replaced.");
            }
            sequences.Add(value, state);
            views.Add(budget.View);
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
                        field = member is PropertyInfo ? Property(item, member, budget.View).Read() : Read(member, item);
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
            id = nodeId ?? Id(),
            kind = "table",
            type = value.GetType().ToString(),
            columns = state.Columns.Length == 0 ? new[] { new { name = "Value" } } : state.Columns.Select(member => new { name = member.Name }).ToArray(),
            rows,
            total,
            totalKnown = state.Finished,
            next = end < total ? end.ToString(CultureInfo.InvariantCulture) : null,
            handle = Handle(value, budget.View)
        };
    }

    private object DatabaseTable(DatabaseRows value, int offset, int limit, int depth, Budget budget, HashSet<object> path, string? nodeId = null)
    {
        var rows = value.Rows.Skip(offset).Take(limit)
            .Select(row => row.Select(cell => Node(cell, depth + 1, budget, path, cell: true)).ToArray()).ToArray();
        var end = offset + rows.Length;
        return new {
            id = nodeId ?? Id(), kind = "table", type = nameof(DatabaseRows),
            columns = value.Columns.Select(name => new { name }).ToArray(), rows,
            total = value.Rows.Count, totalKnown = true, truncated = value.Truncated,
            next = end < value.Rows.Count ? end.ToString(CultureInfo.InvariantCulture) : null,
            handle = Handle(value, budget.View)
        };
    }

    private static bool IsTextual(object value) =>
        value is string or char or Enum or DateTime or DateTimeOffset or TimeSpan or Guid or double or float ||
        value.GetType().FullName is "System.Net.IPAddress" or "System.Net.NetworkInformation.PhysicalAddress" or "NpgsqlTypes.NpgsqlCidr" or "NpgsqlTypes.NpgsqlInterval" or "NodaTime.Period";

    private static bool IsScalar(object value) => value is string or char or bool or byte or sbyte or short or ushort or int or uint or long or ulong or decimal or double or float or Enum or DateTime or DateTimeOffset or TimeSpan or Guid || IsTextual(value);

    public bool Contains(string handle) => handles.ContainsKey(handle);
    public object Expand(string handle, string? cursor, int limit)
    {
        var (value, view) = handles[handle];
        var budget = new Budget(view);
        var path = new HashSet<object>(ReferenceEqualityComparer.Instance);
        var offset = cursor is null ? 0 : int.Parse(cursor, CultureInfo.InvariantCulture);
        if (offset < 0) throw new ArgumentException("Cursor must be a nonnegative offset.");
        limit = Math.Clamp(limit, 1, PageSize);
        if (value is PropertyValue property) return Node(property.Read(), 0, budget, path);
        if (value is DatabaseRows rows) return DatabaseTable(rows, offset, limit, 0, budget, path);
        if (value is IEnumerable enumerable && value is not string)
            return Table(enumerable, offset, limit, 0, budget, new HashSet<object>(ReferenceEqualityComparer.Instance) { value });
        if (cursor is null) return Node(value, 0, budget, path);
        return Object(value, offset, limit, 0, budget, new HashSet<object>(ReferenceEqualityComparer.Instance) { value });
    }

    public object Error(Exception exception) => Error(exception, new Budget(new View()));

    private object Error(Exception exception, Budget budget, string? nodeId = null)
    {
        while (exception is TargetInvocationException { InnerException: { } inner }) exception = inner;
        var message = Bounded(exception.Message, budget);
        var frames = new StackTrace(exception, true).GetFrames()
            .Where(frame => frame.GetFileName() == "script.cs").Take(Math.Max(0, Math.Min(100, budget.Remaining)))
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
        return new { id = nodeId ?? Id(), kind = "error", type = exception.GetType().Name, message, traceback = frames, causes };
    }

    private static string Bounded(string text, Budget budget)
    {
        var length = Math.Min(text.Length, Math.Min(10000, Math.Max(0, budget.Characters)));
        budget.Characters -= length;
        return text[..length];
    }

    // A live value's contents own a separate view from its containing dump.
    // Replacing it must not expire sibling fields or independent normal dumps.
    private object ViewNode(object? value, View view, int depth, Budget budget, HashSet<object> path, string? nodeId = null)
    {
        var previous = budget.View;
        budget.View = view;
        try { return Node(value, depth, budget, path, nodeId: nodeId); }
        finally { budget.View = previous; }
    }

    public object? UpdateNode(string target, object? value)
    {
        if (!liveViews.TryGetValue(target, out var entry)) return null;
        var current = new View();
        entry.Owner.Children.Remove(entry.Current);
        entry.Owner.Children.Add(current);
        liveViews[target] = (entry.Owner, current);
        Release(entry.Current);
        // Iterator cleanup can itself trigger another update. Keep the newest.
        if (!IsCurrent(target, current)) return null;
        var node = Node(value, 0, new Budget(current), new HashSet<object>(ReferenceEqualityComparer.Instance));
        return IsCurrent(target, current) ? node : null;
    }

    private bool IsCurrent(string target, View view) =>
        liveViews.TryGetValue(target, out var entry) && ReferenceEquals(entry.Current, view);

    private void Release(View view)
    {
        if (view.Released) return;
        view.Released = true;
        views.Remove(view);
        foreach (var (value, handler, target) in view.Subscriptions)
        {
            value.Changed -= handler;
            liveViews.Remove(target);
        }
        foreach (var child in view.Children.ToArray()) Release(child);
        foreach (var handle in view.Identities.Values) handles.Remove(handle);
        foreach (var state in view.Sequences.Values.ToArray())
            if (!state.Finished) Close(state);
        view.Subscriptions.Clear();
        view.Children.Clear();
        view.Identities.Clear();
        view.Sequences.Clear();
        view.Properties.Clear();
    }

    public void Dispose()
    {
        Update = null;
        foreach (var view in views.ToArray()) Release(view);
        views.Clear();
        liveViews.Clear();
        handles.Clear();
    }
}
