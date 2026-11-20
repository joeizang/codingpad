//! Recognizes the standard structural Debug format. Custom Debug implementations
//! can emit arbitrary text, which remains readable as a string dump.
use serde_json::Value;

pub(super) type Index = usize;
const MAX_NODES: usize = 500_000;

pub(super) enum ValueNode {
    Null,
    Scalar(Value),
    Text(String),
    List(Vec<Index>),
    Record(Vec<(String, Index)>),
    Html(String),
    Image {
        mime: String,
        name: String,
        data: String,
    },
    Live {
        key: u64,
        revision: u64,
        value: Index,
    },
}

pub(super) struct Node {
    pub(super) name: String,
    pub(super) value: ValueNode,
}

pub(super) struct Snapshot {
    pub(super) nodes: Vec<Node>,
    pub(super) root: Index,
    pub(super) truncated: bool,
}

impl Snapshot {
    pub(super) fn new(text: String, name: String, truncated: bool) -> Self {
        let mut parser = Parser {
            rest: &text,
            nodes: Vec::new(),
        };
        if !truncated {
            if let Some(root) = parser.value(0).filter(|_| parser.rest.trim().is_empty()) {
                return Self::tree(parser.nodes, root, name, false);
            }
        }
        if truncated || parser.nodes.len() >= MAX_NODES {
            let mut parser = Parser {
                rest: &text,
                nodes: Vec::new(),
            };
            if let Some(root) = parser.partial_list() {
                return Self::tree(parser.nodes, root, name, true);
            }
        }
        Self {
            nodes: vec![Node {
                name,
                value: ValueNode::Text(text),
            }],
            root: 0,
            truncated,
        }
    }
    fn tree(mut nodes: Vec<Node>, root: Index, name: String, truncated: bool) -> Self {
        if nodes[root].name.is_empty() {
            nodes[root].name = name;
        }
        Self {
            nodes,
            root,
            truncated,
        }
    }

    pub(super) fn retained_bytes(&self, text_bytes: usize) -> usize {
        text_bytes + self.nodes.capacity() * std::mem::size_of::<Node>()
    }
}

struct Parser<'a> {
    rest: &'a str,
    nodes: Vec<Node>,
}

impl<'a> Parser<'a> {
    fn trim(&mut self) {
        self.rest = self.rest.trim_start();
    }

