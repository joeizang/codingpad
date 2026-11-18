//! Built-in sample scripts and the bundled SQLite sample database (D14).
//!
//! Samples are compiled into the app so they cannot be changed accidentally by
//! opening or saving them. Opening a sample creates an unsaved copy in a tab;
//! the original embedded source is never written back. The database is copied
//! into the app data folder on startup and exposed through a saved connection,
//! so SQL, Python and C# samples can query it without any setup.

use crate::databases::{Connection, ConnectionStore, DatabaseKind};
use serde::Serialize;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// The connection name used by the bundled database and the sample headers.
pub const SAMPLE_CONNECTION: &str = "sample";
/// The database file placed in the app data folder.
pub const SAMPLE_DATABASE_FILE: &str = "sample.sqlite";

/// A built-in script shown in the app's Samples section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    /// Stable identifier, also used as the tab's sample marker.
    pub id: &'static str,
    /// File name shown for the sample, including its extension.
    pub name: &'static str,
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
    Sql,
}

impl SampleLanguage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Python => "Python",
            Self::Csharp => "C#",
            Self::Sql => "SQL",
        }
    }
}

const DATABASE: &[u8] = include_bytes!("../../../samples/sample.sqlite");

macro_rules! sample {
    ($id:literal, $name:literal, $title:literal, $language:ident, $path:literal) => {
        Sample {
            id: $id,
            name: $name,
            title: $title,
            language: SampleLanguage::$language,
            source: include_str!(concat!("../../../samples/", $path)),
        }
    };
}

/// Every built-in sample, grouped by language in the order the app shows them.
pub fn all() -> Vec<Sample> {
    vec![
        sample!(
            "python/hello",
            "hello.py",
            "Hello, world!",
            Python,
            "python/hello.py"
        ),
        sample!(
            "python/values",
            "values.py",
            "Values and dumps",
            Python,
            "python/values.py"
        ),
        sample!(
            "python/tables",
            "tables.py",
            "Tables from Python data",
            Python,
            "python/tables.py"
        ),
        sample!(
            "python/collections",
            "collections.py",
            "Collections and iteration",
            Python,
            "python/collections.py"
        ),
        sample!(
            "python/text",
            "text.py",
            "Text, dates and JSON",
            Python,
            "python/text.py"
        ),
        sample!(
            "python/functions",
            "functions.py",
            "Functions and cache()",
            Python,
            "python/functions.py"
        ),
        sample!(
            "python/live",
            "live.py",
            "Live values",
            Python,
            "python/live.py"
        ),
        sample!(
            "python/html",
            "html.py",
            "HTML output",
            Python,
            "python/html.py"
        ),
        sample!(
            "python/query-products",
            "query_products.py",
            "Query the sample database",
            Python,
            "python/query_products.py"
        ),
        sample!(
            "python/query-orders",
            "query_orders.py",
            "Join the sample database",
            Python,
            "python/query_orders.py"
        ),
        sample!(
            "csharp/hello",
            "hello.cs",
            "Hello, world!",
            Csharp,
            "csharp/hello.cs"
        ),
        sample!(
            "csharp/objects",
            "objects.cs",
            "Objects and nested values",
            Csharp,
            "csharp/objects.cs"
        ),
        sample!(
            "csharp/tables",
            "tables.cs",
            "Tables from sequences",
            Csharp,
            "csharp/tables.cs"
        ),
        sample!(
            "csharp/collections",
            "collections.cs",
            "Collections and LINQ",
            Csharp,
            "csharp/collections.cs"
        ),
        sample!(
            "csharp/linq",
            "linq.cs",
            "Group and aggregate",
            Csharp,
            "csharp/linq.cs"
        ),
        sample!(
            "csharp/async",
            "async.cs",
            "Async work",
            Csharp,
            "csharp/async.cs"
        ),
        sample!(
            "csharp/cache",
            "cache.cs",
            "Cache a slow result",
            Csharp,
            "csharp/cache.cs"
        ),
        sample!(
            "csharp/rich-output",
            "rich_output.cs",
            "Live values and HTML",
            Csharp,
            "csharp/rich_output.cs"
        ),
        sample!(
            "csharp/query-products",
            "query_products.cs",
            "Query the sample database",
            Csharp,
            "csharp/query_products.cs"
        ),
        sample!(
            "csharp/query-orders",
            "query_orders.cs",
            "Join the sample database",
            Csharp,
            "csharp/query_orders.cs"
        ),
        sample!(
            "sql/hello",
            "hello.sql",
            "Hello from SQL",
            Sql,
            "sql/hello.sql"
        ),
        sample!(
            "sql/tables",
            "tables.sql",
            "Explore the database",
            Sql,
            "sql/tables.sql"
        ),
        sample!(
            "sql/customers",
            "customers.sql",
            "Customers",
            Sql,
            "sql/customers.sql"
        ),
        sample!(
            "sql/products",
            "products.sql",
            "Products and categories",
            Sql,
            "sql/products.sql"
        ),
        sample!(
            "sql/orders",
            "orders.sql",
            "Orders and totals",
            Sql,
            "sql/orders.sql"
        ),
        sample!(
            "sql/order-details",
            "order_details.sql",
            "Order line items",
            Sql,
            "sql/order_details.sql"
        ),
        sample!(
            "sql/top-customers",
            "top_customers.sql",
            "Best customers",
            Sql,
            "sql/top_customers.sql"
        ),
        sample!(
            "sql/category-sales",
            "category_sales.sql",
            "Revenue by category",
            Sql,
            "sql/category_sales.sql"
        ),
        sample!(
            "sql/low-stock",
            "low_stock.sql",
            "Low stock",
            Sql,
            "sql/low_stock.sql"
        ),
        sample!(
            "sql/monthly-sales",
            "monthly_sales.sql",
            "Monthly sales",
            Sql,
            "sql/monthly_sales.sql"
        ),
    ]
}

