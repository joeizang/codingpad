//! Built-in sample scripts and the bundled SQLite sample database (D14).
//!
//! Samples are compiled into the app so they cannot be changed accidentally by
//! opening or saving them. Opening a sample creates an unsaved copy in a tab;
//! the original embedded source is never written back. The database is copied
//! into the app data folder on first use and exposed as a built-in connection,
//! so SQL, Python, C# and TypeScript samples can query it without any setup.
//!
//! The built-in connection is resolved by [`crate::databases::ConnectionStore`]
//! and is never written to `connections.json`. That keeps `cpad` from rewriting
//! the desktop's connection file and lets users delete or change their own
//! connections without the sample coming back.

use crate::databases::{Connection, DatabaseKind};
use serde::Serialize;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// The reserved connection name used by the bundled database and sample headers.
pub const SAMPLE_CONNECTION: &str = "codingpad-sample";
/// The database file placed in the app data folder. Bump the version whenever
/// the embedded database changes so existing installs get the new copy.
pub const SAMPLE_DATABASE_FILE: &str = "sample-v1.sqlite";

/// A built-in script shown in the app's Samples section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    /// Stable identifier, also used as the tab's sample marker.
    pub id: String,
    /// File name shown for the sample, including its extension.
    pub name: String,
    /// Short title shown in the sidebar.
    pub title: &'static str,
    pub language: SampleLanguage,
    pub source: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SampleLanguage {
    Python,
    Csharp,
    Typescript,
    Sql,
}

const DATABASE: &[u8] = include_bytes!("../../../samples/sample.sqlite");

fn sample_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

fn sample_id(path: &str) -> String {
    path.rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(path)
        .replace('_', "-")
}

macro_rules! sample {
    ($path:literal, $title:literal, $language:ident) => {
        Sample {
            id: sample_id($path),
            name: sample_name($path),
            title: $title,
            language: SampleLanguage::$language,
            source: include_str!(concat!("../../../samples/", $path)),
        }
    };
}

/// Every built-in sample, grouped by language in the order the app shows them.
pub fn all() -> Vec<Sample> {
    vec![
        sample!("python/hello.py", "Hello, world!", Python),
        sample!("python/values.py", "Values and dumps", Python),
        sample!("python/tables.py", "Tables from Python data", Python),
        sample!("python/collections.py", "Collections and iteration", Python),
        sample!("python/text.py", "Text, dates and JSON", Python),
        sample!("python/functions.py", "Functions and cache()", Python),
        sample!("python/live.py", "Live values", Python),
        sample!("python/html.py", "HTML output", Python),
        sample!(
            "python/query_products.py",
            "Query the sample database",
            Python
        ),
        sample!("python/query_orders.py", "Join the sample database", Python),
        sample!("csharp/hello.cs", "Hello, world!", Csharp),
        sample!("csharp/objects.cs", "Objects and nested values", Csharp),
        sample!("csharp/tables.cs", "Tables from sequences", Csharp),
        sample!("csharp/collections.cs", "Collections and LINQ", Csharp),
        sample!("csharp/linq.cs", "Group and aggregate", Csharp),
        sample!("csharp/async.cs", "Async work", Csharp),
        sample!("csharp/cache.cs", "Cache a slow result", Csharp),
        sample!("csharp/rich_output.cs", "Live values and HTML", Csharp),
        sample!(
            "csharp/query_products.cs",
            "Query the sample database",
            Csharp
        ),
        sample!("csharp/query_orders.cs", "Join the sample database", Csharp),
        sample!("typescript/hello.ts", "Hello, world!", Typescript),
        sample!(
            "typescript/objects.ts",
            "Objects and nested values",
            Typescript
        ),
        sample!(
            "typescript/tables.ts",
            "Tables from TypeScript data",
            Typescript
        ),
        sample!("typescript/collections.ts", "Maps and sets", Typescript),
        sample!("typescript/text.ts", "Text, dates and JSON", Typescript),
        sample!("typescript/async.ts", "Top-level await", Typescript),
        sample!("typescript/cache.ts", "Cache a result", Typescript),
        sample!(
            "typescript/rich_output.ts",
            "Live values and HTML",
            Typescript
        ),
        sample!(
            "typescript/query_products.ts",
            "Query the sample database",
            Typescript
        ),
        sample!(
            "typescript/query_orders.ts",
            "Join the sample database",
            Typescript
        ),
        sample!("sql/hello.sql", "Hello from SQL", Sql),
        sample!("sql/tables.sql", "Explore the database", Sql),
        sample!("sql/customers.sql", "Customers", Sql),
        sample!("sql/products.sql", "Products and categories", Sql),
        sample!("sql/orders.sql", "Orders and totals", Sql),
        sample!("sql/order_details.sql", "Order line items", Sql),
        sample!("sql/top_customers.sql", "Best customers", Sql),
        sample!("sql/category_sales.sql", "Revenue by category", Sql),
        sample!("sql/low_stock.sql", "Low stock", Sql),
        sample!("sql/monthly_sales.sql", "Monthly sales", Sql),
    ]
}

/// The built-in connection that points at the bundled database.
pub fn connection(data_dir: &Path) -> Connection {
    Connection {
        id: SAMPLE_CONNECTION.to_owned(),
        name: SAMPLE_CONNECTION.to_owned(),
        kind: DatabaseKind::Sqlite,
        path: Some(
            data_dir
                .join("samples")
                .join(SAMPLE_DATABASE_FILE)
                .to_string_lossy()
                .into_owned(),
        ),
        host: None,
        port: None,
        database: None,
        user: None,
        secret: None,
        tls: Default::default(),
        trust_server_certificate: false,
    }
}

