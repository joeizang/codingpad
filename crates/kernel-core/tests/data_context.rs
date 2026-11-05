//! Acceptance checks for generated Python and C# data contexts.
//!
//! These are ignored in the normal suite because the first run installs
//! SQLAlchemy/EF Core packages and may need a network or a warm package cache.
//! Run them explicitly with
//! `cargo test -p codingpad-kernel-core --test data_context -- --ignored`.
use codingpad_kernel_core::{
    data_context::GeneratedContext,
    databases::{Connection, ConnectionStore, DatabaseKind, SqlManager},
    secrets::SecretStore,
    DataContextRun, KernelEvent, Language, Supervisor,
};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::time::timeout;

fn sample_database(path: &Path) {
    let database = rusqlite::Connection::open(path).unwrap();
    database
        .execute_batch(
            "CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
             CREATE TABLE orders (
                 id INTEGER PRIMARY KEY,
                 customer_id INTEGER NOT NULL REFERENCES customers(id),
                 total NUMERIC
             );
             INSERT INTO customers (id, name) VALUES (1, 'Ada'), (2, 'Grace');
             INSERT INTO orders (id, customer_id, total) VALUES (10, 1, 12.50);",
        )
        .unwrap();
}

async fn generated(data_dir: &Path, path: &Path) -> GeneratedContext {
    let connections = Arc::new(ConnectionStore::new(data_dir));
    connections
        .save(Connection {
            id: String::new(),
            name: "sample".into(),
            kind: DatabaseKind::Sqlite,
            path: Some(path.to_string_lossy().into_owned()),
            host: None,
            port: None,
            database: None,
            user: None,
            secret: None,
            tls: Default::default(),
            trust_server_certificate: false,
        })
        .unwrap();
    let manager = SqlManager::new(connections, Arc::new(SecretStore::new(data_dir)));
    manager.data_context("sample").await.unwrap()
}

async fn run(
    supervisor: &Supervisor,
    language: Language,
    source: &str,
    data: DataContextRun,
) -> Vec<KernelEvent> {
    let mut events = Vec::new();
    let result = timeout(
        Duration::from_secs(180),
        supervisor.run_language_at_with_data(
            "data-context",
            source,
            language,
            None,
            Some(data),
            |event| events.push(event),
        ),
    )
    .await
    .expect("data context run timed out")
    .expect("data context run failed");
    assert!(result.ok, "{events:?}");
    events
}

fn dumped_text(events: &[KernelEvent]) -> String {
    serde_json::to_string(events).unwrap()
}

#[tokio::test]
#[ignore = "installs SQLAlchemy and runs a Python kernel; run explicitly"]
async fn python_queries_generated_models_and_dumps_the_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sample.db");
    sample_database(&path);
    let context = generated(directory.path(), &path).await;
    let data = DataContextRun {
        source: context.python,
        connection: context.connection,
    };
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        Language::Python,
        "from sqlalchemy import select\nfrom codingpad_data import Customer, DataContext\ncontext = DataContext.connect()\n[customer.name for customer in context.session.scalars(select(Customer)).all()]\n",
        data,
    )
    .await;
    let text = dumped_text(&events);
    assert!(text.contains("Ada"), "{text}");
    assert!(text.contains("Grace"), "{text}");
    supervisor.shutdown().await;
}

#[tokio::test]
#[ignore = "restores EF Core and runs a C# kernel; run explicitly"]
async fn csharp_queries_the_generated_context_and_dumps_the_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sample.db");
    sample_database(&path);
    let context = generated(directory.path(), &path).await;
    let data = DataContextRun {
        source: context.csharp,
        connection: context.connection,
    };
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        Language::Csharp,
        "var context = new DataContext();\ncontext.Customers.OrderBy(customer => customer.Id).Select(customer => customer.Name).ToList().Dump();\n",
        data,
    )
    .await;
    let text = dumped_text(&events);
    assert!(text.contains("Ada"), "{text}");
    assert!(text.contains("Grace"), "{text}");
    supervisor.shutdown().await;
}
