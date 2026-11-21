use syn::{parse::ParseStream, spanned::Spanned, Attribute, Expr, Item, Stmt};
use unicode_width::UnicodeWidthChar;

pub(crate) const FILE: &str = "src/main.rs";

pub(crate) struct Source {
    pub(super) original: String,
    pub(crate) generated: String,
    pub(super) segments: Vec<(usize, usize, usize)>,
    pub(super) frontmatter: std::ops::Range<usize>,
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
            staged.copy(tail.end, original.len());
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
