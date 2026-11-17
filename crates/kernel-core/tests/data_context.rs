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

#[tokio::test]
#[ignore = "installs SQLAlchemy and EF Core on the first run"]
async fn sqlite_defaults_fill_unset_columns_in_both_languages() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("defaults.db");
    database(
        &path,
        "CREATE TABLE items (id TEXT PRIMARY KEY DEFAULT 'generated', created TEXT NOT NULL DEFAULT 'now', label TEXT);",
    );
    let context = generated(directory.path(), &path).await;
    let supervisor = Supervisor::default();
    let csharp = run(
        &supervisor,
        Language::Csharp,
        "var context = new DataContext();\nvar item = new Item { Id = \"cs\", Label = \"a\" };\ncontext.Add(item);\ncontext.SaveChanges();\nConsole.WriteLine(item.Created);\n",
        DataContextRun {
            source: context.csharp.clone(),
            connection: context.connection.clone(),
        },
    )
    .await;
    let python = run(
        &supervisor,
        Language::Python,
        "from codingpad_data import DataContext, Item\ncontext = DataContext.connect()\nitem = Item(label='b')\ncontext.session.add(item)\ncontext.session.flush()\nprint(item.id, item.created)\ncontext.session.rollback()\n",
        DataContextRun {
            source: context.python,
            connection: context.connection,
        },
    )
    .await;
    supervisor.shutdown().await;
    assert_eq!(stdout(&csharp).trim(), "now");
    assert_eq!(stdout(&python).trim(), "generated now");
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
    let setup = "-- codingpad: connection=live\nDROP TABLE IF EXISTS codingpad_dc_kinds; DROP TABLE IF EXISTS codingpad_dc_oids; DROP TABLE IF EXISTS codingpad_dc_shapes; DROP TABLE IF EXISTS codingpad_dc_month; DROP TABLE IF EXISTS codingpad_dc_other; DROP TABLE IF EXISTS codingpad_dc_test; DROP FUNCTION IF EXISTS codingpad_dc_next(); DROP SEQUENCE IF EXISTS codingpad_dc_seq; DROP TYPE IF EXISTS codingpad_dc_mood; DROP TYPE IF EXISTS \"Codingpad Odd\"; DROP SCHEMA IF EXISTS codingpad_dc_s CASCADE; CREATE TABLE codingpad_dc_test (id uuid PRIMARY KEY, span interval, spans interval[], addr inet, t timetz, points point[], oid oid); INSERT INTO codingpad_dc_test VALUES ('00000000-0000-0000-0000-000000000001', '1 day 1 hour 0.000001 seconds', ARRAY['1 hour'::interval, '-1 day'::interval], '127.0.0.1', '12:00:00+00', ARRAY[point(1,2)], 42); CREATE TABLE codingpad_dc_other (id int PRIMARY KEY); INSERT INTO codingpad_dc_other VALUES (1); CREATE TABLE codingpad_dc_month (id int PRIMARY KEY, span interval); INSERT INTO codingpad_dc_month VALUES (1, '1 month'); CREATE TYPE codingpad_dc_mood AS ENUM ('ok', 'sad'); CREATE TYPE \"Codingpad Odd\" AS ENUM ('a b', 'c'); CREATE SCHEMA codingpad_dc_s; CREATE TYPE codingpad_dc_s.smood AS ENUM ('x', 'y'); CREATE SEQUENCE codingpad_dc_seq START 700; CREATE FUNCTION codingpad_dc_next() RETURNS int LANGUAGE sql AS 'SELECT nextval(''codingpad_dc_seq'')::int'; CREATE TABLE codingpad_dc_kinds (id int PRIMARY KEY DEFAULT codingpad_dc_next(), mood codingpad_dc_mood, odd \"Codingpad Odd\", moods codingpad_dc_mood[], smood codingpad_dc_s.smood, smoods codingpad_dc_s.smood[], doc xml, price money, prices money[], stamp timestamptz NOT NULL DEFAULT now(), at timestamp, d date, t2 time, span interval, spans interval[], odds \"Codingpad Odd\"[], odd_count int GENERATED ALWAYS AS (cardinality(odds)) STORED); INSERT INTO codingpad_dc_kinds (id, mood, moods) VALUES (1, 'sad', ARRAY['sad']::codingpad_dc_mood[]); CREATE TABLE codingpad_dc_oids (id oid PRIMARY KEY, refs oid[], label text); INSERT INTO codingpad_dc_oids VALUES (42, ARRAY[1,2]::oid[], 'answer'); CREATE TABLE codingpad_dc_shapes (points point[]);";
    manager.run("setup", setup, None).await.unwrap();
    let context = manager.data_context("live").await.unwrap();
    let supervisor = Supervisor::default();
    let result = run_result(
        &supervisor,
        Language::Csharp,
        r#"var context = new DataContext();
Console.WriteLine(context.CodingpadDcOthers.Single().Id);
var row = context.CodingpadDcTests.Single();
Console.WriteLine(row.Addr);
Console.WriteLine(row.Span!.ToDuration() == NodaTime.Duration.FromDays(1) + NodaTime.Duration.FromHours(1) + NodaTime.Duration.FromTicks(10));
Console.WriteLine(row.Spans!.Select(span => span.ToDuration()).SequenceEqual(new[] { NodaTime.Duration.FromHours(1), NodaTime.Duration.FromDays(-1) }));
Console.WriteLine(typeof(CodingpadDcTest).GetProperty("Points") is null);
Console.WriteLine(row.Oid);
var month = context.CodingpadDcMonths.Single();
Console.WriteLine(month.Span);
context.CodingpadDcMonths.ToList().Dump();

// Enums and other types with no cast from text: insert, filter, update.
var mood = "ok";
var odd = "a b";
var smood = "x";
var added = new CodingpadDcKind
{
    Mood = mood, Odd = odd, Moods = new[] { "ok", "sad" }, Smood = smood, Smoods = new[] { "y" }, Doc = "<a/>",
    Price = 1.5m, Prices = new[] { 2.5m },
    At = new DateTime(2024, 1, 2, 3, 4, 5), D = new DateOnly(2024, 1, 2), T2 = new TimeOnly(1, 2, 3),
    Span = NodaTime.Period.FromMonths(2), Spans = new[] { NodaTime.Period.FromYears(1) },
};
context.Add(added);
context.SaveChanges();
Console.WriteLine(added.Id);
Console.WriteLine(added.Stamp is not null);
var fresh = new DataContext();
Console.WriteLine(fresh.CodingpadDcKinds.Count(x => x.Mood == mood));
Console.WriteLine(fresh.CodingpadDcKinds.Count(x => x.Odd == odd));
Console.WriteLine(fresh.CodingpadDcKinds.Count(x => x.Smood == smood));
var moods = new[] { "ok", "sad" };
Console.WriteLine(fresh.CodingpadDcKinds.Count(x => moods.Contains(x.Mood!)));
Console.WriteLine(fresh.CodingpadDcKinds.Count(x => x.Moods!.Contains(mood)));
Console.WriteLine(fresh.CodingpadDcKinds.Count(x => x.Price == 1.5m));
var period = NodaTime.Period.FromMonths(2);
Console.WriteLine(fresh.CodingpadDcKinds.Count(x => x.Span == period));
var saved = fresh.CodingpadDcKinds.Single(x => x.Id == added.Id);
Console.WriteLine($"{saved.Mood}|{saved.Odd}|{string.Join(",", saved.Moods!)}|{saved.Smood}|{saved.Smoods![0]}|{saved.Doc}|{saved.Price}|{saved.Prices![0]}|{saved.At:O}|{saved.D:O}|{saved.T2:O}|{saved.Span}|{saved.Spans![0]}");
saved.Mood = "sad";
saved.Moods![0] = "sad";
fresh.SaveChanges();
var after = new DataContext().CodingpadDcKinds.Single(x => x.Id == added.Id);
Console.WriteLine($"{after.Mood}|{string.Join(",", after.Moods!)}");
fresh.Add(new CodingpadDcKind { Id = 5 });
fresh.SaveChanges();
Console.WriteLine(new DataContext().CodingpadDcKinds.Single(x => x.Id == 5).Mood is null);

// oid key and table left without a mapped column.
var answer = 42u;
Console.WriteLine(fresh.CodingpadDcOids.Single(x => x.Id == answer).Label);
Console.WriteLine(string.Join(",", fresh.CodingpadDcOids.Single().Refs!));
fresh.CodingpadDcOids.Add(new CodingpadDcOid { Id = 43, Refs = new uint[] { 7 }, Label = "added" });
fresh.SaveChanges();
Console.WriteLine(new DataContext().CodingpadDcOids.Single(x => x.Id == 43).Label);
Console.WriteLine(typeof(DataContext).GetProperties().Any(property => property.Name.Contains("Shape")));

// Quoted enum array and a stored generated column.
var quoted = new CodingpadDcKind { Id = 9, Odds = new[] { "a b", "c" } };
fresh.Add(quoted);
fresh.SaveChanges();
Console.WriteLine(quoted.OddCount);
Console.WriteLine(new DataContext().CodingpadDcKinds.Count(x => x.Odds!.Contains(odd)));
quoted.Odds = new[] { "c" };
fresh.SaveChanges();
Console.WriteLine(quoted.OddCount);
Console.WriteLine(string.Join(",", new DataContext().CodingpadDcKinds.Single(x => x.Id == 9).Odds!));
"#,
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
            "-- codingpad: connection=live\nDROP TABLE IF EXISTS codingpad_dc_kinds; DROP TABLE IF EXISTS codingpad_dc_oids; DROP TABLE IF EXISTS codingpad_dc_shapes; DROP TABLE IF EXISTS codingpad_dc_month; DROP TABLE IF EXISTS codingpad_dc_other; DROP TABLE IF EXISTS codingpad_dc_test; DROP FUNCTION IF EXISTS codingpad_dc_next(); DROP SEQUENCE IF EXISTS codingpad_dc_seq; DROP TYPE IF EXISTS codingpad_dc_mood; DROP TYPE IF EXISTS \"Codingpad Odd\"; DROP SCHEMA IF EXISTS codingpad_dc_s CASCADE;",
            None,
        )
        .await
        .unwrap();
    let (ok, events) = result.expect("Postgres data context run failed");
    assert!(ok, "{events:?}");
    assert_eq!(
        stdout(&events).lines().collect::<Vec<_>>(),
        [
            "1",
            "127.0.0.1",
            "True",
            "True",
            "True",
            "42",
            "P1M",
            "700",
            "True",
            "1",
            "1",
            "1",
            "2",
            "1",
            "1",
            "1",
            "ok|a b|ok,sad|x|y|<a/>|1.50|2.50|2024-01-02T03:04:05.0000000|2024-01-02|01:02:03.0000000|P2M|P1Y",
            "sad|sad,sad",
            "True",
            "answer",
            "1,2",
            "added",
            "False",
            "2",
            "1",
            "1",
            "c",
        ]
    );
    assert!(
        format!("{events:?}").contains("P1M"),
        "Dump should show a month interval as text: {events:?}"
    );
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
    let setup = "-- codingpad: connection=live\nIF OBJECT_ID('codingpad_dc_defaults') IS NOT NULL DROP TABLE codingpad_dc_defaults; IF OBJECT_ID('codingpad_dc_child') IS NOT NULL DROP TABLE codingpad_dc_child; IF OBJECT_ID('codingpad_dc_test') IS NOT NULL DROP TABLE codingpad_dc_test; CREATE TABLE codingpad_dc_defaults (id int PRIMARY KEY DEFAULT 700, created datetime2 NOT NULL DEFAULT sysutcdatetime(), label varchar(10), label_length AS (len(label))); CREATE TABLE codingpad_dc_test (id uniqueidentifier PRIMARY KEY, rv rowversion, t time); CREATE TABLE codingpad_dc_child (id int PRIMARY KEY, parent_id uniqueidentifier REFERENCES codingpad_dc_test(id)); INSERT INTO codingpad_dc_test (id, t) VALUES ('00000000-0000-0000-0000-000000000001', '12:00:00'); INSERT INTO codingpad_dc_child VALUES (1, '00000000-0000-0000-0000-000000000001');";
    manager.run("setup", setup, None).await.unwrap();
    let context = manager.data_context("live").await.unwrap();
    let supervisor = Supervisor::default();
    let csharp = run_result(
        &supervisor,
        Language::Csharp,
        "using Microsoft.EntityFrameworkCore;\nvar context = new DataContext();\nvar parent = context.CodingpadDcTests.Include(row => row.CodingpadDcChilds).First(row => row.Id == Guid.Parse(\"00000000-0000-0000-0000-000000000001\"));\nConsole.WriteLine(parent.Id);\nConsole.WriteLine(parent.CodingpadDcChilds.Count);\nvar defaulted = new CodingpadDcDefault { Label = \"cs\" };\ncontext.Add(defaulted);\ncontext.SaveChanges();\nConsole.WriteLine(defaulted.Id);\nConsole.WriteLine(defaulted.Created is not null);\nConsole.WriteLine(defaulted.LabelLength);\ndefaulted.Label = \"abcd\";\ncontext.SaveChanges();\nConsole.WriteLine(defaulted.LabelLength);\ncontext.Remove(defaulted);\ncontext.SaveChanges();\n",
        DataContextRun {
            source: context.csharp.clone(),
            connection: context.connection.clone(),
        },
    )
    .await;
    let python = run_result(
        &supervisor,
        Language::Python,
        "from uuid import UUID\nfrom sqlalchemy import select\nfrom codingpad_data import CodingpadDcDefault, CodingpadDcTest, DataContext\ncontext = DataContext.connect()\nparent = context.session.scalars(select(CodingpadDcTest).where(CodingpadDcTest.id == UUID('00000000-0000-0000-0000-000000000001'))).first()\nprint(parent.id)\nprint(len(parent.codingpad_dc_childs))\ndefaulted = CodingpadDcDefault(label='py')\ncontext.session.add(defaulted)\ncontext.session.flush()\nprint(defaulted.id)\nprint(defaulted.created is not None)\nprint(defaulted.label_length)\ndefaulted.label = 'abcd'\ncontext.session.flush()\nprint(defaulted.label_length)\ncontext.session.rollback()\n",
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
            "-- codingpad: connection=live\nIF OBJECT_ID('codingpad_dc_defaults') IS NOT NULL DROP TABLE codingpad_dc_defaults; IF OBJECT_ID('codingpad_dc_child') IS NOT NULL DROP TABLE codingpad_dc_child; IF OBJECT_ID('codingpad_dc_test') IS NOT NULL DROP TABLE codingpad_dc_test;",
            None,
        )
        .await
        .unwrap();
    let (ok, events) = csharp.expect("SQL Server C# data context run failed");
    assert!(ok, "{events:?}");
    let text = stdout(&events);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            "00000000-0000-0000-0000-000000000001",
            "1",
            "700",
            "True",
            "2",
            "4"
        ]
    );
    let (ok, events) = python.expect("SQL Server Python data context run failed");
    assert!(ok, "{events:?}");
    let text = stdout(&events);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            "00000000-0000-0000-0000-000000000001",
            "1",
            "700",
            "True",
            "2",
            "4"
        ]
    );
}
