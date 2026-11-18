// Group and aggregate

var products = new[]
{
    new { Name = "Ethiopia Yirgacheffe", Category = "Coffee", Price = 18.50m },
    new { Name = "Colombia Huila", Category = "Coffee", Price = 16.00m },
    new { Name = "Sencha Green", Category = "Tea", Price = 14.00m },
    new { Name = "Earl Grey", Category = "Tea", Price = 12.50m },
    new { Name = "Pour-over Kettle", Category = "Equipment", Price = 54.00m },
};

var byCategory = products
    .GroupBy(product => product.Category)
    .Select(group => new
    {
        Category = group.Key,
        Count = group.Count(),
        Average = group.Average(product => product.Price),
        Cheapest = group.Min(product => product.Name),
    })
    .OrderBy(item => item.Category);

byCategory.Dump();
