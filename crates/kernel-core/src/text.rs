//! Readable text for the dump wire format, used by `cpad run` (D28).
//!
//! The desktop renders the same nodes as expandable trees and virtualised
//! tables. Terminal output keeps the shape recognisable in plain text: compact
//! scalars and short collections stay on one line, while long collections,
//! tables and tracebacks use one line per entry. A page larger than the kernel
//! sent is marked with its total, so text output never pretends to be complete.

use crate::dump::{as_text, has_more, kind, primitive, row_count, MAX_DEPTH};
use serde_json::Value;

/// Longest compact rendering kept on one line.
const INLINE_LIMIT: usize = 72;

/// Render one dump node as plain text, without a trailing newline.
///
/// Tables show the page the kernel sent plus a row count; use the desktop or a
/// CSV export for every row of a large table.
pub fn render_dump(node: &Value) -> String {
    let mut out = String::new();
    render(node, 0, &mut out);
    out
}

fn string_node(node: &Value) -> String {
    let text = as_text(node, "v").unwrap_or("");
    let mut quoted = Value::String(text.to_owned()).to_string();
    if node.get("truncated").and_then(Value::as_bool) == Some(true) {
        match node.get("total").and_then(Value::as_u64) {
            Some(total) => quoted.push_str(&format!(" (truncated, {total} chars)")),
            None => quoted.push_str(" (truncated)"),
        }
    }
    quoted
}

fn image_text(node: &Value) -> String {
    let mime = as_text(node, "mime").unwrap_or("image");
    match as_text(node, "name") {
        Some(name) => format!("[image {name}, {mime}]"),
        None => format!("[image, {mime}]"),
    }
}

/// One-line form, or `None` when the value needs a block.
fn inline(node: &Value, depth: usize) -> Option<String> {
    if depth > MAX_DEPTH {
        return Some("…".into());
    }
    match kind(node) {
        "null" => Some("None".into()),
        "primitive" => Some(primitive(node)),
        "string" => Some(string_node(node)),
        "lazy" => Some(as_text(node, "preview").unwrap_or("<lazy>").to_owned()),
        "image" => Some(image_text(node)),
        "html" => {
            let markup = as_text(node, "html").unwrap_or("");
            (!markup.contains('\n') && markup.chars().count() <= INLINE_LIMIT)
                .then(|| markup.to_owned())
        }
        "live" => inline(node.get("node")?, depth + 1),
        "list" => {
            let items = node.get("items")?.as_array()?;
            if items.is_empty() {
                return Some("[]".into());
            }
            if items.len() > 8 || has_more(node, items.len()) {
                return None;
            }
            let parts: Vec<String> = items
                .iter()
                .map(|item| inline(item, depth + 1))
                .collect::<Option<_>>()?;
            let text = format!("[{}]", parts.join(", "));
            (text.chars().count() <= INLINE_LIMIT).then_some(text)
        }
        "object" => {
            let fields = node.get("fields")?.as_array()?;
            if fields.is_empty() {
                return Some("{}".into());
            }
            if fields.len() > 6 || has_more(node, fields.len()) {
                return None;
            }
            let mut parts = Vec::with_capacity(fields.len());
            for field in fields {
                let name = as_text(field, "name")?;
                let value = inline(field.get("value")?, depth + 1)?;
                parts.push(format!("{name}: {value}"));
            }
            let text = format!("{{{}}}", parts.join(", "));
            (text.chars().count() <= INLINE_LIMIT).then_some(text)
        }
        _ => None,
    }
}