    fn take(&mut self, character: char) -> bool {
        self.trim();
        if let Some(rest) = self.rest.strip_prefix(character) {
            self.rest = rest;
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Option<Index> {
        // Guard the host independently of the captured character limit.
        if depth > 64 || self.nodes.len() >= MAX_NODES {
            return None;
        }
        self.trim();
        let mut name = "";
        let value = match self.rest.chars().next()? {
            '"' => ValueNode::Text(self.quoted('"')?),
            '\'' => ValueNode::Text(self.quoted('\'')?),
            '[' => {
                self.take('[');
                ValueNode::List(self.sequence(']', depth + 1)?)
            }
            '(' => {
                self.take('(');
                let values = self.sequence(')', depth + 1)?;
                if values.is_empty() {
                    ValueNode::Null
                } else {
                    ValueNode::List(values)
                }
            }
            '{' => {
                self.take('{');
                self.braces(depth + 1)?
            }
            _ => {
                let end = self
                    .rest
                    .find(|ch: char| {
                        ch.is_whitespace()
                            || matches!(ch, '(' | ')' | '[' | ']' | '{' | '}' | ',' | ':')
                    })
                    .unwrap_or(self.rest.len());
                if end == 0 {
                    return None;
                }
                (name, self.rest) = self.rest.split_at(end);
                if self.take('{') {
                    let ValueNode::Record(fields) = self.braces(depth + 1)? else {
                        return None;
                    };
                    ValueNode::Record(fields)
                } else if self.take('(') {
                    let values = self.sequence(')', depth + 1)?;
                    self.variant(name, values)?
                } else {
                    match name {
                        "None" => ValueNode::Null,
                        "true" | "false" => ValueNode::Scalar(Value::Bool(name == "true")),
                        _ => number(name)
                            .map_or_else(|| ValueNode::Text(name.to_owned()), ValueNode::Scalar),
                    }
                }
            }
        };
        let label = match value {
            ValueNode::Record(_) | ValueNode::List(_) => name,
            _ => "",
        };
        let index = self.nodes.len();
        self.nodes.push(Node {
            name: label.to_owned(),
            value,
        });
        Some(index)
    }

    fn quoted(&mut self, quote: char) -> Option<String> {
        let mut escaped = false;
        for (index, ch) in self.rest.char_indices().skip(1) {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                let (literal, rest) = self.rest.split_at(index + 1);
                self.rest = rest;
                return if quote == '"' {
                    // JSON is the common fast path, including large image data.
                    serde_json::from_str(literal).ok().or_else(|| {
                        syn::parse_str::<syn::LitStr>(literal)
                            .ok()
                            .map(|literal| literal.value())
                    })
                } else {
                    syn::parse_str::<syn::LitChar>(literal)
                        .ok()
                        .map(|literal| literal.value().to_string())
                };
            }
        }
        None
    }

    fn sequence(&mut self, close: char, depth: usize) -> Option<Vec<Index>> {
        let mut values = Vec::new();
        while !self.take(close) {
            values.push(self.value(depth)?);
            if !self.take(',') {
                return self.take(close).then_some(values);
            }
        }
        Some(values)
    }

    fn partial_list(&mut self) -> Option<Index> {
        if !self.take('[') {
            return None;
        }
        let mut items = Vec::new();
        loop {
            let checkpoint = self.nodes.len();
            let Some(item) = self.value(1) else {
                self.nodes.truncate(checkpoint);
                break;
            };
            // The formatting limit may fall in the middle of a scalar.
            if !(self.take(',') || self.take(']')) {
                self.nodes.truncate(checkpoint);
                break;
            }
            items.push(item);
        }
        if items.is_empty() {
            return None;
        }
        let root = self.nodes.len();
        self.nodes.push(Node {
            name: String::new(),
            value: ValueNode::List(items),
        });
        Some(root)
    }

    fn braces(&mut self, depth: usize) -> Option<ValueNode> {
        if self.take('}') {
            return Some(ValueNode::Record(Vec::new()));
        }
        let start = self.rest;
        let checkpoint = self.nodes.len();
        let first = self.value(depth)?;
        if !self.take(':') {
            let mut items = vec![first];
            if self.take(',') {
                items.extend(self.sequence('}', depth)?);
            } else if !self.take('}') {
                return None;
            }
            return Some(ValueNode::List(items));
        }
        let mut key = self.key(first, start);
        self.nodes.truncate(checkpoint);
        let mut fields = Vec::new();
        loop {
            fields.push((key, self.value(depth)?));
            if !self.take(',') {
                return self.take('}').then_some(ValueNode::Record(fields));
            }
            if self.take('}') {
                return Some(ValueNode::Record(fields));
            }
            let start = self.rest;
            let checkpoint = self.nodes.len();
            let index = self.value(depth)?;
            if !self.take(':') {
                return None;
            }
            key = self.key(index, start);
            self.nodes.truncate(checkpoint);
        }
    }

    fn key(&self, index: Index, start: &str) -> String {
        match &self.nodes[index].value {
            ValueNode::Text(text) => text.clone(),
            _ => start[..start.len() - self.rest.len() - 1].trim().to_owned(),
        }
    }

    fn variant(&mut self, name: &str, values: Vec<Index>) -> Option<ValueNode> {
        let text = |index: Index| match &self.nodes.get(index)?.value {
            ValueNode::Text(text) => Some(text.clone()),
            _ => None,
        };
        match (name, values.as_slice()) {
            ("__CodingPadHtml", [markup]) => Some(ValueNode::Html(text(*markup)?)),
            ("__CodingPadImage", [mime, name, data]) => {
                let mime = text(*mime)?;
                if !matches!(
                    mime.as_str(),
                    "image/png"
                        | "image/jpeg"
                        | "image/gif"
                        | "image/webp"
                        | "image/bmp"
                        | "image/svg+xml"
                ) {
                    return None;
                }
                Some(ValueNode::Image {
                    mime,
                    name: text(*name)?,
                    data: text(*data)?,
                })
            }
            ("__CodingPadLive", [key, revision, value]) => {
                let unsigned = |index: Index| match &self.nodes[index].value {
                    ValueNode::Scalar(value) => value.as_u64(),
                    _ => None,
                };
                Some(ValueNode::Live {
                    key: unsigned(*key)?,
                    revision: unsigned(*revision)?,
                    value: *value,
                })
            }
            _ => Some(ValueNode::Record(
                values
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| {
                        (
                            if name == "Some" {
                                "value".to_owned()
                            } else {
                                index.to_string()
                            },
                            value,
                        )
                    })
                    .collect(),
            )),
        }
    }
}

fn number(text: &str) -> Option<Value> {
    const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
    if let Ok(value) = text.parse::<i64>() {
        return (value.unsigned_abs() <= MAX_SAFE_INTEGER).then(|| Value::from(value));
    }
    if text
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'-')
    {
        return None;
    }
    let value: f64 = text.parse().ok()?;
    value.is_finite().then(|| Value::from(value))
}
