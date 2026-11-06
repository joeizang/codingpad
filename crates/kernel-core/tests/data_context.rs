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

async fn run_result(
    supervisor: &Supervisor,
    language: Language,
    source: &str,
    data: DataContextRun,
) -> Result<(bool, Vec<KernelEvent>), String> {
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
    .map_err(|_| "data context run timed out".to_owned())?
    .map_err(|error| format!("data context run failed: {error}"))?;
    Ok((result.ok, events))
}

async fn run(
    supervisor: &Supervisor,
    language: Language,
    source: &str,
    data: DataContextRun,
) -> Vec<KernelEvent> {
    let (ok, events) = run_result(supervisor, language, source, data)
        .await
        .expect("data context run failed");
    assert!(ok, "{events:?}");
    events
}

fn stdout(events: &[KernelEvent]) -> String {
    events
        .iter()
        .filter(|event| event.method == "stream" && event.params["stream"] == "stdout")
        .filter_map(|event| event.params["text"].as_str())
        .collect()
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
        "from sqlalchemy import select\nfrom codingpad_data import Customer, DataContext\ncontext = DataContext.connect()\nfor customer in context.session.scalars(select(Customer)).all():\n    print(customer.name)\n",
        data,
    )
    .await;
    let text = stdout(&events);
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
        "var context = new DataContext();\nforeach (var customer in context.Customers.OrderBy(customer => customer.Id)) Console.WriteLine(customer.Name);\n",
        data,
    )
    .await;
    let text = stdout(&events);
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
        "using Microsoft.EntityFrameworkCore;\nvar context = new DataContext();\nforeach (var customer in context.Customers.Include(c => c.Orders).Include(c => c.Invoices)) Console.WriteLine(customer.Orders.Count + customer.Invoices.Count);\n",
        DataContextRun {
            source: context.csharp,
            connection: context.connection.clone(),
        },
    )
    .await;
    assert_eq!(stdout(&csharp).trim(), "2");
    let python = run(
        &supervisor,
        Language::Python,
        "from sqlalchemy import select\nfrom codingpad_data import Customer, DataContext\ncontext = DataContext.connect()\nfor customer in context.session.scalars(select(Customer)).all():\n    print(len(customer.orders) + len(customer.invoices))\n",
        DataContextRun {
            source: context.python,
            connection: context.connection,
        },
    )
    .await;
    assert_eq!(stdout(&python).trim(), "2");
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
        "foreach (var log in new DataContext().Logs) Console.WriteLine(log.Level);\n",
        DataContextRun {
            source: context.csharp,
            connection: context.connection,
        },
    )
    .await;
    assert_eq!(stdout(&events).trim(), "info");
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
        "foreach (var order in new DataContext().Orders) Console.WriteLine(order.CustomerCode);\n",
        DataContextRun {
            source: context.csharp,
            connection: context.connection,
        },
    )
    .await;
    assert_eq!(stdout(&events).trim(), "A");
    supervisor.shutdown().await;
}

