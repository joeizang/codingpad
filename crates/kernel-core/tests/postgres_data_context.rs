//! Live generated-model regression checks. Run explicitly with CODINGPAD_TEST_POSTGRES_*.
use codingpad_kernel_core::{
    databases::{Connection, ConnectionStore, DatabaseKind, SqlManager, TlsMode},
    secrets::SecretStore,
    DataContextRun, Language, Supervisor,
};
use std::{sync::Arc, time::Duration};

#[tokio::test]
#[ignore = "requires CODINGPAD_TEST_POSTGRES_* and installs SQLAlchemy"]
async fn python_postgres_enum_arrays_read_and_bind_without_corruption() {
    let directory = tempfile::tempdir().unwrap();
    let secrets = Arc::new(SecretStore::new(directory.path()));
    secrets
        .set_session(
            "connection:live",
            &std::env::var("CODINGPAD_TEST_POSTGRES_PASSWORD").unwrap(),
        )
        .unwrap();
    let connections = Arc::new(ConnectionStore::new(directory.path()));
    connections
        .save(Connection {
            id: String::new(),
            name: "live".into(),
            kind: DatabaseKind::Postgres,
            host: Some(std::env::var("CODINGPAD_TEST_POSTGRES_HOST").unwrap()),
            port: Some(
                std::env::var("CODINGPAD_TEST_POSTGRES_PORT")
                    .unwrap()
                    .parse()
                    .unwrap(),
            ),
            database: Some(std::env::var("CODINGPAD_TEST_POSTGRES_DATABASE").unwrap()),
            user: Some(std::env::var("CODINGPAD_TEST_POSTGRES_USER").unwrap()),
            path: None,
            secret: Some("connection:live".into()),
            tls: TlsMode::Prefer,
            trust_server_certificate: false,
        })
        .unwrap();
    let manager = SqlManager::new(connections, secrets);
    let setup = r#"-- codingpad: connection=live
DROP TABLE IF EXISTS codingpad_array_test;
DROP TYPE IF EXISTS codingpad_mood;
DROP TYPE IF EXISTS "Odd Mood";
CREATE TYPE codingpad_mood AS ENUM ('ok', 'so, so', 'NULL', '', 'a"b', 'a\b');
CREATE TYPE "Odd Mood" AS ENUM ('a b', 'c');
CREATE TABLE codingpad_array_test (id int PRIMARY KEY, moods codingpad_mood[], odd "Odd Mood"[], points point[], price money, prices money[]);
INSERT INTO codingpad_array_test VALUES
(1, ARRAY['ok', 'so, so', NULL, 'NULL', '', 'a"b', 'a\b']::codingpad_mood[], ARRAY['a b', 'c']::"Odd Mood"[], ARRAY[point(1,2), point(3,4)], 1234.50, ARRAY[1.50, NULL, 1234.50]::money[]),
(2, '{}', '{}', '{}', NULL, '{}'), (3, NULL, NULL, NULL, NULL, NULL);
"#;
    manager.run("setup", setup, None).await.unwrap();
    let context = manager.data_context("live").await.unwrap();
    let supervisor = Supervisor::default();
    let source = r#"from decimal import Decimal
from sqlalchemy import select
from codingpad_data import CodingpadArrayTest, DataContext
context = DataContext.connect()
rows = context.session.scalars(select(CodingpadArrayTest).order_by(CodingpadArrayTest.id)).all()
expected = ['ok', 'so, so', None, 'NULL', '', 'a"b', 'a\\b']
assert rows[0].moods == expected, repr(rows[0].moods)
assert rows[0].odd == ['a b', 'c'], repr(rows[0].odd)
assert rows[0].points == ['(1,2)', '(3,4)'], repr(rows[0].points)
assert rows[1].moods == [] and rows[1].odd == [] and rows[1].points == []
assert rows[2].moods is None and rows[2].odd is None
assert rows[0].price == Decimal('1234.50') and rows[0].prices == [Decimal('1.50'), None, Decimal('1234.50')]
for value in [Decimal('1234.50'), '1234.50']:
    assert context.session.scalar(select(CodingpadArrayTest.id).where(CodingpadArrayTest.price == value)) == 1
assert context.session.scalar(select(CodingpadArrayTest.id).where(CodingpadArrayTest.prices == [Decimal('1.50'), None, Decimal('1234.50')])) == 1
for attr, value in [('moods', expected), ('odd', ['a b', 'c'])]:
    assert context.session.scalar(select(CodingpadArrayTest.id).where(getattr(CodingpadArrayTest, attr) == value)) == 1
context.session.add(CodingpadArrayTest(id=4, moods=expected, odd=['a b', 'c'], points=['(1,2)', '(3,4)'], price=Decimal('1.50'), prices=[Decimal('1234.50'), None]))
context.session.flush()
context.session.expire_all()
inserted = context.session.get(CodingpadArrayTest, 4)
assert inserted.moods == expected and inserted.odd == ['a b', 'c']
assert inserted.points == ['(1,2)', '(3,4)']
assert inserted.price == Decimal('1.50') and inserted.prices == [Decimal('1234.50'), None]
context.session.rollback()
print('arrays read and bind correctly')
"#;
    let mut events = Vec::new();
    let result = tokio::time::timeout(
        Duration::from_secs(180),
        supervisor.run_language_at_with_data(
            "postgres-arrays",
            source,
            Language::Python,
            None,
            Some(DataContextRun {
                source: context.python,
                connection: context.connection,
            }),
            |event| events.push(event),
        ),
    )
    .await;
    supervisor.shutdown().await;
    manager.run("cleanup", "-- codingpad: connection=live\nDROP TABLE codingpad_array_test; DROP TYPE codingpad_mood; DROP TYPE \"Odd Mood\";", None).await.unwrap();
    assert!(result.unwrap().unwrap().ok, "{events:?}");
}
