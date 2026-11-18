// Async work

Console.WriteLine("Loading values...");

var values = await Task.WhenAll(
    Enumerable.Range(1, 5).Select(async index =>
    {
        await Task.Delay(index * 10);
        return index * index;
    })
);

new { Values = values, Total = values.Sum() }.Dump();

// Stop cancels awaited work when the task receives the script's Cancel token.
await Task.Delay(10, Cancel);