/// The bundled database bytes, exposed for tests and packaging checks.
pub fn database_bytes() -> &'static [u8] {
    DATABASE
}

/// Write the bundled database into the app data folder, replacing an older
/// copy when the embedded bytes differ. The sample database contains no user
/// data, so replacing it keeps the samples working after an app update.
pub fn ensure_database(data_dir: &Path) -> Result<PathBuf, String> {
    let directory = data_dir.join("samples");
    let path = directory.join(SAMPLE_DATABASE_FILE);
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
    if fs::read(&path).ok().as_deref() == Some(DATABASE) {
        return Ok(path);
    }
    let mut temp = tempfile::NamedTempFile::new_in(&directory)
        .map_err(|error| format!("Could not create a temporary sample database: {error}"))?;
    temp.write_all(DATABASE)
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    temp.persist(&path)
        .map_err(|error| format!("Could not save {}: {}", path.display(), error.error))?;
    Ok(path)
}

/// Prepare the bundled database and make it available as the ready `sample`
/// connection. An existing connection with that name is left alone unless it
/// already points into CodingPad's own samples folder (for example after an
/// app update moved the file).
pub fn prepare(data_dir: &Path, connections: &ConnectionStore) -> Result<PathBuf, String> {
    let path = ensure_database(data_dir)?;
    let sample_path = path.to_string_lossy().into_owned();
    let previous = connections.by_name(SAMPLE_CONNECTION);
    if let Some(existing) = &previous {
        if existing.kind == DatabaseKind::Sqlite
            && existing.path.as_deref().map(Path::new) == Some(path.as_path())
        {
            return Ok(path);
        }
        let ours = existing
            .path
            .as_deref()
            .is_some_and(|saved| Path::new(saved).starts_with(data_dir.join("samples")));
        if !ours {
            // A user connection already uses the name "sample"; do not replace
            // it. The bundled database is still on disk for manual connection.
            return Ok(path);
        }
    }
    connections.save(Connection {
        id: previous.map(|connection| connection.id).unwrap_or_default(),
        name: SAMPLE_CONNECTION.to_owned(),
        kind: DatabaseKind::Sqlite,
        path: Some(sample_path),
        host: None,
        port: None,
        database: None,
        user: None,
        secret: None,
        tls: Default::default(),
        trust_server_certificate: false,
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_have_unique_ids_and_matching_extensions() {
        let samples = all();
        assert!(
            samples
                .iter()
                .filter(|sample| sample.language == SampleLanguage::Python)
                .count()
                >= 10
        );
        assert!(
            samples
                .iter()
                .filter(|sample| sample.language == SampleLanguage::Csharp)
                .count()
                >= 10
        );
        assert!(
            samples
                .iter()
                .filter(|sample| sample.language == SampleLanguage::Sql)
                .count()
                >= 10
        );
        let mut ids = std::collections::HashSet::new();
        for sample in samples {
            assert!(ids.insert(sample.id), "duplicate sample id {}", sample.id);
            let expected = match sample.language {
                SampleLanguage::Python => ".py",
                SampleLanguage::Csharp => ".cs",
                SampleLanguage::Sql => ".sql",
            };
            assert!(sample.name.ends_with(expected), "{}", sample.name);
            assert!(!sample.source.trim().is_empty(), "{}", sample.id);
        }
    }

    #[test]
    fn bundled_database_is_a_sqlite_file_with_the_sample_schema() {
        assert!(DATABASE.starts_with(b"SQLite format 3\0"));
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
}
