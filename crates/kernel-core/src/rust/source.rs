use serde_json::{json, Value};
use syn::{parse::ParseStream, spanned::Spanned, Attribute, Expr, Item, Stmt};
use unicode_width::UnicodeWidthChar;

pub(crate) const FILE: &str = "src/main.rs";

pub(crate) struct Source {
    original: String,
    pub(crate) generated: String,
    segments: Vec<(usize, usize, usize)>,
    frontmatter: std::ops::Range<usize>,
}

impl Source {
    pub(super) fn with_frontmatter(
        original: &str,
        frontmatter: std::ops::Range<usize>,
        runtime: &str,
    ) -> Self {
        if frontmatter.is_empty() {
            return Self::build(original, 0, runtime);
        }
        // Keep byte offsets and line breaks so compiler spans still map back to
        // the editor, including offsets after non-ASCII manifest comments.
        let mut masked = original.as_bytes().to_vec();
        for byte in &mut masked[frontmatter.clone()] {
            if !matches!(byte, b'\n' | b'\r') {
                *byte = b' ';
            }
        }
        let masked = String::from_utf8(masked).expect("replacing a full range preserves UTF-8");
        let mut staged = Self::build(&masked, frontmatter.end, runtime);
        staged.original = original.to_owned();
        staged.frontmatter = frontmatter;
        staged
    }

    fn build(original: &str, frontmatter_end: usize, runtime: &str) -> Self {
        let _spans = SpanScope;
        let runtime = runtime.replace('-', "_");
        let prefix = source_prefix_len(original).max(frontmatter_end);
        let mut staged = Self {
            original: original.to_owned(),
            generated: String::new(),
            segments: Vec::new(),
            frontmatter: 0..0,
        };
        // Native Rust programs retain their entry point and crate-level items.
        if let Some(entry) = native_entry(&original[prefix..]).map(|entry| prefix + entry) {
            staged.copy(0, entry);
            staged.generated.push_str(&format!(
                " let __codingpad_panic_guard = ::{runtime}::__start(); "
            ));
            staged.copy(entry, original.len());
            return staged;
        }

        let parse = |input: ParseStream<'_>| {
            let attrs = input.call(Attribute::parse_inner)?;
            let statements = input.call(syn::Block::parse_within)?;
            Ok((attrs, statements))
        };
        let (body, tail) = syn::parse::Parser::parse_str(parse, &original[prefix..])
            .map(|(attrs, statements)| {
                let body = attrs
                    .last()
                    .map_or(prefix, |attr| prefix + attr.span().byte_range().end);
                let tail = match statements.last() {
                    Some(Stmt::Expr(expr, None)) if plain(expr) => {
                        let range = expr.span().byte_range();
                        Some(prefix + range.start..prefix + range.end)
                    }
                    _ => None,
                };
                (body, tail)
            })
            .unwrap_or((prefix, None));
        staged.copy(0, body);
        staged.generated.push_str(&format!(
            "\nfn main() {{ let __codingpad_panic_guard = ::{runtime}::__start();\n"
        ));
        if let Some(tail) = tail {
            staged.copy(body, tail.start);
            staged
                .generated
                .push_str(&format!("::{runtime}::__finish("));
            staged.copy(tail.start, tail.end);
            staged.generated.push_str(");");
            if tail.end < original.len() {
                staged.copy(tail.end, original.len());
            }
        } else {
            staged.copy(body, original.len());
        }
        staged.generated.push_str("\n}\n");
        staged
    }

    fn copy(&mut self, start: usize, end: usize) {
        let destination = self.generated.len();
        self.generated.push_str(&self.original[start..end]);
        self.segments
            .push((destination, destination + end - start, start));
    }

    pub(super) fn original_offset(&self, offset: usize) -> Option<usize> {
        self.segments.iter().find_map(|&(start, end, original)| {
            (start..=end)
                .contains(&offset)
                .then(|| original + offset - start)
        })
    }

    pub(super) fn position(&self, offset: usize) -> Option<(usize, usize)> {
        let prefix = self.original.get(..self.original_offset(offset)?)?;
        let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
        let column = prefix.rsplit('\n').next()?.encode_utf16().count() + 1;
        Some((line, column))
    }

    fn generated_line(&self, line: usize) -> Option<(usize, &str)> {
        let mut lines = self.generated.split_inclusive('\n');
        let offset = (&mut lines)
            .take(line.checked_sub(1)?)
            .map(str::len)
            .sum::<usize>();
        Some((offset, lines.next()?))
    }

    pub(super) fn backtrace_position(&self, line: usize, column: usize) -> Option<(usize, usize)> {
        let (offset, generated) = self.generated_line(line)?;
        let byte = column.saturating_sub(1);
        generated.is_char_boundary(byte).then_some(())?;
        self.position(offset + byte)
    }

    pub(super) fn panic_position(&self, line: usize, column: usize) -> Option<(usize, usize)> {
        let (offset, generated) = self.generated_line(line)?;
        // rustc reports panic locations in display columns, with tabs counted
        // as four spaces. Monaco needs UTF-16 columns in the original buffer.
        let mut display_column = 1;
        for (byte, character) in generated.char_indices() {
            if display_column >= column {
                return self.position(offset + byte);
            }
            display_column += if character == '\t' {
                4
            } else {
                character.width().unwrap_or(0)
            };
        }
        None
    }

    pub(super) fn line(&self, line: usize) -> &str {
        self.original
            .lines()
            .nth(line.saturating_sub(1))
            .unwrap_or_default()
    }
}

