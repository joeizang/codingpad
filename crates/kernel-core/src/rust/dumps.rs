use super::debug::{Index, Node, Snapshot, ValueNode};
use crate::{databases::TableData, export::ExportPage};
use serde_json::{json, Value};
use std::{
    collections::{HashSet, VecDeque},
    sync::atomic::{AtomicU64, Ordering},
};

const EXPIRED: &str = "That preview is no longer available. Run the script again.";
const PAGE: usize = 100;

pub(crate) struct Dumps {
    run: u64,
    eager: bool,
    next_node: u64,
    next_snapshot: u64,
    bytes: usize,
    snapshots: VecDeque<Stored>,
    queries: VecDeque<StoredQuery>,
    query_bytes: usize,
    next_query: u64,
}

struct Stored {
    id: u64,
    bytes: usize,
    snapshot: Snapshot,
}

struct StoredQuery {
    id: u64,
    bytes: usize,
    table: TableData,
}

impl Dumps {
    pub(super) fn new(eager: bool) -> Self {
        static NEXT_RUN: AtomicU64 = AtomicU64::new(1);
        Self {
            run: NEXT_RUN.fetch_add(1, Ordering::Relaxed),
            eager,
            next_node: 0,
            next_snapshot: 0,
            bytes: 0,
            snapshots: VecDeque::new(),
            queries: VecDeque::new(),
            query_bytes: 0,
            next_query: 0,
        }
    }

    pub(super) fn run_id(&self) -> u64 {
        self.run
    }

    pub(super) fn id(&mut self) -> String {
        id(self.run, &mut self.next_node)
    }

    pub(super) fn next_query(&mut self) -> u64 {
        self.next_query += 1;
        self.next_query
    }

    pub(super) fn store_query(&mut self, id: u64, table: TableData, bytes: usize) {
        while self.queries.len() >= 64 || self.query_bytes + bytes > 32 * 1024 * 1024 {
            let Some(oldest) = self.queries.pop_front() else {
                break;
            };
            self.query_bytes -= oldest.bytes;
        }
        self.queries.push_back(StoredQuery { id, table, bytes });
        self.query_bytes += bytes;
    }

    pub(super) fn insert(&mut self, snapshot: Snapshot, bytes: usize) -> u64 {
        // The viewer retains 500 blocks. Also bound retained snapshot text for
        // scripts with many large dumps or a tight loop of live updates.
        while self.snapshots.len() >= 500 || self.bytes + bytes > 64 * 1024 * 1024 {
            let Some(oldest) = self.snapshots.pop_front() else {
                break;
            };
            self.bytes -= oldest.bytes;
        }
        self.next_snapshot += 1;
        self.snapshots.push_back(Stored {
            id: self.next_snapshot,
            bytes,
            snapshot,
        });
        self.bytes += bytes;
        self.next_snapshot
    }

    pub(super) fn remove(&mut self, group: u64) {
        if let Some(index) = self.snapshots.iter().position(|entry| entry.id == group) {
            if let Some(entry) = self.snapshots.remove(index) {
                self.bytes -= entry.bytes;
            }
        }
    }

    pub(super) fn initial(&mut self, group: u64, index: Index, id: Option<String>) -> Value {
        self.current(group, index, id)
            .expect("render immediately after inserting a dump")
    }

    pub(super) fn current(
        &mut self,
        group: u64,
        index: Index,
        id: Option<String>,
    ) -> Option<Value> {
        let entry = self
            .snapshots
            .iter()
            .rev()
            .find(|entry| entry.id == group)?;
        Some(
            Renderer::new(
                self.run,
                self.eager,
                &mut self.next_node,
                entry,
                &self.queries,
            )
            .node(index, 0, PAGE, 0, false, id),
        )
    }

    pub(crate) fn request(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        let handle = params["handle"].as_str().ok_or(EXPIRED)?;
        let mut parts = handle.split(':');
        let run: u64 = parts
            .next()
            .and_then(|part| part.parse().ok())
            .ok_or(EXPIRED)?;
        let group: u64 = parts
            .next()
            .and_then(|part| part.parse().ok())
            .ok_or(EXPIRED)?;
        let start = match params["cursor"].as_str() {
            Some(cursor) => cursor
                .parse::<usize>()
                .map_err(|_| "Invalid Rust dump cursor")?,
            None => 0,
        };
        let limit = params["limit"]
            .as_u64()
            .unwrap_or(PAGE as u64)
            .clamp(1, 1_000) as usize;
        if run != self.run {
            return Err(EXPIRED.into());
        }
        if parts.clone().eq(["query"]) {
            let table = self
                .queries
                .iter()
                .find(|query| query.id == group)
                .ok_or(EXPIRED)?;
            let limit = limit.min((16_000 / table.table.column_names().count().max(1)).max(1));
            return Ok(if method == "expand" {
                json!({ "node": table.table.node(handle, start, limit) })
            } else {
                json!(table.table.export_page(start, limit))
            });
        }
        let index: Index = parts
            .next()
            .and_then(|part| part.parse().ok())
            .ok_or(EXPIRED)?;
        if parts.next().is_some() {
            return Err(EXPIRED.into());
        }
        let entry = self
            .snapshots
            .iter()
            .find(|entry| entry.id == group && index < entry.snapshot.nodes.len())
            .ok_or(EXPIRED)?;
        if method == "expand" {
            let node = Renderer::new(
                self.run,
                self.eager,
                &mut self.next_node,
                entry,
                &self.queries,
            )
            .node(index, start, limit, 0, false, None);
            return Ok(json!({ "node": node }));
        }
        let Some((items, columns)) = table(&entry.snapshot, index, usize::MAX) else {
            return Err("That Rust dump is not a table.".into());
        };
        let limit = limit.min((16_000 / columns.len()).max(1));
        let rows = items
            .iter()
            .skip(start)
            .take(limit)
            .map(|&row| {
                columns
                    .iter()
                    .map(|column| {
                        cell(&entry.snapshot, row, column)
                            .map_or(Value::Null, |index| export_cell(&entry.snapshot, index))
                    })
                    .collect()
            })
            .collect::<Vec<_>>();
        Ok(json!(ExportPage {
            columns,
            next: next(start, rows.len(), items.len()),
            total: Some(items.len() as u64),
            rows,
        }))
    }
}

