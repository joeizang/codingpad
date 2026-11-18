// Tables from sequences

var drinks = new[]
{
    new { Name = "Espresso", Price = 2.50m, Caffeine = 63 },
    new { Name = "Filter coffee", Price = 3.00m, Caffeine = 95 },
    new { Name = "Green tea", Price = 2.80m, Caffeine = 28 },
};

drinks.Dump();

// Sequences are read in pages, so a large table stays responsive.
Enumerable.Range(1, 100)
    .Select(index => new { Index = index, Square = index * index, Root = Math.Sqrt(index) })
    .Dump();
