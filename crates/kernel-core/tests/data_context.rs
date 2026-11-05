//! Acceptance checks for generated Python and C# data contexts.
//!
//! These are ignored in the normal suite because the first run installs
//! SQLAlchemy/EF Core packages and may need a network or a warm package cache.
//! Run them explicitly with
//! `cargo test -p codingpad-kernel-core --test data_context -- --ignored`.
use codingpad_kernel_core::{
    data_context::GeneratedContext,
    databases::{Connection, ConnectionStore, DatabaseKind, SqlManager, TlsMode},
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

fn database(path: &Path, sql: &str) {
    let database = rusqlite::Connection::open(path).unwrap();
    database.execute_batch(sql).unwrap();
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

#[tokio::test]
#[ignore = "installs SQLAlchemy/EF Core and runs kernels; run explicitly"]
async fn two_children_pointing_at_one_parent_work_in_both_languages() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sample.db");
    database(
        &path,
        "CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT);
         CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id));
         CREATE TABLE invoices (id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id));
         INSERT INTO customers VALUES (1, 'Ada');
         INSERT INTO orders VALUES (10, 1);
         INSERT INTO invoices VALUES (20, 1);",
    );
    let context = generated(directory.path(), &path).await;
    let supervisor = Supervisor::default();
    let csharp = run(
        &supervisor,
        Language::Csharp,
        "var context = new DataContext();\ncontext.Customers.Select(c => c.Orders.Count + c.Invoices.Count).ToList().Dump();\n",
        DataContextRun { source: context.csharp, connection: context.connection.clone() },
    )
    .await;
    assert!(serde_json::to_string(&csharp).unwrap().contains("2"));
    let python = run(
        &supervisor,
        Language::Python,
        "from sqlalchemy import select\nfrom codingpad_data import Customer, DataContext\ncontext = DataContext.connect()\n[len(c.orders) + len(c.invoices) for c in context.session.scalars(select(Customer)).all()]\n",
        DataContextRun { source: context.python, connection: context.connection },
    )
    .await;
    assert!(serde_json::to_string(&python).unwrap().contains("2"));
    supervisor.shutdown().await;
}

#[tokio::test]
#[ignore = "installs SQLAlchemy/EF Core and runs kernels; run explicitly"]
async fn keyless_child_with_a_foreign_key_does_not_break_the_context() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sample.db");
    database(
        &path,
        "CREATE TABLE customers (id INTEGER PRIMARY KEY);
         CREATE TABLE logs (level TEXT, customer_id INTEGER REFERENCES customers(id));
         INSERT INTO customers VALUES (1);
         INSERT INTO logs VALUES ('info', 1);",
    );
    let context = generated(directory.path(), &path).await;
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        Language::Csharp,
        "new DataContext().Logs.ToList().Dump();\n",
        DataContextRun {
            source: context.csharp,
            connection: context.connection,
        },
    )
    .await;
    assert!(serde_json::to_string(&events).unwrap().contains("info"));
    supervisor.shutdown().await;
}

#[tokio::test]
#[ignore = "installs SQLAlchemy/EF Core and runs kernels; run explicitly"]
async fn foreign_key_to_a_unique_non_primary_key_works() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sample.db");
    database(
        &path,
        "CREATE TABLE customers (id INTEGER PRIMARY KEY, code TEXT UNIQUE);
         CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_code TEXT REFERENCES customers(code));
         INSERT INTO customers VALUES (1, 'A');
         INSERT INTO orders VALUES (10, 'A');",
    );
    let context = generated(directory.path(), &path).await;
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        Language::Csharp,
        "new DataContext().Orders.ToList().Dump();\n",
        DataContextRun {
            source: context.csharp,
            connection: context.connection,
        },
    )
    .await;
    assert!(serde_json::to_string(&events).unwrap().contains("A"));
    supervisor.shutdown().await;
}

async fn live_manager(
    kind: DatabaseKind,
    host: String,
    port: u16,
    database: String,
    user: String,
    password: String,
) -> (tempfile::TempDir, SqlManager) {
    let directory = tempfile::tempdir().unwrap();
    let secrets = Arc::new(SecretStore::new(directory.path()));
    secrets.set_session("connection:live", &password).unwrap();
    let connections = Arc::new(ConnectionStore::new(directory.path()));
    connections
        .save(Connection {
            id: String::new(),
            name: "live".into(),
            kind,
            path: None,
            host: Some(host),
            port: Some(port),
            database: Some(database),
            user: Some(user),
            secret: Some("connection:live".into()),
            tls: TlsMode::Prefer,
            trust_server_certificate: false,
        })
        .unwrap();
    let manager = SqlManager::new(connections, secrets);
    (directory, manager)
}

