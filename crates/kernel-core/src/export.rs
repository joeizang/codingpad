//! CSV and Excel export of table dumps (D27).
//!
//! The host pulls export pages (plain cell values, never truncated for display)
//! from a kernel or from a stored SQL result and streams them into a file. The
//! file is written next to its destination under a temporary name and renamed
//! only when the export completes, so a cancelled or failed export never leaves
//! a partial file behind.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Rows requested per export page.
pub const EXPORT_PAGE: u32 = 1_000;
/// Excel's limits: 1,048,576 rows (one is the header) and 32,767 characters a cell.
const XLSX_MAX_ROWS: u64 = 1_048_575;
const XLSX_MAX_COLUMNS: usize = 16_384;
const XLSX_MAX_TEXT: usize = 32_767;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Csv,
    Xlsx,
}

/// One page of plain cells. A cell is `null`, a boolean, a number, a string, or
/// `{"n": "<text>"}` for an exact decimal that should stay numeric in Excel.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ExportPage {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub next: Option<String>,
    /// Total rows when known; deferred sequences report `None` until finished.
    #[serde(default)]
    pub total: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummary {
    pub rows: u64,
    /// Cells cut to Excel's 32,767-character limit.
    pub shortened_cells: u64,
}

pub const CANCELLED: &str = "Export cancelled.";

/// Pull every page from `fetch` and write it to `path`.
///
/// `progress` receives the rows written so far and the total when known.
pub async fn export_table<F, Fut>(
    path: &Path,
    format: ExportFormat,
    mut fetch: F,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<ExportSummary, String>
where
    F: FnMut(Option<String>, u32) -> Fut,
    Fut: Future<Output = Result<ExportPage, String>>,
{
    let partial = partial_path(path)?;
    let result = async {
        let mut writer: Option<Box<dyn TableWriter + Send>> = None;
        let mut cursor = None;
        let mut summary = ExportSummary::default();
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(CANCELLED.to_owned());
            }
            let page = fetch(cursor.clone(), EXPORT_PAGE).await?;
            let writer = match &mut writer {
                Some(writer) => writer,
                None => writer.insert(match format {
                    ExportFormat::Csv => Box::new(CsvWriter::create(&partial, &page.columns)?),
                    ExportFormat::Xlsx => Box::new(XlsxWriter::create(&page.columns)?),
                }),
            };
            for row in &page.rows {
                writer.row(row, &mut summary)?;
            }
            progress(summary.rows, page.total);
            match page.next {
                // A cursor that does not advance would loop forever.
                Some(next) if Some(&next) != cursor.as_ref() && !page.rows.is_empty() => {
                    cursor = Some(next)
                }
                Some(_) => return Err("The table stopped returning rows.".to_owned()),
                None => break,
            }
        }
        if cancel.load(Ordering::SeqCst) {
            return Err(CANCELLED.to_owned());
        }
        match writer {
            Some(writer) => {
                let partial = partial.clone();
                tokio::task::spawn_blocking(move || writer.finish(&partial))
                    .await
                    .map_err(|error| error.to_string())??
            }
            None => return Err("The table has no rows to export.".to_owned()),
        }
        Ok(summary)
    }
    .await;
    match result {
        Ok(summary) => {
            std::fs::rename(&partial, path)
                .map_err(|error| format!("Could not save {}: {error}", path.display()))?;
            Ok(summary)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&partial);
            Err(error)
        }
    }
}

fn partial_path(path: &Path) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or_else(|| "Choose a file name for the export.".to_owned())?;
    let mut partial = name.to_os_string();
    partial.push(".codingpad-partial");
    Ok(path.with_file_name(partial))
}

/// Convert a dump cell node into a plain export cell. Used for stored SQL
/// results, whose cells are display nodes; `full` holds text cut for display.
pub fn plain_cell(node: &Value) -> Value {
    match node.get("kind").and_then(Value::as_str) {
        Some("null") | None => Value::Null,
        Some("primitive") => node.get("v").cloned().unwrap_or(Value::Null),
        Some("string") => {
            let text = node
                .get("full")
                .or_else(|| node.get("v"))
                .cloned()
                .unwrap_or(Value::Null);
            let numeric = matches!(
                node.get("type").and_then(Value::as_str),
                Some("int" | "int8" | "numeric" | "money")
            );
            match text {
                Value::String(text) if numeric => serde_json::json!({ "n": text }),
                other => other,
            }
        }
        Some("lazy") => node.get("preview").cloned().unwrap_or(Value::Null),
        Some(_) => Value::String(node.to_string()),
    }
}

