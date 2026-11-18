// Cache a slow result

// Cache() keeps a value between runs of this tab. Change the key or press
// Reset to compute it again.
var squares = Cache("squares", () => Enumerable.Range(1, 10).Select(value => value * value).ToArray());

new
{
    Squares = squares,
    Total = squares.Sum(),
}.Dump();

// Failed factories are not cached, so the next run can retry.
Cache("answer", () => 6 * 7).Dump();