fn native_entry(source: &str) -> Option<usize> {
    let file = syn::parse_str::<syn::File>(source).ok()?;
    let main = file.items.iter().find_map(|item| match item {
        Item::Fn(function) if function.sig.ident == "main" => Some(function),
        _ => None,
    })?;
    let entry = main
        .attrs
        .iter()
        .rev()
        .find(|attr| matches!(attr.style, syn::AttrStyle::Inner(_)))
        .map_or_else(
            || main.block.brace_token.span.open().byte_range().end,
            |attr| attr.span().byte_range().end,
        );
    Some(entry)
}

// Rust allows a BOM and a shebang only at the start of the file.
pub(super) fn source_prefix_len(source: &str) -> usize {
    let rest = source.strip_prefix('\u{feff}').unwrap_or(source);
    let bom = source.len() - rest.len();
    if rest
        .strip_prefix("#!")
        .is_some_and(|after| !after.trim_start().starts_with('['))
    {
        return bom + rest.find('\n').unwrap_or(rest.len());
    }
    bom
}

struct SpanScope;

impl Drop for SpanScope {
    fn drop(&mut self) {
        // All spans are local to the synchronous parse. The generated source
        // keeps only byte offsets, so release proc-macro2's thread-local map.
        proc_macro2::extra::invalidate_current_thread_spans();
    }
}

fn plain(expression: &Expr) -> bool {
    !matches!(
        expression,
        Expr::Assign(_)
            | Expr::Break(_)
            | Expr::Continue(_)
            | Expr::ForLoop(_)
            | Expr::Loop(_)
            | Expr::Return(_)
            | Expr::While(_)
    ) && !matches!(expression, Expr::Macro(call) if call.mac.path.segments.last().is_some_and(|segment| {
        matches!(segment.ident.to_string().as_str(), "panic" | "unreachable" | "todo" | "unimplemented")
    }))
}

impl Source {
    pub(crate) fn editor_to_shadow(&self, value: &Value) -> Option<Value> {
        let offset = position_offset(&self.original, value)?;
        if self.frontmatter.contains(&offset) {
            return None;
        }
        let generated = self
            .segments
            .iter()
            .rev()
            .find_map(|&(start, end, original)| {
                let delta = offset.checked_sub(original)?;
                (delta <= end - start).then_some(start + delta)
            })?;
        byte_position(&self.generated, generated)
    }

