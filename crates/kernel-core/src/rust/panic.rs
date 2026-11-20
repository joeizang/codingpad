use super::source::{self, Source};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub(super) struct Panic {
    pub(super) message: String,
    file: String,
    line: String,
    column: String,
    trace: String,
}

impl Panic {
    pub(super) fn frames(&self, source: &Source<'_>, name: &str) -> Vec<Value> {
        let mut frames = Vec::new();
        let mut function = "<panic>";
        for entry in self.trace.lines().map(str::trim) {
            if let Some(location) = entry.strip_prefix("at ") {
                if let Some((file, line, column)) = split_location(location) {
                    if let Some(frame) = frame(source, name, function, file, line, column) {
                        frames.push(frame);
                    }
                }
            } else if let Some((ordinal, symbol)) = entry.split_once(": ") {
                if ordinal.trim().parse::<usize>().is_ok() {
                    function = symbol;
                }
            }
        }

        // The panic hook also supplies the location directly. Keep it even
        // when the platform cannot symbolize the stack or the panic came
        // from a library which has no editor buffer.
        let line = self.line.parse().unwrap_or(0);
        let column = self.column.parse().unwrap_or(0);
        if let Some(top) = frame(source, name, "<panic>", &self.file, line, column) {
            // Symbolized return addresses can point past the actual panic
            // site. The hook has the exact span supplied by the compiler.
            if frames
                .first()
                .is_some_and(|first| first["line"] == top["line"])
            {
                frames[0]["line"] = top["line"].clone();
                frames[0]["column"] = top["column"].clone();
            } else {
                frames.insert(0, top);
            }
        } else if line != 0 && !self.file.is_empty() {
            frames.insert(
                0,
                json!({
                    "file": self.file, "line": line, "column": column,
                    "function": "<panic>", "text": "", "script": false,
                }),
            );
        }
        frames.truncate(32);
        frames
    }
}

fn split_location(location: &str) -> Option<(&str, usize, usize)> {
    // Symbolizers can omit columns. Parse only numeric suffixes to preserve
    // Windows drive letters in absolute paths.
    let (prefix, last) = location.rsplit_once(':')?;
    let last = last.parse().ok()?;
    if let Some((file, line)) = prefix.rsplit_once(':') {
        if let Ok(line) = line.parse() {
            return Some((file, line, last));
        }
    }
    Some((prefix, last, 0))
}

fn frame(
    source: &Source<'_>,
    name: &str,
    function: &str,
    file: &str,
    line: usize,
    column: usize,
) -> Option<Value> {
    let file = file.replace('\\', "/");
    if file != source::FILE && !file.ends_with(&format!("/{}", source::FILE)) {
        return None;
    }
    let (line, column) = source.trace_position(line, column)?;
    Some(json!({
        "file": name, "line": line, "column": column, "function": function,
        "text": source.line(line), "script": true,
    }))
}

#[cfg(test)]
mod tests {
    use super::split_location;

    #[test]
    fn parses_windows_backtrace_locations_with_or_without_columns() {
        let path = r"C:\scratch\src\main.rs";
        assert_eq!(split_location(&format!("{path}:8:12")), Some((path, 8, 12)));
        assert_eq!(split_location(&format!("{path}:8")), Some((path, 8, 0)));
    }
}