/// Text form of a plain cell, as written to CSV.
fn cell_text(cell: &Value) -> String {
    match cell {
        Value::Null => String::new(),
        Value::Bool(value) => if *value { "true" } else { "false" }.to_owned(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        Value::Object(object) => match object.get("n") {
            Some(Value::String(text)) => text.clone(),
            _ => cell.to_string(),
        },
        Value::Array(_) => cell.to_string(),
    }
}

trait TableWriter {
    fn row(&mut self, cells: &[Value], summary: &mut ExportSummary) -> Result<(), String>;
    fn finish(self: Box<Self>, path: &Path) -> Result<(), String>;
}

struct CsvWriter {
    out: BufWriter<std::fs::File>,
    columns: usize,
}

impl CsvWriter {
    fn create(path: &Path, columns: &[String]) -> Result<Self, String> {
        let file = std::fs::File::create(path)
            .map_err(|error| format!("Could not create {}: {error}", path.display()))?;
        let mut writer = Self {
            out: BufWriter::new(file),
            columns: columns.len(),
        };
        // A byte-order mark makes Excel on Windows read the file as UTF-8.
        writer.write(b"\xEF\xBB\xBF")?;
        let header: Vec<Value> = columns.iter().cloned().map(Value::String).collect();
        writer.line(&header)?;
        Ok(writer)
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.out
            .write_all(bytes)
            .map_err(|error| format!("Could not write the export: {error}"))
    }

    fn line(&mut self, cells: &[Value]) -> Result<(), String> {
        let mut line = String::new();
        for index in 0..self.columns.max(cells.len()) {
            if index > 0 {
                line.push(',');
            }
            let text = cells.get(index).map(cell_text).unwrap_or_default();
            if text.contains([',', '"', '\r', '\n']) || text.trim() != text {
                line.push('"');
                line.push_str(&text.replace('"', "\"\""));
                line.push('"');
            } else {
                line.push_str(&text);
            }
        }
        line.push_str("\r\n");
        self.write(line.as_bytes())
    }
}

impl TableWriter for CsvWriter {
    fn row(&mut self, cells: &[Value], summary: &mut ExportSummary) -> Result<(), String> {
        self.line(cells)?;
        summary.rows += 1;
        Ok(())
    }

    fn finish(mut self: Box<Self>, _path: &Path) -> Result<(), String> {
        self.out
            .flush()
            .map_err(|error| format!("Could not write the export: {error}"))
    }
}

struct XlsxWriter {
    workbook: rust_xlsxwriter::Workbook,
    row: u32,
}

fn xlsx_error(error: rust_xlsxwriter::XlsxError) -> String {
    format!("Could not write the Excel file: {error}")
}

impl XlsxWriter {
    fn create(columns: &[String]) -> Result<Self, String> {
        if columns.len() > XLSX_MAX_COLUMNS {
            return Err(format!(
                "Excel supports at most {XLSX_MAX_COLUMNS} columns; this table has {}. Export it as CSV instead.",
                columns.len()
            ));
        }
        let mut workbook = rust_xlsxwriter::Workbook::new();
        let bold = rust_xlsxwriter::Format::new().set_bold();
        let sheet = workbook
            .add_worksheet_with_constant_memory()
            .map_err(xlsx_error)?;
        for (index, name) in columns.iter().enumerate() {
            sheet
                .write_string_with_format(0, index as u16, shorten(name).0, &bold)
                .map_err(xlsx_error)?;
        }
        Ok(Self { workbook, row: 0 })
    }
}

fn shorten(text: &str) -> (&str, bool) {
    match text.char_indices().nth(XLSX_MAX_TEXT) {
        Some((end, _)) => (&text[..end], true),
        None => (text, false),
    }
}

impl TableWriter for XlsxWriter {
    fn row(&mut self, cells: &[Value], summary: &mut ExportSummary) -> Result<(), String> {
        if u64::from(self.row) >= XLSX_MAX_ROWS {
            return Err(format!(
                "Excel supports at most {XLSX_MAX_ROWS} rows. Export this table as CSV instead."
            ));
        }
        self.row += 1;
        let row = self.row;
        let sheet = self.workbook.worksheet_from_index(0).map_err(xlsx_error)?;
        for (index, cell) in cells.iter().enumerate().take(XLSX_MAX_COLUMNS) {
            let column = index as u16;
            let number = match cell {
                Value::Number(number) => number.as_f64(),
                Value::Object(object) => object
                    .get("n")
                    .and_then(Value::as_str)
                    .and_then(|text| text.trim().parse::<f64>().ok()),
                _ => None,
            };
            let written = match (cell, number) {
                (Value::Null, _) => continue,
                (_, Some(number)) if number.is_finite() => sheet.write_number(row, column, number),
                (Value::Bool(value), _) => sheet.write_boolean(row, column, *value),
                _ => {
                    let text = cell_text(cell);
                    let (text, cut) = shorten(&text);
                    if cut {
                        summary.shortened_cells += 1;
                    }
                    sheet.write_string(row, column, text)
                }
            };
            written.map_err(xlsx_error)?;
        }
        summary.rows += 1;
        Ok(())
    }

    fn finish(mut self: Box<Self>, path: &Path) -> Result<(), String> {
        self.workbook.save(path).map_err(xlsx_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pages(rows: usize, page: usize) -> Vec<ExportPage> {
        let mut result = vec![];
        let mut start = 0;
        while start < rows {
            let end = (start + page).min(rows);
            result.push(ExportPage {
                columns: vec!["id".into(), "name".into()],
                rows: (start..end)
                    .map(|i| vec![json!(i), json!(format!("row {i}"))])
                    .collect(),
                next: (end < rows).then(|| end.to_string()),
                total: Some(rows as u64),
            });
            start = end;
        }
        result
    }

    async fn run(
        path: &Path,
        format: ExportFormat,
        source: Vec<ExportPage>,
        cancel: &AtomicBool,
    ) -> Result<ExportSummary, String> {
        export_table(
            path,
            format,
            |cursor, _| {
                let index = cursor.map_or(0, |cursor| {
                    source
                        .iter()
                        .position(|page| page.rows[0][0] == json!(cursor.parse::<u64>().unwrap()))
                        .unwrap()
                });
                let page = source[index].clone();
                async move { Ok(page) }
            },
            cancel,
            |_, _| {},
        )
        .await
    }

    #[tokio::test]
    async fn csv_quotes_fields_and_writes_every_page() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("out.csv");
        let source = vec![ExportPage {
            columns: vec!["a".into(), "b,c".into()],
            rows: vec![
                vec![json!("x\"y"), json!(null)],
                vec![json!("line\nbreak"), json!({"n": "1.50"})],
                vec![json!(true), json!(" padded")],
            ],
            next: None,
            total: Some(3),
        }];
        let summary = run(&path, ExportFormat::Csv, source, &AtomicBool::new(false))
            .await
            .unwrap();
        assert_eq!(summary.rows, 3);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "\u{feff}a,\"b,c\"\r\n\"x\"\"y\",\r\n\"line\nbreak\",1.50\r\ntrue,\" padded\"\r\n"
        );
        let big = directory.path().join("big.csv");
        let summary = run(
            &big,
            ExportFormat::Csv,
            pages(100_000, 1_000),
            &AtomicBool::new(false),
        )
        .await
        .unwrap();
        assert_eq!(summary.rows, 100_000);
        let text = std::fs::read_to_string(&big).unwrap();
        assert_eq!(text.lines().count(), 100_001);
        assert!(text.ends_with("99999,row 99999\r\n"));
    }

    #[tokio::test]
    async fn xlsx_export_is_a_workbook_with_every_row() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("out.xlsx");
        let summary = run(
            &path,
            ExportFormat::Xlsx,
            pages(100_000, 1_000),
            &AtomicBool::new(false),
        )
        .await
        .unwrap();
        assert_eq!(
            summary,
            ExportSummary {
                rows: 100_000,
                shortened_cells: 0
            }
        );
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], b"PK", "an .xlsx file is a zip archive");
    }

    #[tokio::test]
    async fn long_text_is_shortened_to_the_excel_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("long.xlsx");
        let source = vec![ExportPage {
            columns: vec!["text".into()],
            rows: vec![vec![json!("é".repeat(XLSX_MAX_TEXT + 5))]],
            next: None,
            total: Some(1),
        }];
        let summary = run(&path, ExportFormat::Xlsx, source, &AtomicBool::new(false))
            .await
            .unwrap();
        assert_eq!(summary.shortened_cells, 1);
    }

    #[tokio::test]
    async fn cancelling_leaves_no_file_behind() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("out.csv");
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let error = export_table(
            &path,
            ExportFormat::Csv,
            |cursor, _| {
                calls += 1;
                if calls == 3 {
                    cancel.store(true, Ordering::SeqCst);
                }
                let start = cursor.map_or(0, |cursor| cursor.parse::<usize>().unwrap());
                let page = ExportPage {
                    columns: vec!["id".into()],
                    rows: vec![vec![json!(start)]],
                    next: Some((start + 1).to_string()),
                    total: None,
                };
                async move { Ok(page) }
            },
            &cancel,
            |_, _| {},
        )
        .await
        .unwrap_err();
        assert_eq!(error, CANCELLED);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_cursor_that_does_not_advance_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("out.csv");
        let error = export_table(
            &path,
            ExportFormat::Csv,
            |_, _| async {
                Ok(ExportPage {
                    columns: vec!["id".into()],
                    rows: vec![vec![json!(1)]],
                    next: Some("0".into()),
                    total: None,
                })
            },
            &AtomicBool::new(false),
            |_, _| {},
        )
        .await;
        // The first page has no cursor, so "0" advances once; the second repeats it.
        assert!(error.unwrap_err().contains("stopped returning rows"));
        assert!(!path.exists());
    }

    #[test]
    fn sql_display_cells_become_plain_values() {
        assert_eq!(plain_cell(&json!({"kind": "null"})), json!(null));
        assert_eq!(plain_cell(&json!({"kind": "primitive", "v": 3})), json!(3));
        assert_eq!(
            plain_cell(&json!({"kind": "string", "v": "ab", "truncated": true, "full": "abcdef"})),
            json!("abcdef")
        );
        assert_eq!(
            plain_cell(&json!({"kind": "string", "v": "12.50", "type": "numeric"})),
            json!({"n": "12.50"})
        );
        assert_eq!(
            plain_cell(&json!({"kind": "string", "v": "2024-01-02", "type": "date"})),
            json!("2024-01-02")
        );
    }
}
