namespace CodingPad;

public static class DumpExtensions
{
    // AsyncLocal keeps late background writes tied to their original run.
    public static readonly AsyncLocal<Action<object?>?> Output = new();

    public static T Dump<T>(this T value)
    {
        Output.Value?.Invoke(value);
        return value;
    }
}
