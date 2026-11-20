use syn::{parse::ParseStream, spanned::Spanned, Attribute, Expr, Item, Stmt};
use unicode_width::UnicodeWidthChar;

pub(super) const FILE: &str = "src/main.rs";

pub(super) struct Source<'a> {
    original: &'a str,
    pub(super) generated: String,
    segments: Vec<(usize, usize, usize)>,
}

impl<'a> Source<'a> {
    pub(super) fn new(original: &'a str) -> Self {
        let mut staged = Self {
            original,
            generated: String::new(),
            segments: Vec::new(),
        };
        // Native Rust programs retain their entry point and crate-level items.
        if let Some(entry) = native_entry(original) {
            staged.copy(0, entry);
            staged.generated.push_str(" ::codingpad::__start(); ");
            staged.copy(entry, original.len());
            return staged;
        }

        let parse = |input: ParseStream<'_>| {
            let attrs = input.call(Attribute::parse_inner)?;
            let statements = input.call(syn::Block::parse_within)?;
            Ok((attrs, statements))
        };
        let (body, tail) = syn::parse::Parser::parse_str(parse, original)
            .map(|(attrs, statements)| {
                let body = attrs.last().map_or(0, |attr| attr.span().byte_range().end);
                let tail = match statements.last() {
                    Some(Stmt::Expr(expr, None)) if plain(expr) => Some(expr.span().byte_range()),
                    _ => None,
                };
                (body, tail)
            })
            .unwrap_or((0, None));
        staged.copy(0, body);
        staged
            .generated
            .push_str("\nfn main() { ::codingpad::__start();\n");
        if let Some(tail) = tail {
            staged.copy(body, tail.start);
            staged.generated.push_str("::codingpad::__finish(");
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

    fn original_offset(&self, offset: usize) -> Option<usize> {
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

    pub(super) fn trace_position(&self, line: usize, column: usize) -> Option<(usize, usize)> {
        let mut lines = self.generated.split_inclusive('\n');
        let offset = (&mut lines)
            .take(line.checked_sub(1)?)
            .map(str::len)
            .sum::<usize>();
        let generated = lines.next()?;
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
    // parse_file strips a BOM and shebang before parsing. Account for the
    // prefix when translating the entry point's span back into the snapshot.
    let bom = source
        .strip_prefix('\u{feff}')
        .map_or(0, |_| '\u{feff}'.len_utf8());
    let source = &source[bom..];
    let file = syn::parse_file(source).ok()?;
    let shebang = file.shebang.as_ref().map_or(0, String::len);
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
    Some(bom + shebang + entry)
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
        let staged = Source::new(original);
        assert!(staged.generated.starts_with("#![allow(unused)]\nfn main()"));
        let expr = "::codingpad::__finish(café.len()); // dump";
        assert!(staged.generated.contains(expr), "{}", staged.generated);
        let offset = staged.generated.find("café.len").unwrap();
        assert_eq!(staged.position(offset), Some((3, 1)));
        assert!(!Source::new("42;").generated.contains("__finish"));
    }

    #[test]
    fn adds_panic_capture_after_an_explicit_mains_inner_attributes() {
        let source = "#!/usr/bin/env rust-script\n#![allow(unused)]\nfn main() { #![allow(unused)] println!(\"hello\"); }";
        let staged = Source::new(source);
        assert!(staged
            .generated
            .contains("{ #![allow(unused)] ::codingpad::__start();"));
        assert_eq!(
            staged
                .generated
                .replacen(" ::codingpad::__start(); ", "", 1),
            source
        );
    }
}
