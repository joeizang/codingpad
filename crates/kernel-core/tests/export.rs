//! Export of kernel table dumps through the Supervisor (D27, issue #46).

use codingpad_kernel_core::export::{export_table, ExportFormat, ExportSummary, CANCELLED};
use codingpad_kernel_core::{KernelEvent, Language, Supervisor};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::time::timeout;

async fn table_handle(
    supervisor: &Supervisor,
    id: &str,
    language: Language,
    source: &str,
) -> String {
    let mut events: Vec<KernelEvent> = Vec::new();
    let result = timeout(
        Duration::from_secs(60),
        supervisor.run_language(id, source, language, |event| events.push(event)),
    )
    .await
    .expect("run timed out")
    .expect("kernel failed");
    assert!(result.ok, "{events:?}");
    let node = events
        .iter()
        .find(|event| event.method == "dump" && event.params["node"]["kind"] == "table")
        .unwrap_or_else(|| panic!("no table dump: {events:?}"));
    node.params["node"]["handle"].as_str().unwrap().to_owned()
}

async fn export(
    supervisor: &Supervisor,
    id: &str,
    handle: &str,
    path: &Path,
    format: ExportFormat,
    cancel: &AtomicBool,
) -> Result<ExportSummary, String> {
    export_table(
        path,
        format,
        |cursor, limit| async move {
            supervisor
                .export_page(id, handle, cursor.as_deref(), limit)
                .await
        },
        cancel,
        |_, _| {},
    )
    .await
}

#[tokio::test]
async fn python_100k_rows_export_completely_to_csv_and_excel() {
    let supervisor = Supervisor::default();
    let handle = table_handle(
        &supervisor,
        "py",
        Language::Python,
        "from decimal import Decimal\nrows = [{'id': i, 'name': f'row {i}', 'price': Decimal('1.50'), 'note': 'x' * 5000 if i == 7 else None} for i in range(100_000)]\ndump(rows)",
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let csv = directory.path().join("rows.csv");
    let started = Instant::now();
    let summary = export(
        &supervisor,
        "py",
        &handle,
        &csv,
        ExportFormat::Csv,
        &AtomicBool::new(false),
    )
    .await
    .unwrap();
    eprintln!("python csv export of 100k rows: {:?}", started.elapsed());
    assert_eq!(summary.rows, 100_000);
    let text = std::fs::read_to_string(&csv).unwrap();
    let lines: Vec<&str> = text.split("\r\n").collect();
    assert_eq!(lines[0], "\u{feff}id,name,price,note");
    assert_eq!(lines[1], "0,row 0,1.50,");
    assert_eq!(
        lines[8].len(),
        "7,row 7,1.50,".len() + 5000,
        "long text is exported whole"
    );
    assert_eq!(lines[100_000], "99999,row 99999,1.50,");
    let xlsx = directory.path().join("rows.xlsx");
    let summary = export(
        &supervisor,
        "py",
        &handle,
        &xlsx,
        ExportFormat::Xlsx,
        &AtomicBool::new(false),
    )
    .await
    .unwrap();
    assert_eq!(summary.rows, 100_000);
    assert_eq!(&std::fs::read(&xlsx).unwrap()[..2], b"PK");
    supervisor.shutdown().await;
}

#[tokio::test]
async fn csharp_deferred_sequences_and_records_export_completely() {
    let supervisor = Supervisor::default();
    let handle = table_handle(
        &supervisor,
        "cs",
        Language::Csharp,
        r#"
Enumerable.Range(0, 100_000).Select(i => new Row(i, $"row {i}", 1.50m, i == 7 ? new string('x', 5000) : null)).Dump();
record Row(int Id, string Name, decimal Price, string? Note);
"#,
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let csv = directory.path().join("rows.csv");
    let started = Instant::now();
    let summary = export(
        &supervisor,
        "cs",
        &handle,
        &csv,
        ExportFormat::Csv,
        &AtomicBool::new(false),
    )
    .await
    .unwrap();
    eprintln!("C# csv export of 100k rows: {:?}", started.elapsed());
    assert_eq!(summary.rows, 100_000);
    let text = std::fs::read_to_string(&csv).unwrap();
    let lines: Vec<&str> = text.split("\r\n").collect();
    assert_eq!(lines[0], "\u{feff}Id,Name,Note,Price");
    assert_eq!(lines[1], "0,row 0,,1.50");
    assert_eq!(lines[8].len(), "7,row 7,,1.50".len() + 5000);
    assert_eq!(lines[100_000], "99999,row 99999,,1.50");
    let xlsx = directory.path().join("rows.xlsx");
    let summary = export(
        &supervisor,
        "cs",
        &handle,
        &xlsx,
        ExportFormat::Xlsx,
        &AtomicBool::new(false),
    )
    .await
    .unwrap();
    assert_eq!(summary.rows, 100_000);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn a_failing_sequence_fails_the_export_instead_of_ending_early() {
    let supervisor = Supervisor::default();
    let handle = table_handle(
        &supervisor,
        "fail",
        Language::Csharp,
        "Enumerable.Range(0, 1500).Select(i => i < 1200 ? i : throw new InvalidOperationException(\"boom\")).Dump();",
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.csv");
    let error = export(
        &supervisor,
        "fail",
        &handle,
        &path,
        ExportFormat::Csv,
        &AtomicBool::new(false),
    )
    .await
    .unwrap_err();
    assert!(error.contains("boom"), "{error}");
    assert!(!path.exists());
    supervisor.shutdown().await;
}

#[tokio::test]
async fn cancelling_an_endless_export_stops_and_leaves_no_file() {
    let supervisor = Supervisor::default();
    let handle = table_handle(
        &supervisor,
        "endless",
        Language::Csharp,
        "static IEnumerable<int> Forever() { for (var i = 0; ; i++) yield return i; }\nForever().Dump();",
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rows.csv");
    let cancel = AtomicBool::new(false);
    let error = export_table(
        &path,
        ExportFormat::Csv,
        |cursor, limit| {
            let supervisor = &supervisor;
            let handle = &handle;
            async move {
                supervisor
                    .export_page("endless", handle, cursor.as_deref(), limit)
                    .await
            }
        },
        &cancel,
        |rows, total| {
            assert_eq!(total, None, "an endless sequence has no known total");
            if rows >= 5_000 {
                cancel.store(true, Ordering::SeqCst);
            }
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error, CANCELLED);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    supervisor.shutdown().await;
}