async fn live_manager(
    kind: DatabaseKind,
    host: String,
    port: u16,
    database: String,
    user: String,
    password: String,
    trust: bool,
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
            trust_server_certificate: trust,
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
        panic!("Set the CODINGPAD_TEST_POSTGRES_* variables to run this test.");
    };
    let (_directory, manager) = live_manager(
        DatabaseKind::Postgres,
        host,
        port.parse().unwrap(),
        database,
        user,
        password,
        false,
    )
    .await;
    let setup = "-- codingpad: connection=live\nDROP TABLE IF EXISTS codingpad_dc_test; CREATE TABLE codingpad_dc_test (id uuid PRIMARY KEY, span interval, addr inet, t timetz); INSERT INTO codingpad_dc_test VALUES ('00000000-0000-0000-0000-000000000001', '1 day', '127.0.0.1', '12:00:00+00');";
    manager.run("setup", setup, None).await.unwrap();
    let context = manager.data_context("live").await.unwrap();
    let supervisor = Supervisor::default();
    let result = run_result(
        &supervisor,
        Language::Csharp,
        "foreach (var row in new DataContext().CodingpadDcTests) Console.WriteLine(row.Addr);\n",
        DataContextRun {
            source: context.csharp,
            connection: context.connection,
        },
    )
    .await;
    supervisor.shutdown().await;
    manager
        .run(
            "cleanup",
            "-- codingpad: connection=live\nDROP TABLE IF EXISTS codingpad_dc_test;",
            None,
        )
        .await
        .unwrap();
    let (ok, events) = result.expect("Postgres data context run failed");
    assert!(ok, "{events:?}");
    assert_eq!(stdout(&events).trim(), "127.0.0.1");
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
        panic!("Set the CODINGPAD_TEST_SQLSERVER_* variables to run this test.");
    };
    let trust = std::env::var("CODINGPAD_TEST_SQLSERVER_TRUST")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
    let (_directory, manager) = live_manager(
        DatabaseKind::SqlServer,
        host,
        port.parse().unwrap(),
        database,
        user,
        password,
        trust,
    )
    .await;
    let setup = "-- codingpad: connection=live\nIF OBJECT_ID('codingpad_dc_child') IS NOT NULL DROP TABLE codingpad_dc_child; IF OBJECT_ID('codingpad_dc_test') IS NOT NULL DROP TABLE codingpad_dc_test; CREATE TABLE codingpad_dc_test (id uniqueidentifier PRIMARY KEY, rv rowversion, t time); CREATE TABLE codingpad_dc_child (id int PRIMARY KEY, parent_id uniqueidentifier REFERENCES codingpad_dc_test(id)); INSERT INTO codingpad_dc_test (id, t) VALUES ('00000000-0000-0000-0000-000000000001', '12:00:00'); INSERT INTO codingpad_dc_child VALUES (1, '00000000-0000-0000-0000-000000000001');";
    manager.run("setup", setup, None).await.unwrap();
    let context = manager.data_context("live").await.unwrap();
    let supervisor = Supervisor::default();
    let csharp = run_result(
        &supervisor,
        Language::Csharp,
        "using Microsoft.EntityFrameworkCore;\nvar context = new DataContext();\nvar parent = context.CodingpadDcTests.Include(row => row.CodingpadDcChilds).First(row => row.Id == Guid.Parse(\"00000000-0000-0000-0000-000000000001\"));\nConsole.WriteLine(parent.Id);\nConsole.WriteLine(parent.CodingpadDcChilds.Count);\n",
        DataContextRun {
            source: context.csharp.clone(),
            connection: context.connection.clone(),
        },
    )
    .await;
    let python = run_result(
        &supervisor,
        Language::Python,
        "from uuid import UUID\nfrom sqlalchemy import select\nfrom codingpad_data import CodingpadDcTest, DataContext\ncontext = DataContext.connect()\nparent = context.session.scalars(select(CodingpadDcTest).where(CodingpadDcTest.id == UUID('00000000-0000-0000-0000-000000000001'))).first()\nprint(parent.id)\nprint(len(parent.codingpad_dc_childs))\n",
        DataContextRun {
            source: context.python,
            connection: context.connection,
        },
    )
    .await;
    supervisor.shutdown().await;
    manager
        .run(
            "cleanup",
            "-- codingpad: connection=live\nIF OBJECT_ID('codingpad_dc_child') IS NOT NULL DROP TABLE codingpad_dc_child; IF OBJECT_ID('codingpad_dc_test') IS NOT NULL DROP TABLE codingpad_dc_test;",
            None,
        )
        .await
        .unwrap();
    let (ok, events) = csharp.expect("SQL Server C# data context run failed");
    assert!(ok, "{events:?}");
    let text = stdout(&events);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines, ["00000000-0000-0000-0000-000000000001", "1"]);
    let (ok, events) = python.expect("SQL Server Python data context run failed");
    assert!(ok, "{events:?}");
    let text = stdout(&events);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines, ["00000000-0000-0000-0000-000000000001", "1"]);
}
