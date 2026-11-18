// Live values and HTML output

var progress = new LiveValue("starting");
progress.Dump();

new HtmlValue(
    "<h2>Daily summary</h2>"
    + "<p><strong>Orders:</strong> 10</p>"
    + "<p><strong>Revenue:</strong> &pound;412.50</p>"
    + "<ul><li>Coffee</li><li>Tea</li><li>Equipment</li></ul>"
).Dump();

for (var step = 1; step <= 3; step++)
{
    progress.Value = $"step {step} of 3";
    await Task.Delay(50);
}
