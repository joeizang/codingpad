namespace CodingPad;

/// <summary>Markup rendered in the results viewer's sandboxed frame.</summary>
public sealed record HtmlValue(string Html);

/// <summary>Image bytes rendered by the results viewer (maximum 4 MiB).</summary>
public sealed record ImageValue(byte[] Bytes, string Mime = "image/png", string? Name = null);

/// <summary>A dumped value that can be replaced during the current run.</summary>
public sealed class LiveValue(object? initialValue)
{
    private object? current = initialValue;
    public event Action<object?>? Changed;
    public object? Value
    {
        get => current;
        set { current = value; Changed?.Invoke(value); }
    }
}
