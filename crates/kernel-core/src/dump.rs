//! Shared helpers for reading the dump wire format.
//!
//! The text and HTML renderers must agree on node kinds, scalar spelling and
//! table row counts. Keeping the rules here means a semantic change (such as
//! the C# placeholder-row rule) lands in both renderers at once.

use serde_json::Value;

/// A safety net for malformed input; real trees stop at six levels.
pub(crate) const MAX_DEPTH: usize = 32;

pub(crate) fn kind(node: &Value) -> &str {
    node.get("kind").and_then(Value::as_str).unwrap_or("null")
}

pub(crate) fn as_text<'a>(node: &'a Value, key: &str) -> Option<&'a str> {
    node.get(key).and_then(Value::as_str)
}

pub(crate) fn primitive(node: &Value) -> String {
    match node.get("v") {
        Some(Value::Bool(true)) => "True".into(),
        Some(Value::Bool(false)) => "False".into(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(value) => value.to_string(),
        None => "None".into(),
    }
}

pub(crate) fn has_more(node: &Value, shown: usize) -> bool {
    node.get("next").and_then(Value::as_str).is_some()
        || node
            .get("total")
            .and_then(Value::as_u64)
            .is_some_and(|total| total > shown as u64)
}

/// The row count line shared by text and HTML tables.
pub(crate) fn row_count(node: &Value, shown: usize) -> String {
    let total = node
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or(shown as u64);
    let mut text = if node.get("totalKnown").and_then(Value::as_bool) == Some(false) {
        // C# uses one placeholder row until enumeration finishes.
        let known = if node.get("next").and_then(Value::as_str).is_some() {
            shown
        } else {
            shown.min(total.saturating_sub(1) as usize)
        };
        format!("({known}+ rows)")
    } else if total > shown as u64 {
        format!("({shown} of {total} rows)")
    } else {
        format!("({total} rows)")
    };
    if node.get("truncated").and_then(Value::as_bool) == Some(true) {
        text.push_str(" (results truncated)");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::row_count;
    use serde_json::json;

    #[test]
    fn csharp_unknown_totals_use_a_plus_marker() {
        let text = row_count(&json!({"total": 3, "totalKnown": false, "next": "2"}), 2);
        assert_eq!(text, "(2+ rows)");
    }

    #[test]
    fn truncated_results_are_marked() {
        let text = row_count(
            &json!({"total": 100, "next": "100", "truncated": true}),
            100,
        );
        assert_eq!(text, "(100 rows) (results truncated)");
    }
}