    fn shadow_to_editor(&self, value: &Value, insertion: bool) -> Option<Value> {
        let offset = position_offset(&self.generated, value)?;
        let mut original = self.original_offset(offset)?;
        if self.frontmatter.contains(&original) {
            if !insertion {
                return None;
            }
            original = self.frontmatter.end;
        }
        byte_position(&self.original, original)
    }

    fn map_range(&self, range: &mut Value, insertion: bool) -> bool {
        let insertion = insertion && range["start"] == range["end"];
        let Some(start) = self.shadow_to_editor(&range["start"], insertion) else {
            return false;
        };
        let Some(end) = self.shadow_to_editor(&range["end"], insertion) else {
            return false;
        };
        *range = json!({ "start": start, "end": end });
        true
    }

    pub(crate) fn map_diagnostics(&self, params: &mut Value, uri: &str) {
        let Some(diagnostics) = params["diagnostics"].as_array_mut() else {
            return;
        };
        diagnostics.retain_mut(|diagnostic| {
            if !self.map_range(&mut diagnostic["range"], false) {
                // Real compiler errors can point at generated wrapper code.
                // Give them a script location; omit analyzer-only wrapper diagnostics.
                if diagnostic["source"] != "rustc" || diagnostic["severity"] != 1 {
                    return false;
                }
                let Some(position) = byte_position(&self.original, self.frontmatter.end) else {
                    return false;
                };
                diagnostic["range"] = json!({ "start": position, "end": position });
            }
            if let Some(related) = diagnostic["relatedInformation"].as_array_mut() {
                related.retain_mut(|info| {
                    let location = &mut info["location"];
                    location["uri"] != uri || self.map_range(&mut location["range"], false)
                });
            }
            true
        });
    }

    pub(crate) fn map_response(&self, method: &str, response: &mut Value) {
        match method {
            "textDocument/completion" => {
                let mut default_edit = response
                    .get_mut("itemDefaults")
                    .and_then(Value::as_object_mut)
                    .and_then(|defaults| defaults.remove("editRange"));
                let valid_default = default_edit.as_mut().is_none_or(|range| {
                    if range.get("insert").is_some() {
                        self.map_range(&mut range["insert"], false)
                            && self.map_range(&mut range["replace"], false)
                    } else {
                        self.map_range(range, false)
                    }
                });
                let items = if response.is_array() {
                    response.as_array_mut()
                } else {
                    response.get_mut("items").and_then(Value::as_array_mut)
                };
                if let Some(items) = items {
                    items.retain_mut(|item| {
                        let valid = match item.get_mut("textEdit") {
                            Some(edit) if edit.get("range").is_some() => {
                                self.map_range(&mut edit["range"], false)
                            }
                            Some(edit) => {
                                self.map_range(&mut edit["insert"], false)
                                    && self.map_range(&mut edit["replace"], false)
                            }
                            None if !valid_default => false,
                            None => {
                                if let Some(range) = &default_edit {
                                    let text = item["textEditText"]
                                        .as_str()
                                        .or_else(|| item["insertText"].as_str())
                                        .unwrap_or_else(|| {
                                            item["label"].as_str().unwrap_or_default()
                                        });
                                    item["textEdit"] = if range.get("insert").is_some() {
                                        json!({
                                            "newText": text,
                                            "insert": range["insert"],
                                            "replace": range["replace"]
                                        })
                                    } else {
                                        json!({ "newText": text, "range": range })
                                    };
                                }
                                true
                            }
                        };
                        self.map_additional_edits(item);
                        valid
                    });
                }
            }
            "completionItem/resolve" => self.map_additional_edits(response),
            "textDocument/hover" => {
                if let Some(range) = response.get_mut("range") {
                    if !self.map_range(range, false) {
                        response.as_object_mut().unwrap().remove("range");
                    }
                }
            }
            _ => {}
        }
    }

