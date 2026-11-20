use syn::{parse::ParseStream, spanned::Spanned, Attribute, Expr, Item, Stmt};

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
        if syn::parse_file(original).is_ok_and(|file| {
            file.items
                .iter()
                .any(|item| matches!(item, Item::Fn(function) if function.sig.ident == "main"))
        }) {
            staged.copy(0, original.len());
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
        staged.generated.push_str("\nfn main() {\n");
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
    fn leaves_an_explicit_main_alone() {
        let source = "#![allow(unused)]\nfn main() { println!(\"hello\"); }";
        assert_eq!(Source::new(source).generated, source);
    }
}
