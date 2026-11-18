// Objects and nested values

var author = new
{
    Name = "Ada Lovelace",
    Languages = new[] { "C#", "Python", "SQL" },
    Scores = new Dictionary<string, int> { ["first"] = 10, ["second"] = 20 },
};

author.Dump();

new
{
    Name = "Grace Hopper",
    Scores = new[] { 90, 95, 100 },
}.Dump();
