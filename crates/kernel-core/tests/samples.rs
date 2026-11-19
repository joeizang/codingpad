//! Every built-in sample must run against the bundled sample database.
//!
//! The structural, SQL and TypeScript checks run in the normal suite. The full
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
use std::{sync::Arc, time::Duration};
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
        let expected = match sample.language {
            SampleLanguage::Python => ".py",
            SampleLanguage::Csharp => ".cs",
            SampleLanguage::Typescript => ".ts",
            SampleLanguage::Sql => ".sql",
        };
        assert!(sample.name.ends_with(expected), "{}", sample.name);
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
async fn typescript_samples_run_as_unsaved_tabs() {
    let directory = tempfile::tempdir().unwrap();
    let (connections, _) = manager(directory.path());
    let supervisor =
        Supervisor::with_data_dir(directory.path().to_path_buf()).with_connections(connections);
    for sample in samples::all()
        .into_iter()
        .filter(|sample| sample.language == SampleLanguage::Typescript)
    {
        let mut events = Vec::new();
        let result = timeout(
            Duration::from_secs(90),
            supervisor.run_language(&sample.id, sample.source, Language::Typescript, |event| {
                events.push(event);
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("{} timed out", sample.id));
        let result = match result {
            Err(message)
                if message.starts_with("Bun was not found.")
                    && std::env::var_os("CI").is_none() =>
            {
                eprintln!("Install Bun to exercise the TypeScript samples");
                return;
            }
            result => result.unwrap_or_else(|error| panic!("{} failed: {error}", sample.id)),
        };
        assert!(result.ok, "{} failed: {events:?}", sample.id);
        assert!(
            events.iter().any(|event| event.method == "dump"),
            "{} produced no dump",
            sample.id
        );
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
    for (language, expected) in [
        (Language::Python, SampleLanguage::Python),
        (Language::Csharp, SampleLanguage::Csharp),
    ] {
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
