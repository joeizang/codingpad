# HTML output

from codingpad import html

summary = """
<h2>Daily summary</h2>
<p><strong>Orders:</strong> 10</p>
<p><strong>Revenue:</strong> &pound;625.00</p>
<ul>
  <li>Coffee</li>
  <li>Tea</li>
  <li>Equipment</li>
</ul>
"""

# HTML is rendered in a sandboxed frame; scripts and network access stay blocked.
dump(html(summary))
