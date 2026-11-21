//! Every built-in sample must run against the bundled sample database.
//!
//! The structural, SQL, TypeScript and Rust checks run in the normal suite. The full
//! acceptance run is ignored because the first Python/C# database sample installs
//! SQLAlchemy or EF Core and may need network access; run it explicitly with
//! `cargo test -p codingpad-kernel-core --test samples -- --ignored`.

use codingpad_kernel_core::{
    attached_data_context,
    databases::{ConnectionStore, SqlManager},
    samples::{self, SampleLanguage},
    secrets::SecretStore,
    Language, Supervisor,
};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::time::timeout;

fn manager(directory: &std::path::Path) -> (Arc<ConnectionStore>, SqlManager) {
    let connections = Arc::new(ConnectionStore::new(directory));
    samples::ensure_database(directory).expect("prepare the sample database");
    let sql = SqlManager::new(connections.clone(), Arc::new(SecretStore::new(directory)));
    (connections, sql)
}

#[test]
fn every_sample_is_listed_with_runnable_source() {
    let samples = samples::all();
    let mut ids = std::collections::HashSet::new();
    for language in [
        SampleLanguage::Python,
        SampleLanguage::Csharp,
        SampleLanguage::Typescript,
        SampleLanguage::Rust,
        SampleLanguage::Sql,
    ] {
        let count = samples
            .iter()
            .filter(|sample| sample.language == language)
            .count();
        assert!(count >= 10, "{language:?} has only {count} samples");
    }
    for sample in &samples {
        assert!(sample.title.len() > 3, "{} has no title", sample.id);
        assert!(
            !sample.source.trim().is_empty(),
            "{} has no source",
            sample.id
        );
        assert!(
            ids.insert(sample.id.clone()),
            "duplicate sample id {}",
            sample.id
        );
        assert_eq!(
            Path::new(&sample.name)
                .extension()
                .and_then(|ext| ext.to_str()),
            Some(sample.language.extension()),
            "{}",
            sample.name
        );
    }
}

#[tokio::test]
async fn sql_samples_run_against_the_bundled_database() {
    let directory = tempfile::tempdir().unwrap();
    let (_connections, sql) = manager(directory.path());
    for sample in samples::all()
        .into_iter()
        .filter(|sample| sample.language == SampleLanguage::Sql)
    {
        let result = timeout(
            Duration::from_secs(30),
            sql.run(&sample.id, sample.source, None),
        )
        .await
        .unwrap_or_else(|_| panic!("{} timed out", sample.id))
        .unwrap_or_else(|error| panic!("{} failed: {error}", sample.id));
        assert!(result.ok, "{} did not finish successfully", sample.id);
        assert!(
            !result.tables.is_empty(),
            "{} returned no result table",
            sample.id
        );
    }
}

#[tokio::test]
async fn typescript_and_rust_samples_run_without_a_saved_path() {
    let directory = tempfile::tempdir().unwrap();
    let (connections, _) = manager(directory.path());
    let supervisor =
        Supervisor::with_data_dir(directory.path().to_path_buf()).with_connections(connections);
    for sample in samples::all() {
        let Some(language @ (Language::Typescript | Language::Rust)) = sample.language.language()
        else {
            continue;
        };
        let mut events = Vec::new();
        let result = timeout(
            Duration::from_secs(90),
            supervisor.run_language(&sample.id, sample.source, language, |event| {
                events.push(event);
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("{} timed out", sample.id))
        .unwrap_or_else(|error| panic!("{} failed: {error}", sample.id));
        assert!(result.ok, "{} failed: {events:?}", sample.id);
        let dumps: Vec<_> = events
            .iter()
            .filter(|event| event.method == "dump")
            .map(|event| &event.params["node"])
            .collect();
        assert!(!dumps.is_empty(), "{} produced no dump", sample.id);
        match (language, sample.name.as_str()) {
            (Language::Rust, "tables.rs") => {
                assert_eq!(dumps[0]["kind"], "table");
                assert_eq!(dumps[0]["columns"][0]["name"], "title");
                assert_eq!(dumps[0]["total"], 3);
            }
            (Language::Rust, "query_products.rs" | "query_orders.rs") => {
                assert_eq!(dumps[0]["kind"], "table");
                assert_eq!(dumps[0]["total"], 8);
            }
            (Language::Rust, "enums.rs") => {
                assert_eq!(dumps.len(), 3);
                assert!(dumps[1].to_string().contains("Invalid score: oops"));
            }
            _ => {}
        }
    }
    supervisor.shutdown().await;
}

#[tokio::test]
#[ignore = "installs SQLAlchemy and EF Core packages; run explicitly"]
async fn python_csharp_and_sql_samples_run_end_to_end() {
    let directory = tempfile::tempdir().unwrap();
    let (connections, sql) = manager(directory.path());
    let supervisor =
        Supervisor::with_data_dir(directory.path().to_path_buf()).with_connections(connections);
    let samples = samples::all();

    // Run the samples that need no database first, so each language keeps one
    // warm kernel for the plain scripts and restarts only for the db samples.
    for expected in [SampleLanguage::Python, SampleLanguage::Csharp] {
        let language = expected.language().unwrap();
        let mut scripts: Vec<_> = samples
            .iter()
            .filter(|sample| sample.language == expected)
            .collect();
        scripts.sort_by_key(|sample| {
            sample
                .source
                .to_ascii_lowercase()
                .contains("connection=codingpad-sample")
        });
        let script_id = format!("{expected:?}-samples");
        for sample in scripts {
            let (data, data_error) = attached_data_context(&sql, sample.source, language).await;
            assert_eq!(data_error, None, "{}: data context", sample.id);
            let mut events = Vec::new();
            let result = timeout(
                Duration::from_secs(180),
                supervisor.run_language_at_with_data(
                    &script_id,
                    sample.source,
                    language,
                    None,
                    data,
                    |event| events.push(event),
                ),
            )
            .await
            .unwrap_or_else(|_| panic!("{} timed out", sample.id))
            .unwrap_or_else(|error| panic!("{} failed: {error}", sample.id));
            assert!(result.ok, "{} failed: {events:?}", sample.id);
        }
    }

    for sample in samples
        .iter()
        .filter(|sample| sample.language == SampleLanguage::Sql)
    {
        let result = timeout(
            Duration::from_secs(30),
            sql.run(&sample.id, sample.source, None),
        )
        .await
        .unwrap_or_else(|_| panic!("{} timed out", sample.id))
        .unwrap_or_else(|error| panic!("{} failed: {error}", sample.id));
        assert!(result.ok, "{} did not finish successfully", sample.id);
    }
    supervisor.shutdown().await;
}