fn id(run: u64, next: &mut u64) -> String {
    *next += 1;
    format!("{run}:n{next}")
}

struct Renderer<'a> {
    run: u64,
    group: u64,
    snapshot: &'a Snapshot,
    ids: &'a mut u64,
    eager: bool,
    remaining: usize,
    queries: &'a VecDeque<StoredQuery>,
    text_bytes: usize,
}

impl<'a> Renderer<'a> {
    fn new(
        run: u64,
        eager: bool,
        ids: &'a mut u64,
        entry: &'a Stored,
        queries: &'a VecDeque<StoredQuery>,
    ) -> Self {
        Self {
            run,
            group: entry.id,
            snapshot: &entry.snapshot,
            queries,
            ids,
            eager,
            remaining: 1_000,
            text_bytes: 2 * 1024 * 1024,
        }
    }
    fn node(
        &mut self,
        index: Index,
        start: usize,
        limit: usize,
        depth: usize,
        cell: bool,
        id_override: Option<String>,
    ) -> Value {
        let id = id_override.unwrap_or_else(|| id(self.run, self.ids));
        self.remaining = self.remaining.saturating_sub(1);
        let node = &self.snapshot.nodes[index];
        let handle = format!("{}:{}:{index}", self.run, self.group);
        let container = matches!(
            node.value,
            ValueNode::Record(_) | ValueNode::List(_) | ValueNode::Live { .. }
        );
        let rich = matches!(node.value, ValueNode::Image { .. } | ValueNode::Html(_));
        let depth_limit = if self.eager { 20 } else { 6 };
        let label = label(node);
        let truncated = index == self.snapshot.root && self.snapshot.truncated;
        if (container && (depth >= depth_limit || self.remaining == 0 || (cell && !self.eager)))
            || (rich && depth > 0)
        {
            return json!({"id": id, "kind": "lazy", "handle": handle, "type": label, "preview": label});
        }

        match &node.value {
            ValueNode::Null => json!({"id": id, "kind": "null"}),
            ValueNode::Scalar(value) => json!({"id": id, "kind": "primitive", "v": value}),
            ValueNode::Text(text) => {
                let cap = if cell { 1_000 } else { 10_000 };
                let end = text
                    .char_indices()
                    .take(cap + 1)
                    .take_while(|(index, ch)| index + ch.len_utf8() <= self.text_bytes)
                    .map(|(index, ch)| index + ch.len_utf8())
                    .last()
                    .unwrap_or(0);
                let end = end.min(text.char_indices().nth(cap).map_or(text.len(), |(i, _)| i));
                self.text_bytes -= end;
                let truncated = end < text.len() || truncated;
                json!({"id": id, "kind": "string", "type": label, "v": &text[..end],
                    "truncated": truncated, "total": text.chars().count()})
            }
            ValueNode::Query(query_id) => {
                let Some(query) = self.queries.iter().find(|query| query.id == *query_id) else {
                    return json!({"id": id, "kind": "string", "type": "query", "v": EXPIRED});
                };
                let columns = query.table.column_names().count().max(1);
                let limit = limit.min((16_000 / columns).max(1));
                let mut table =
                    query
                        .table
                        .node(&format!("{}:{query_id}:query", self.run), start, limit);
                table["id"] = json!(id);
                table
            }
            ValueNode::Html(html) => json!({"id": id, "kind": "html", "html": html}),
            ValueNode::Image { mime, name, data } => {
                let name = (!name.is_empty()).then_some(name);
                json!({"id": id, "kind": "image", "mime": mime, "name": name,
                    "src": format!("data:{mime};base64,{data}")})
            }
            ValueNode::Live { value, .. } => {
                let node = self.node(*value, 0, PAGE, depth + 1, false, None);
                json!({"id": id, "kind": "live", "node": node})
            }
            ValueNode::List(items) => {
                if let Some((items, columns)) = table(self.snapshot, index, 32) {
                    let rows = self.rows(items, &columns, start, limit, depth);
                    let columns = columns
                        .iter()
                        .map(|name| json!({"name": name}))
                        .collect::<Vec<_>>();
                    return json!({"id": id, "kind": "table", "type": label, "handle": handle,
                        "columns": columns, "rows": rows, "total": items.len(),
                        "next": next(start, rows.len(), items.len()), "truncated": truncated});
                }
                let mut page = Vec::new();
                for &item in items.iter().skip(start).take(limit) {
                    if self.remaining == 0 {
                        break;
                    }
                    page.push(self.node(item, 0, PAGE, depth + 1, false, None));
                }
                json!({"id": id, "kind": "list", "type": label, "items": page, "handle": handle,
                    "total": items.len(), "next": next(start, page.len(), items.len()), "truncated": truncated})
            }
            ValueNode::Record(fields) => {
                let mut page = Vec::new();
                for (name, value) in fields.iter().skip(start).take(limit) {
                    if self.remaining == 0 {
                        break;
                    }
                    page.push(json!({"name": name, "value": self.node(*value, 0, PAGE, depth + 1, false, None)}));
                }
                json!({"id": id, "kind": "object", "type": label, "fields": page, "handle": handle,
                    "total": fields.len(), "next": next(start, page.len(), fields.len()), "truncated": false})
            }
        }
    }