#[tokio::test]
#[ignore = "requires CODINGPAD_TEST_POSTGRES_* and installs SQLAlchemy/EF Core"]
async fn postgres_live_types_work_through_generated_models() {
    let (Ok(host), Ok(port), Ok(database), Ok(user), Ok(password)) = (
        std::env::var("CODINGPAD_TEST_POSTGRES_HOST"),
        std::env::var("CODINGPAD_TEST_POSTGRES_PORT"),
        std::env::var("CODINGPAD_TEST_POSTGRES_DATABASE"),
        std::env::var("CODINGPAD_TEST_POSTGRES_USER"),
        std::env::var("CODINGPAD_TEST_POSTGRES_PASSWORD"),
    ) else {
        eprintln!("Set the CODINGPAD_TEST_POSTGRES_* variables to run this test.");
        return;
    };
    let (_directory, manager) = live_manager(
        DatabaseKind::Postgres,
        host,
        port.parse().unwrap(),
        database,
        user,
        password,
    )
    .await;
    let setup = "-- codingpad: connection=live\nDROP TABLE IF EXISTS codingpad_dc_test; CREATE TABLE codingpad_dc_test (id uuid PRIMARY KEY, span interval, addr inet, t timetz); INSERT INTO codingpad_dc_test VALUES ('00000000-0000-0000-0000-000000000001', '1 day', '127.0.0.1', '12:00:00+00');";
    manager.run("setup", setup, None).await.unwrap();
    let context = manager.data_context("live").await.unwrap();
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        Language::Csharp,
        "new DataContext().CodingpadDcTests.ToList().Dump();\n",
        DataContextRun {
            source: context.csharp,
            connection: context.connection,
        },
    )
    .await;
    assert!(serde_json::to_string(&events)
        .unwrap()
        .contains("127.0.0.1"));
    supervisor.shutdown().await;
    manager
        .run(
            "cleanup",
            "-- codingpad: connection=live\nDROP TABLE IF EXISTS codingpad_dc_test;",
            None,
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CODINGPAD_TEST_SQLSERVER_* and installs SQLAlchemy/EF Core"]
async fn sqlserver_live_types_work_through_generated_models() {
    let (Ok(host), Ok(port), Ok(database), Ok(user), Ok(password)) = (
        std::env::var("CODINGPAD_TEST_SQLSERVER_HOST"),
        std::env::var("CODINGPAD_TEST_SQLSERVER_PORT"),
        std::env::var("CODINGPAD_TEST_SQLSERVER_DATABASE"),
        std::env::var("CODINGPAD_TEST_SQLSERVER_USER"),
        std::env::var("CODINGPAD_TEST_SQLSERVER_PASSWORD"),
    ) else {
        eprintln!("Set the CODINGPAD_TEST_SQLSERVER_* variables to run this test.");
        return;
    };
    let (_directory, manager) = live_manager(
        DatabaseKind::SqlServer,
        host,
        port.parse().unwrap(),
        database,
        user,
        password,
    )
    .await;
    let setup = "-- codingpad: connection=live\nIF OBJECT_ID('codingpad_dc_test') IS NOT NULL DROP TABLE codingpad_dc_test; CREATE TABLE codingpad_dc_test (id uniqueidentifier PRIMARY KEY, rv rowversion, t time); INSERT INTO codingpad_dc_test (id, t) VALUES ('00000000-0000-0000-0000-000000000001', '12:00:00');";
    manager.run("setup", setup, None).await.unwrap();
    let context = manager.data_context("live").await.unwrap();
    let supervisor = Supervisor::default();
    let events = run(
        &supervisor,
        Language::Csharp,
        "new DataContext().CodingpadDcTests.ToList().Dump();\n",
        DataContextRun {
            source: context.csharp,
            connection: context.connection,
        },
    )
    .await;
    assert!(serde_json::to_string(&events)
        .unwrap()
        .contains("00000000-0000-0000-0000-000000000001"));
    supervisor.shutdown().await;
    manager
        .run(
            "cleanup",
            "-- codingpad: connection=live\nIF OBJECT_ID('codingpad_dc_test') IS NOT NULL DROP TABLE codingpad_dc_test;",
            None,
        )
        .await
        .unwrap();
}