    fn map_additional_edits(&self, item: &mut Value) {
        if let Some(edits) = item
            .get_mut("additionalTextEdits")
            .and_then(Value::as_array_mut)
        {
            edits.retain_mut(|edit| self.map_range(&mut edit["range"], true));
        }
    }
}

fn position_offset(text: &str, position: &Value) -> Option<usize> {
    let line = usize::try_from(position["line"].as_u64()?).ok()?;
    let character = usize::try_from(position["character"].as_u64()?).ok()?;
    let mut lines = text.split('\n');
    let offset: usize = lines.by_ref().take(line).map(|line| line.len() + 1).sum();
    let line = lines.next()?.trim_end_matches('\r');
    let mut utf16 = 0;
    for (index, ch) in line.char_indices() {
        if utf16 == character {
            return Some(offset + index);
        }
        utf16 += ch.len_utf16();
    }
    (utf16 == character).then_some(offset + line.len())
}

fn byte_position(text: &str, offset: usize) -> Option<Value> {
    let prefix = text.get(..offset)?;
    Some(json!({
        "line": prefix.bytes().filter(|&byte| byte == b'\n').count(),
        "character": prefix.rsplit('\n').next()?.encode_utf16().count(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_attributes_and_wraps_only_the_unterminated_expression() {
        let original = "#![allow(unused)]\nlet café = \"☕\";\ncafé.len() // dump\n";
        let staged = Source::with_frontmatter(original, 0..0, "codingpad");
        assert!(staged.generated.starts_with("#![allow(unused)]\nfn main()"));
        let expr = "::codingpad::__finish(café.len()); // dump";
        assert!(staged.generated.contains(expr), "{}", staged.generated);
        let offset = staged.generated.find("café.len").unwrap();
        assert_eq!(staged.position(offset), Some((3, 1)));
        assert!(!Source::with_frontmatter("42;", 0..0, "codingpad")
            .generated
            .contains("__finish"));
    }

    #[test]
    fn completion_at_eof_stays_inside_the_automatic_dump() {
        let original = "codingpad::du";
        let source = Source::with_frontmatter(original, 0..0, "codingpad");
        let mapped = source
            .editor_to_shadow(&serde_json::json!({"line": 0, "character": original.len()}))
            .unwrap();
        let line = source
            .generated
            .lines()
            .nth(mapped["line"].as_u64().unwrap() as usize)
            .unwrap();
        assert_eq!(
            &line[mapped["character"].as_u64().unwrap() as usize..],
            ");"
        );
    }

    #[test]
    fn completion_list_defaults_become_script_text_edits() {
        let source = Source::with_frontmatter("let _ = codingpad::du;", 0..0, "codingpad");
        let start = source
            .editor_to_shadow(&json!({ "line": 0, "character": 19 }))
            .unwrap();
        let end = source
            .editor_to_shadow(&json!({ "line": 0, "character": 21 }))
            .unwrap();
        let mut response = json!({
            "itemDefaults": { "editRange": { "start": start, "end": end } },
            "items": [{ "label": "dump(…)", "textEditText": "dump" }],
        });
        source.map_response("textDocument/completion", &mut response);
        assert_eq!(
            response["items"][0]["textEdit"],
            json!({
                "newText": "dump",
                "range": {
                    "start": { "line": 0, "character": 19 },
                    "end": { "line": 0, "character": 21 },
                }
            })
        );
    }

    #[test]
    fn adds_panic_capture_after_an_explicit_mains_inner_attributes() {
        let source = "#!/usr/bin/env rust-script\n#![allow(unused)]\nfn main() { #![allow(unused)] println!(\"hello\"); }";
        let staged = Source::with_frontmatter(source, 0..0, "codingpad");
        assert!(staged
            .generated
            .contains("{ #![allow(unused)] let __codingpad_panic_guard = ::codingpad::__start();"));
        assert_eq!(
            staged.generated.replacen(
                " let __codingpad_panic_guard = ::codingpad::__start(); ",
                "",
                1
            ),
            source
        );
    }
}