    fn rows(
        &mut self,
        items: &[Index],
        columns: &[String],
        start: usize,
        limit: usize,
        depth: usize,
    ) -> Vec<Vec<Value>> {
        let mut rows = Vec::new();
        for &row in items.iter().skip(start).take(limit) {
            if self.remaining < columns.len() {
                break;
            }
            rows.push(
                columns
                    .iter()
                    .map(|column| {
                        if let Some(index) = cell(self.snapshot, row, column) {
                            self.node(index, 0, PAGE, depth + 1, true, None)
                        } else {
                            self.remaining = self.remaining.saturating_sub(1);
                            json!({"id": id(self.run, self.ids), "kind": "null"})
                        }
                    })
                    .collect::<Vec<_>>(),
            );
        }
        rows
    }
}

fn label(node: &Node) -> &str {
    if !node.name.is_empty() {
        return &node.name;
    }
    match node.value {
        ValueNode::List(_) => "List",
        ValueNode::Record(_) => "Map",
        ValueNode::Html(_) => "HTML",
        ValueNode::Image { .. } => "Image",
        ValueNode::Live { .. } => "Live value",
        _ => "Value",
    }
}

fn next(start: usize, count: usize, total: usize) -> Option<String> {
    let end = start.saturating_add(count);
    (count > 0 && end < total).then(|| end.to_string())
}

fn table(
    snapshot: &Snapshot,
    index: Index,
    column_limit: usize,
) -> Option<(&[Index], Vec<String>)> {
    let ValueNode::List(items) = &snapshot.nodes[index].value else {
        return None;
    };
    let row_type = &snapshot.nodes[*items.first()?].name;
    if !items.iter().all(|&item| {
        let row = &snapshot.nodes[item];
        row.name == *row_type && matches!(row.value, ValueNode::Record(_))
    }) {
        return None;
    }
    let mut names = HashSet::new();
    let mut columns = Vec::new();
    for &item in items {
        let ValueNode::Record(fields) = &snapshot.nodes[item].value else {
            unreachable!()
        };
        for (name, _) in fields {
            if names.insert(name) {
                columns.push(name.clone());
                if columns.len() == column_limit {
                    return Some((items, columns));
                }
            }
        }
    }
    (!columns.is_empty()).then_some((items, columns))
}

fn cell(snapshot: &Snapshot, row: Index, column: &str) -> Option<Index> {
    let ValueNode::Record(fields) = &snapshot.nodes[row].value else {
        return None;
    };
    fields
        .iter()
        .find_map(|(name, value)| (name == column).then_some(*value))
}

fn export_cell(snapshot: &Snapshot, index: Index) -> Value {
    let value = export_value(snapshot, index);
    match value {
        Value::Array(_) | Value::Object(_) => Value::String(value.to_string()),
        value => value,
    }
}

fn export_value(snapshot: &Snapshot, index: Index) -> Value {
    let node = &snapshot.nodes[index];
    match &node.value {
        ValueNode::Null => Value::Null,
        ValueNode::Query(_) => Value::String("Query result".into()),
        ValueNode::Scalar(value) => value.clone(),
        ValueNode::Text(text) | ValueNode::Html(text) => Value::String(text.clone()),
        ValueNode::List(items) => Value::Array(
            items
                .iter()
                .map(|&item| export_value(snapshot, item))
                .collect(),
        ),
        ValueNode::Record(fields) => Value::Object(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), export_value(snapshot, *value)))
                .collect(),
        ),
        ValueNode::Live { value, .. } => export_value(snapshot, *value),
        ValueNode::Image { mime, name, .. } => Value::String(if name.is_empty() {
            mime.clone()
        } else {
            name.clone()
        }),
    }
}