fn indent(depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn render(node: &Value, depth: usize, out: &mut String) {
    if depth > MAX_DEPTH {
        out.push('…');
        return;
    }
    if let Some(text) = inline(node, depth) {
        out.push_str(&text);
        return;
    }
    render_block(node, depth, out);
}

/// Render a value that `inline` rejected, without asking `inline` again.
fn render_block(node: &Value, depth: usize, out: &mut String) {
    match kind(node) {
        "list" => render_list(node, depth, out),
        "object" => render_object(node, depth, out),
        "table" => render_table(node, depth, out),
        "error" => render_error(node, depth, out),
        "html" => out.push_str(as_text(node, "html").unwrap_or("")),
        "live" => match node.get("node") {
            Some(inner) => render(inner, depth, out),
            None => out.push_str("<live>"),
        },
        _ => out.push('…'),
    }
}

fn render_list(node: &Value, depth: usize, out: &mut String) {
    let Some(items) = node.get("items").and_then(Value::as_array) else {
        out.push_str("[]");
        return;
    };
    out.push('[');
    for item in items {
        out.push('\n');
        indent(depth + 1, out);
        render(item, depth + 1, out);
    }
    if has_more(node, items.len()) {
        out.push('\n');
        indent(depth + 1, out);
        match node.get("total").and_then(Value::as_u64) {
            Some(total) => out.push_str(&format!("… ({} of {total})", items.len())),
            None => out.push_str(&format!("… ({} shown)", items.len())),
        }
    }
    out.push('\n');
    indent(depth, out);
    out.push(']');
}

fn render_object(node: &Value, depth: usize, out: &mut String) {
    let Some(fields) = node.get("fields").and_then(Value::as_array) else {
        out.push_str("{}");
        return;
    };
    out.push('{');
    for field in fields {
        out.push('\n');
        indent(depth + 1, out);
        out.push_str(as_text(field, "name").unwrap_or("?"));
        out.push(':');
        match field.get("value") {
            Some(value) => match inline(value, depth + 1) {
                Some(text) => {
                    out.push(' ');
                    out.push_str(&text);
                }
                // Lists and objects keep their bracket beside the label so their
                // entries line up under it; tables and tracebacks read better
                // below the label with every line indented further.
                None if matches!(kind(value), "list" | "object") => {
                    out.push(' ');
                    render_block(value, depth + 1, out);
                }
                None => {
                    out.push('\n');
                    indent(depth + 2, out);
                    render_block(value, depth + 2, out);
                }
            },
            None => out.push_str(" None"),
        }
    }
    if has_more(node, fields.len()) {
        out.push('\n');
        indent(depth + 1, out);
        match node.get("total").and_then(Value::as_u64) {
            Some(total) => out.push_str(&format!("… ({} of {total})", fields.len())),
            None => out.push_str(&format!("… ({} shown)", fields.len())),
        }
    }
    out.push('\n');
    indent(depth, out);
    out.push('}');
}

fn cell_text(node: &Value) -> String {
    let text = match kind(node) {
        "null" => "None".to_owned(),
        "primitive" => primitive(node),
        "string" => as_text(node, "full")
            .or_else(|| as_text(node, "v"))
            .unwrap_or("")
            .to_owned(),
        "lazy" => as_text(node, "preview").unwrap_or("<lazy>").to_owned(),
        "image" => image_text(node),
        "html" => "[html]".to_owned(),
        _ => inline(node, 0).unwrap_or_else(|| format!("<{}>", kind(node))),
    };
    // A table is one line per row, so keep embedded newlines visible.
    text.replace('\n', " ")
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
    let rows: Vec<Vec<String>> = node
        .get("rows")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    row.as_array()
                        .map(|cells| cells.iter().map(cell_text).collect())
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    let width = columns
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let mut widths = vec![0; width];
    for (index, name) in columns.iter().enumerate() {
        widths[index] = name.chars().count();
    }
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }

    let row_line = |cells: &[String]| {
        let mut line = String::new();
        for (index, width) in widths.iter().enumerate() {
            if index > 0 {
                line.push_str("  ");
            }
            let cell = cells.get(index).map(String::as_str).unwrap_or("");
            line.push_str(cell);
            let padding = width.saturating_sub(cell.chars().count());
            for _ in 0..padding {
                line.push(' ');
            }
        }
        line.trim_end().to_owned()
    };

    let headers: Vec<String> = (0..width)
        .map(|index| {
            columns
                .get(index)
                .cloned()
                .unwrap_or_else(|| format!("column {}", index + 1))
        })
        .collect();
    out.push_str(&row_line(&headers));
    out.push('\n');
    indent(depth, out);
    let rule: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    let mut rule_line = String::new();
    for (index, part) in rule.iter().enumerate() {
        if index > 0 {
            rule_line.push_str("  ");
        }
        rule_line.push_str(part);
    }
    out.push_str(&rule_line);
    for row in &rows {
        out.push('\n');
        indent(depth, out);
        out.push_str(&row_line(row));
    }
    out.push('\n');
    indent(depth, out);
    out.push_str(&row_count(node, rows.len()));
}

