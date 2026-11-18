//! Standalone HTML for the dump wire format, used by `cpad run --html` (D28).
//!
//! `render_dump` turns one node into semantic markup: tables stay tables, object
//! fields become a description list and tracebacks keep their frames. Every
//! value is escaped except `html` nodes, whose markup is the script's output.
//! The page carries a strict Content Security Policy, and navigation tags are
//! stripped from `html` nodes so a saved report cannot redirect the browser or
//! submit a form.

use crate::dump::{as_text, has_more, kind, primitive, row_count, MAX_DEPTH};
use serde_json::Value;
use std::borrow::Cow;

const PAGE_STYLE: &str = "\
:root { color-scheme: light dark; }
* { box-sizing: border-box; }
body { margin: 0; font: 15px/1.55 system-ui, -apple-system, \"Segoe UI\", sans-serif; }
header, main { max-width: 72rem; margin: 0 auto; padding: 0 1.5rem; }
header { padding-top: 2rem; }
header h1 { margin: 0; font-size: 1.35rem; overflow-wrap: anywhere; }
header .meta { margin: .25rem 0 0; opacity: .65; font-size: .85rem; }
main { padding-bottom: 3rem; }
pre.stream { margin: .75rem 0; padding: .7rem 1rem; border-radius: .45rem;
  background: rgba(127, 127, 127, .12); white-space: pre-wrap; overflow-wrap: anywhere;
  font: 13px/1.45 ui-monospace, SFMono-Regular, Consolas, monospace; }
pre.stderr { border-left: 4px solid #c0392b; }
section.dump { margin: .9rem 0; padding: .7rem 1rem; border: 1px solid rgba(127, 127, 127, .35);
  border-radius: .45rem; overflow-x: auto; }
ul.list, ol.traceback { margin: .2rem 0 .2rem 1.1rem; padding: 0; }
ul.list li, ol.traceback li { margin: .1rem 0; }
dl.object { display: grid; grid-template-columns: max-content 1fr; gap: .1rem .75rem; margin: .2rem 0; }
dl.object dt { font-weight: 600; }
dl.object dd { margin: 0; min-width: 0; }
.table { overflow-x: auto; margin: .3rem 0; }
table { border-collapse: collapse; font-size: .95em; }
th, td { border: 1px solid rgba(127, 127, 127, .35); padding: .25rem .55rem;
  text-align: left; vertical-align: top; }
th { background: rgba(127, 127, 127, .12); }
.table .rows, .more { margin: .3rem 0 0; opacity: .7; font-size: .85rem; }
.html { margin: .3rem 0; }
.html img { max-width: 100%; }
figure.image { margin: .3rem 0; }
figure.image img { max-width: 100%; max-height: 30rem; }
figure.image figcaption { opacity: .7; font-size: .85rem; }
.live .badge { display: inline-block; margin-right: .4rem; padding: 0 .35rem;
  border-radius: .25rem; background: #2b7a4b; color: #fff; font-size: .7em;
  letter-spacing: .06em; text-transform: uppercase; vertical-align: middle; }
.null, .lazy, .unknown { opacity: .75; font-style: italic; }
.string { color: #a31515; }
@media (prefers-color-scheme: dark) { .string { color: #ce9178; } }
.error { border-left: 4px solid #c0392b; padding-left: .8rem; }
.error .error-title { margin: 0; }
.error .causes { margin: .25rem 0; padding-left: 1.1rem; opacity: .85; }
.traceback .frame code { font-size: .92em; }
.traceback pre { margin: .15rem 0; font: 12.5px/1.4 ui-monospace, Consolas, monospace;
  white-space: pre-wrap; overflow-wrap: anywhere; }
";

/// Escape text for use in HTML content and quoted attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(character),
        }
    }
    out
}

/// Remove `<meta>` and `<base>` tags from script-provided markup.
///
/// A `<meta http-equiv="refresh">` navigates the whole standalone page, which
/// the Content Security Policy cannot stop; a `<base>` tag would rewrite every
/// relative link. Scripts, frames and forms are already blocked by the policy.
fn strip_navigation_markup(markup: &str) -> Cow<'_, str> {
    let lower = markup.to_ascii_lowercase();
    let mut out = String::new();
    let mut copied = 0;
    let mut index = 0;
    while let Some(offset) = lower[index..].find('<') {
        let start = index + offset;
        let mut name = lower[start + 1..].trim_start();
        if let Some(rest) = name.strip_prefix('/') {
            name = rest.trim_start();
        }
        let name_len = name.chars().take_while(char::is_ascii_alphabetic).count();
        let navigation = matches!(&name[..name_len], "meta" | "base");
        if navigation {
            match markup[start..].find('>') {
                Some(end) => {
                    out.push_str(&markup[copied..start]);
                    copied = start + end + 1;
                    index = copied;
                    continue;
                }
                None => {
                    // An unclosed tag is completed by the browser with the
                    // wrapper markup that follows, so drop the rest of the
                    // script's markup rather than letting it become a tag.
                    out.push_str(&markup[copied..start]);
                    copied = markup.len();
                    break;
                }
            }
        }
        index = start + 1;
    }
    if copied == 0 {
        return Cow::Borrowed(markup);
    }
    out.push_str(&markup[copied..]);
    Cow::Owned(out)
}

/// Wrap rendered blocks in a complete, self-contained page.
pub fn document(title: &str, body: &str) -> String {
    let title = escape(title);
    format!(
        "<!doctype html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta http-equiv=\"Content-Security-Policy\" \
         content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'; \
         form-action 'none'; base-uri 'none'\">\n\
         <title>{title}</title>\n\
         <style>\n{PAGE_STYLE}</style>\n\
         </head>\n\
         <body>\n\
         <header>\n<h1>{title}</h1>\n<p class=\"meta\">Generated by cpad run</p>\n</header>\n\
         <main>\n{body}</main>\n\
         </body>\n\
         </html>\n"
    )
}

/// Render one printed stream block.
pub fn stream_block(stream: &str, text: &str) -> String {
    let class = if stream == "stderr" {
        "stderr"
    } else {
        "stdout"
    };
    format!("<pre class=\"stream {class}\">{}</pre>\n", escape(text))
}

/// Render one dump node without a trailing newline.
pub fn render_dump(node: &Value) -> String {
    let mut out = String::new();
    render(node, 0, &mut out);
    out
}

fn more_text(node: &Value, shown: usize) -> String {
    match node.get("total").and_then(Value::as_u64) {
        Some(total) => format!("… ({shown} of {total})"),
        None => format!("… ({shown} shown)"),
    }
}

fn render(node: &Value, depth: usize, out: &mut String) {
    if depth > MAX_DEPTH {
        out.push_str("<span class=\"unknown\">…</span>");
        return;
    }
    match kind(node) {
        "null" => out.push_str("<span class=\"null\">None</span>"),
        "primitive" => {
            let class = match node.get("v") {
                Some(Value::Bool(_)) => "bool",
                Some(Value::Number(_)) => "number",
                _ => "value",
            };
            out.push_str(&format!(
                "<span class=\"{class}\">{}</span>",
                escape(&primitive(node))
            ));
        }
        "string" => {
            let text = as_text(node, "v").unwrap_or("");
            let mut quoted = Value::String(text.to_owned()).to_string();
            if node.get("truncated").and_then(Value::as_bool) == Some(true) {
                match node.get("total").and_then(Value::as_u64) {
                    Some(total) => quoted.push_str(&format!(" (truncated, {total} chars)")),
                    None => quoted.push_str(" (truncated)"),
                }
            }
            out.push_str(&format!(
                "<span class=\"string\">{}</span>",
                escape(&quoted)
            ));
        }
        "lazy" => out.push_str(&format!(
            "<span class=\"lazy\" title=\"not expanded\">{}</span>",
            escape(as_text(node, "preview").unwrap_or("<lazy>"))
        )),
        "image" => render_image(node, out),
        "html" => {
            out.push_str("<div class=\"html\">");
            out.push_str(&strip_navigation_markup(
                as_text(node, "html").unwrap_or(""),
            ));
            out.push_str("</div>");
        }
        "live" => {
            out.push_str("<div class=\"live\"><span class=\"badge\">live</span>");
            match node.get("node") {
                Some(inner) => render(inner, depth + 1, out),
                None => out.push_str("<span class=\"lazy\">&lt;live&gt;</span>"),
            }
            out.push_str("</div>");
        }
        "list" => render_list(node, depth, out),
        "object" => render_object(node, depth, out),
        "table" => render_table(node, depth, out),
        "error" => render_error(node, out),
        other => out.push_str(&format!(
            "<span class=\"unknown\">&lt;{}&gt;</span>",
            escape(other)
        )),
    }
}

fn render_image(node: &Value, out: &mut String) {
    let mime = as_text(node, "mime").unwrap_or("image");
    let name = as_text(node, "name");
    let alt = name.unwrap_or(mime);
    out.push_str("<figure class=\"image\">");
    match as_text(node, "src") {
        Some(src) => out.push_str(&format!(
            "<img src=\"{}\" alt=\"{}\">",
            escape(src),
            escape(alt)
        )),
        None => out.push_str(&format!("<span class=\"lazy\">{}</span>", escape(alt))),
    }
    out.push_str(&format!("<figcaption>{}</figcaption>", escape(alt)));
    out.push_str("</figure>");
}

fn render_list(node: &Value, depth: usize, out: &mut String) {
    let items = node
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    out.push_str("<ul class=\"list\">");
    for item in items {
        out.push_str("<li>");
        render(item, depth + 1, out);
        out.push_str("</li>");
    }
    if has_more(node, items.len()) {
        out.push_str(&format!(
            "<li class=\"more\">{}</li>",
            escape(&more_text(node, items.len()))
        ));
    }
    out.push_str("</ul>");
}

fn render_object(node: &Value, depth: usize, out: &mut String) {
    let fields = node
        .get("fields")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    out.push_str("<dl class=\"object\">");
    for field in fields {
        out.push_str(&format!(
            "<dt>{}</dt><dd>",
            escape(as_text(field, "name").unwrap_or("?"))
        ));
        match field.get("value") {
            Some(value) => render(value, depth + 1, out),
            None => out.push_str("<span class=\"null\">None</span>"),
        }
        out.push_str("</dd>");
    }
    out.push_str("</dl>");
    if has_more(node, fields.len()) {
        out.push_str(&format!(
            "<p class=\"more\">{}</p>",
            escape(&more_text(node, fields.len()))
        ));
    }
}

fn render_table(node: &Value, depth: usize, out: &mut String) {
    let columns: Vec<String> = node
        .get("columns")
        .and_then(Value::as_array)
        .map(|columns| {
            columns
                .iter()
                .map(|column| as_text(column, "name").unwrap_or("?").to_owned())
                .collect()
        })
        .unwrap_or_default();
    let rows = node
        .get("rows")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let width = columns.len().max(
        rows.iter()
            .filter_map(|row| row.as_array().map(Vec::len))
            .max()
            .unwrap_or(0),
    );
    out.push_str("<div class=\"table\"><table><thead><tr>");
    for index in 0..width {
        let name = columns
            .get(index)
            .cloned()
            .unwrap_or_else(|| format!("column {}", index + 1));
        out.push_str(&format!("<th>{}</th>", escape(&name)));
    }
    out.push_str("</tr></thead><tbody>");
    for row in rows {
        out.push_str("<tr>");
        let cells = row.as_array().map(Vec::as_slice).unwrap_or_default();
        for index in 0..width {
            out.push_str("<td>");
            if let Some(cell) = cells.get(index) {
                render(cell, depth + 1, out);
            }
            out.push_str("</td>");
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table>");
    out.push_str(&format!(
        "<p class=\"rows\">{}</p></div>",
        escape(&row_count(node, rows.len()))
    ));
}

fn render_error(node: &Value, out: &mut String) {
    out.push_str("<div class=\"error\">");
    out.push_str(&format!(
        "<p class=\"error-title\"><span class=\"error-type\">{}</span>: \
         <span class=\"error-message\">{}</span></p>",
        escape(as_text(node, "type").unwrap_or("Error")),
        escape(as_text(node, "message").unwrap_or(""))
    ));
    if let Some(causes) = node.get("causes").and_then(Value::as_array) {
        if !causes.is_empty() {
            out.push_str("<ul class=\"causes\">");
            for cause in causes {
                out.push_str(&format!(
                    "<li>Caused by <strong>{}</strong>: {}</li>",
                    escape(as_text(cause, "type").unwrap_or("Error")),
                    escape(as_text(cause, "message").unwrap_or(""))
                ));
            }
            out.push_str("</ul>");
        }
    }
    let frames = node
        .get("traceback")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if !frames.is_empty() {
        out.push_str("<ol class=\"traceback\">");
        for frame in frames {
            out.push_str("<li><span class=\"frame\">File <code>");
            out.push_str(&escape(as_text(frame, "file").unwrap_or("?")));
            out.push_str(&format!(
                "</code>, line {}",
                frame.get("line").and_then(Value::as_u64).unwrap_or(0)
            ));
            if let Some(function) = as_text(frame, "function").filter(|text| !text.is_empty()) {
                out.push_str(", in <code>");
                out.push_str(&escape(function));
                out.push_str("</code>");
            }
            out.push_str("</span>");
            if let Some(text) = as_text(frame, "text").filter(|text| !text.trim().is_empty()) {
                out.push_str(&format!("<pre>{}</pre>", escape(text.trim())));
            }
            out.push_str("</li>");
        }
        out.push_str("</ol>");
    }
    out.push_str("</div>");
}

#[cfg(test)]
mod tests {
    use super::{document, escape, render_dump, stream_block};
    use serde_json::json;

    #[test]
    fn text_is_escaped_and_html_nodes_pass_through() {
        assert_eq!(escape("a < b & \"c\""), "a &lt; b &amp; &quot;c&quot;");
        assert_eq!(
            render_dump(&json!({"kind":"string","v":"<script>alert(1)</script>"})),
            "<span class=\"string\">&quot;&lt;script&gt;alert(1)&lt;/script&gt;&quot;</span>"
        );
        assert_eq!(
            render_dump(&json!({"kind":"html","html":"<b>trusted</b>"})),
            "<div class=\"html\"><b>trusted</b></div>"
        );
    }

    #[test]
    fn navigation_tags_in_html_nodes_are_removed() {
        let html = render_dump(&json!({
            "kind":"html",
            "html":"<meta http-equiv=\"refresh\" content=\"0;url=https://example.com\"><base href=\"https://e/\"><b>ok</b>"
        }));
        assert!(!html.contains("http-equiv"), "{html}");
        assert!(!html.contains("<base"), "{html}");
        assert!(html.contains("<b>ok</b>"), "{html}");
    }

    #[test]
    fn unclosed_navigation_tags_drop_the_rest_of_the_markup() {
        let html = render_dump(&json!({
            "kind":"html",
            "html":"<b>ok</b><meta http-equiv=\"refresh\" content=\"0;url=https://example.com\""
        }));
        assert!(!html.contains("http-equiv"), "{html}");
        assert!(!html.contains("example.com"), "{html}");
        assert!(html.contains("<b>ok</b>"), "{html}");
    }

    #[test]
    fn tables_keep_their_structure_and_row_count() {
        let html = render_dump(&json!({
            "kind":"table",
            "type":"list",
            "columns":[{"name":"name"},{"name":"score"}],
            "rows":[
                [{"kind":"string","v":"Ada"},{"kind":"primitive","v":10}],
                [{"kind":"string","v":"Grace"},{"kind":"primitive","v":20}]
            ],
            "total":2,
            "next":null
        }));
        assert!(html.starts_with("<div class=\"table\"><table>"), "{html}");
        assert!(html.contains("<th>name</th><th>score</th>"), "{html}");
        assert!(
            html.contains("<td><span class=\"string\">&quot;Ada&quot;</span></td>"),
            "{html}"
        );
        assert!(html.contains("(2 rows)"), "{html}");
    }

    #[test]
    fn paged_collections_say_how_many_are_shown() {
        let html = render_dump(&json!({
            "kind":"list",
            "type":"list",
            "items":[{"kind":"primitive","v":1}],
            "total":100,
            "next":"1"
        }));
        assert!(html.contains("… (1 of 100)"), "{html}");
    }

    #[test]
    fn errors_keep_causes_and_frames() {
        let html = render_dump(&json!({
            "kind":"error",
            "type":"ValueError",
            "message":"bad <value>",
            "causes":[{"type":"TypeError","message":"not a number"}],
            "traceback":[{"file":"a<b.py","line":3,"function":"parse","text":"raise ValueError()"}]
        }));
        assert!(html.contains("bad &lt;value&gt;"), "{html}");
        assert!(
            html.contains("Caused by <strong>TypeError</strong>"),
            "{html}"
        );
        assert!(html.contains("a&lt;b.py"), "{html}");
        assert!(html.contains("<pre>raise ValueError()</pre>"), "{html}");
    }

    #[test]
    fn deeply_nested_trees_stop_at_the_depth_limit() {
        let mut node = json!({"kind":"null"});
        for _ in 0..40 {
            node = json!({"kind":"list","items":[node],"total":1});
        }
        let html = render_dump(&node);
        assert!(html.contains("…"), "{html}");
    }

    #[test]
    fn the_page_is_complete_and_carries_a_restrictive_policy() {
        let page = document("hello & <world>", &stream_block("stdout", "hi <there>"));
        assert!(page.starts_with("<!doctype html>"), "{page}");
        assert!(
            page.contains("<title>hello &amp; &lt;world&gt;</title>"),
            "{page}"
        );
        assert!(page.contains("default-src 'none'"), "{page}");
        assert!(page.contains("form-action 'none'"), "{page}");
        assert!(page.contains("base-uri 'none'"), "{page}");
        assert!(
            page.contains("<pre class=\"stream stdout\">hi &lt;there&gt;</pre>"),
            "{page}"
        );
        assert!(page.ends_with("</html>\n"), "{page}");
    }
}
