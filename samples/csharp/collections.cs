// Collections and LINQ

var counts = new Dictionary<string, int>();
foreach (var word in "the quick brown fox jumps over the lazy dog the end".Split(' '))
    counts[word] = counts.GetValueOrDefault(word) + 1;

counts
    .OrderByDescending(pair => pair.Value)
    .ThenBy(pair => pair.Key)
    .Dump();

counts
    .Where(pair => pair.Key.Length > 3)
    .Select(pair => pair.Key.ToUpperInvariant())
    .Dump();