fn render_error(node: &Value, depth: usize, out: &mut String) {
    out.push_str(as_text(node, "type").unwrap_or("Error"));
    out.push_str(": ");
    out.push_str(as_text(node, "message").unwrap_or(""));
    if let Some(causes) = node.get("causes").and_then(Value::as_array) {
        for cause in causes {
            out.push('\n');
            indent(depth, out);
            out.push_str("  Caused by ");
            out.push_str(as_text(cause, "type").unwrap_or("Error"));
            out.push_str(": ");
            out.push_str(as_text(cause, "message").unwrap_or(""));
        }
    }
    let frames = node
        .get("traceback")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for frame in frames {
        out.push('\n');
        indent(depth, out);
        out.push_str(&format!(
            "  File \"{}\", line {}",
            as_text(frame, "file").unwrap_or("?"),
            frame.get("line").and_then(Value::as_u64).unwrap_or(0),
        ));
        if let Some(function) = as_text(frame, "function").filter(|text| !text.is_empty()) {
            out.push_str(&format!(", in {function}"));
        }
        if let Some(text) = as_text(frame, "text").filter(|text| !text.trim().is_empty()) {
            out.push('\n');
            indent(depth, out);
            out.push_str("    ");
            out.push_str(text.trim());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::render_dump;
    use serde_json::json;

    #[test]
    fn scalars_and_short_collections_stay_compact() {
        assert_eq!(render_dump(&json!({"kind":"null"})), "None");
        assert_eq!(render_dump(&json!({"kind":"primitive","v":true})), "True");
        assert_eq!(render_dump(&json!({"kind":"primitive","v":42})), "42");
        assert_eq!(render_dump(&json!({"kind":"string","v":"Ada"})), "\"Ada\"");
        assert_eq!(
            render_dump(&json!({
                "kind":"list",
                "type":"list",
                "items":[{"kind":"primitive","v":1},{"kind":"primitive","v":2}],
                "total":2,
                "next":null,
                "truncated":false
            })),
            "[1, 2]"
        );
        assert_eq!(
            render_dump(&json!({
                "kind":"object",
                "type":"dict",
                "fields":[
                    {"name":"name","value":{"kind":"string","v":"Ada"}},
                    {"name":"score","value":{"kind":"primitive","v":10}}
                ],
                "total":2,
                "next":null,
                "truncated":false
            })),
            "{name: \"Ada\", score: 10}"
        );
    }

    #[test]
    fn long_collections_use_one_entry_per_line() {
        let items: Vec<_> = (0..9).map(|v| json!({"kind":"primitive","v":v})).collect();
        let text = render_dump(&json!({
            "kind":"list","type":"list","items":items,"total":9,"next":null
        }));
        assert!(text.starts_with("[\n  0\n  1"));
        assert!(text.ends_with("\n  8\n]"));
    }

    #[test]
    fn paged_collections_show_the_total() {
        let items: Vec<_> = (0..3).map(|v| json!({"kind":"primitive","v":v})).collect();
        let text = render_dump(&json!({
            "kind":"list","type":"list","items":items,"total":100,"next":"3"
        }));
        assert!(text.contains("… (3 of 100)"));
    }

    #[test]
    fn tables_are_aligned_and_report_their_size() {
        let text = render_dump(&json!({
            "kind":"table",
            "type":"list",
            "columns":[{"name":"name"},{"name":"score"}],
            "rows":[
                [{"kind":"string","v":"Ada"},{"kind":"primitive","v":10}],
                [{"kind":"string","v":"Grace"},{"kind":"primitive","v":200}]
            ],
            "total":2,
            "next":null,
            "truncated":false
        }));
        assert_eq!(
            text,
            "name   score\n-----  -----\nAda    10\nGrace  200\n(2 rows)"
        );
    }

    #[test]
    fn paged_tables_say_how_many_rows_are_shown() {
        let text = render_dump(&json!({
            "kind":"table",
            "type":"list",
            "columns":[{"name":"value"}],
            "rows":[[{"kind":"primitive","v":1}]],
            "total":1000,
            "next":"1",
            "truncated":false
        }));
        assert!(text.contains("(1 of 1000 rows)"));
    }

    #[test]
    fn csharp_unknown_totals_use_a_plus_marker() {
        let text = render_dump(&json!({
            "kind":"table",
            "type":"IEnumerable",
            "columns":[{"name":"Value"}],
            "rows":[[{"kind":"primitive","v":1}], [{"kind":"primitive","v":2}]],
            "total":3,
            "totalKnown":false,
            "next":"2",
            "truncated":false
        }));
        assert!(text.contains("(2+ rows)"));
    }

    #[test]
    fn errors_show_message_causes_and_frames() {
        let text = render_dump(&json!({
            "kind":"error",
            "type":"ValueError",
            "message":"bad value",
            "causes":[{"type":"TypeError","message":"not a number"}],
            "traceback":[
                {"file":"script.py","line":3,"column":1,"function":"<module>","text":"raise ValueError(\"bad value\")","script":true},
                {"file":"library.py","line":9,"column":2,"function":"parse","text":"","script":false}
            ]
        }));
        assert_eq!(
            text,
            "ValueError: bad value\n  Caused by TypeError: not a number\n  File \"script.py\", line 3, in <module>\n    raise ValueError(\"bad value\")\n  File \"library.py\", line 9, in parse"
        );
    }

    #[test]
    fn rich_values_keep_their_text_meaning() {
        assert_eq!(
            render_dump(&json!({"kind":"html","html":"<b>hi</b>"})),
            "<b>hi</b>"
        );
        assert_eq!(
            render_dump(&json!({
                "kind":"live",
                "node":{"kind":"primitive","v":7}
            })),
            "7"
        );
        assert_eq!(
            render_dump(&json!({
                "kind":"image","mime":"image/png","name":"plot.png","src":"data:image/png;base64,AA=="
            })),
            "[image plot.png, image/png]"
        );
        assert_eq!(
            render_dump(&json!({
                "kind":"lazy","type":"list","preview":"list (100 items)","handle":"h1"
            })),
            "list (100 items)"
        );
    }

    #[test]
    fn nested_blocks_are_indented() {
        let items: Vec<_> = (0..9).map(|v| json!({"kind":"primitive","v":v})).collect();
        let text = render_dump(&json!({
            "kind":"object",
            "type":"dict",
            "fields":[
                {"name":"numbers","value":{
                    "kind":"list","type":"list",
                    "items":items,
                    "total":9,
                    "next":null
                }}
            ],
            "total":1,
            "next":null
        }));
        assert!(
            text.starts_with("{\n  numbers: [\n    0\n    1\n"),
            "{text:?}"
        );
        assert!(text.ends_with("\n  ]\n}"), "{text:?}");
    }
}