/// The bundled database bytes, exposed for tests and packaging checks.
pub fn database_bytes() -> &'static [u8] {
    DATABASE
}

/// Write the bundled database into the app data folder when it is missing.
///
/// The file is never replaced: users may add their own rows through the sample
/// connection, and replacing the file under an open warm kernel loses those
/// writes (and fails on Windows while the file is open). A new bundled version
/// changes [`SAMPLE_DATABASE_FILE`], so an update copies a new file under a new
/// name instead of overwriting the old one.
pub fn ensure_database(data_dir: &Path) -> Result<PathBuf, String> {
    let directory = data_dir.join("samples");
    let path = directory.join(SAMPLE_DATABASE_FILE);
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
    if path.exists() {
        return Ok(path);
    }
    // A journal or WAL can outlive a database that was deleted between runs.
    // Leaving it beside a freshly written file lets SQLite roll old bytes onto
    // the new database.
    for suffix in ["-journal", "-wal", "-shm"] {
        let sidecar = directory.join(format!("{SAMPLE_DATABASE_FILE}{suffix}"));
        let _ = fs::remove_file(sidecar);
    }
    let mut temp = tempfile::NamedTempFile::new_in(&directory)
        .map_err(|error| format!("Could not create a temporary sample database: {error}"))?;
    temp.write_all(DATABASE)
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    match temp.persist_noclobber(&path) {
        Ok(_) => {
            remove_old_databases(&directory);
            Ok(path)
        }
        // Another app or `cpad` process created the file first.
        Err(_) if path.exists() => Ok(path),
        Err(error) => Err(format!(
            "Could not save {}: {}",
            path.display(),
            error.error
        )),
    }
}

/// Remove databases from earlier bundled versions after a new version has
/// been written. Failures are ignored: another process may still have an old
/// file open, especially on Windows.
fn remove_old_databases(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == SAMPLE_DATABASE_FILE || !is_versioned_database(&name) {
            continue;
        }
        let path = entry.path();
        if fs::remove_file(&path).is_ok() {
            for suffix in ["-journal", "-wal", "-shm"] {
                let _ = fs::remove_file(directory.join(format!("{name}{suffix}")));
            }
        }
    }
}

fn is_versioned_database(name: &str) -> bool {
    let Some(version) = name
        .strip_prefix("sample-v")
        .and_then(|rest| rest.strip_suffix(".sqlite"))
    else {
        return false;
    };
    !version.is_empty() && version.chars().all(|character| character.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::databases::{ConnectionStore, DatabaseKind};

    #[test]
    fn bundled_database_is_a_sqlite_file_with_the_sample_schema() {
        assert!(database_bytes().starts_with(b"SQLite format 3\0"));
        let path = tempfile::tempdir().unwrap();
        let path = ensure_database(path.path()).unwrap();
        let database = rusqlite::Connection::open(path).unwrap();
        let count: i64 = database
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(count >= 5);
        let orders: i64 = database
            .query_row("SELECT count(*) FROM orders", [], |row| row.get(0))
            .unwrap();
        assert_eq!(orders, 10);
    }

    #[test]
    fn an_existing_database_is_never_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = ensure_database(directory.path()).unwrap();
        fs::write(&path, b"user changes").unwrap();
        assert_eq!(ensure_database(directory.path()).unwrap(), path);
        assert_eq!(fs::read(&path).unwrap(), b"user changes");
    }

    #[test]
    fn stale_journals_are_removed_when_the_database_is_created() {
        let directory = tempfile::tempdir().unwrap();
        let samples = directory.path().join("samples");
        fs::create_dir_all(&samples).unwrap();
        for suffix in ["-journal", "-wal", "-shm"] {
            fs::write(
                samples.join(format!("{SAMPLE_DATABASE_FILE}{suffix}")),
                b"stale",
            )
            .unwrap();
        }
        ensure_database(directory.path()).unwrap();
        for suffix in ["-journal", "-wal", "-shm"] {
            assert!(!samples
                .join(format!("{SAMPLE_DATABASE_FILE}{suffix}"))
                .exists());
        }
    }

    #[test]
    fn old_bundled_versions_are_removed_after_a_new_one_is_written() {
        let directory = tempfile::tempdir().unwrap();
        let samples = directory.path().join("samples");
        fs::create_dir_all(&samples).unwrap();
        for name in [
            "sample-v2.sqlite",
            "sample-v2.sqlite-wal",
            "sample-v10.sqlite",
            "notes.sqlite",
            "sample-v2.backup",
        ] {
            fs::write(samples.join(name), b"old").unwrap();
        }
        ensure_database(directory.path()).unwrap();
        assert!(!samples.join("sample-v2.sqlite").exists());
        assert!(!samples.join("sample-v2.sqlite-wal").exists());
        assert!(!samples.join("sample-v10.sqlite").exists());
        assert!(samples.join(SAMPLE_DATABASE_FILE).exists());
        assert!(samples.join("notes.sqlite").exists());
        assert!(samples.join("sample-v2.backup").exists());
    }

    #[test]
    fn the_sample_connection_is_built_in_and_never_persisted() {
        let directory = tempfile::tempdir().unwrap();
        let path = ensure_database(directory.path()).unwrap();
        let connections = ConnectionStore::new(directory.path());
        let sample = connections.by_name(SAMPLE_CONNECTION).unwrap();
        assert_eq!(sample.kind, DatabaseKind::Sqlite);
        assert_eq!(sample.path.as_deref(), path.to_str());
        assert!(connections.list().is_empty());
        assert!(!directory.path().join("connections.json").exists());
        assert!(connections
            .save(Connection {
                name: SAMPLE_CONNECTION.to_owned(),
                ..sample
            })
            .is_err());
    }
}
