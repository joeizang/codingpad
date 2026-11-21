//! A database session owned by a single native Rust script run.
use super::{
    affected_rows_table, execute_sqlite, has_go_batch, pg_error, postgres_simple_tables,
    read_sqlserver_tables, sqlite_error, sqlite_path, sqlite_statement, Connection, DatabaseKind,
    PostgresSession, SecretStore, SqlManager, SqlServerClient, SqliteConnection, TableData,
    STOPPED,
};
use bytes::BytesMut;
use serde::Deserialize;
use std::{
    borrow::Cow,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio_postgres::types::{Format, IsNull, ToSql, Type};

const SERVER_CANCEL_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) struct ScriptQuery {
    pub sql: String,
    pub parameters: String,
    pub execute: bool,
}

pub(crate) enum ScriptResult {
    Rows(TableData),
    Affected(u64),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Parameter {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    Text(String),
}

impl Parameter {
    fn postgres_text(&self) -> Option<Cow<'_, str>> {
        Some(match self {
            Self::Null => return None,
            Self::Bool(value) => Cow::Owned(value.to_string()),
            Self::Integer(value) => Cow::Owned(value.to_string()),
            Self::Float(value) => Cow::Owned(value.to_string()),
            Self::Text(value) => Cow::Borrowed(value),
        })
    }
}

impl ToSql for Parameter {
    fn to_sql(
        &self,
        _: &Type,
        output: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        let Some(value) = self.postgres_text() else {
            return Ok(IsNull::Yes);
        };
        output.extend_from_slice(value.as_bytes());
        Ok(IsNull::No)
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    fn encode_format(&self, _: &Type) -> Format {
        Format::Text
    }

    tokio_postgres::types::to_sql_checked!();
}

impl tiberius::ToSql for Parameter {
    fn to_sql(&self) -> tiberius::ColumnData<'_> {
        use tiberius::ColumnData;
        match self {
            Self::Null => ColumnData::String(None),
            Self::Bool(value) => ColumnData::Bit(Some(*value)),
            Self::Integer(value) => i32::try_from(*value)
                .map_or(ColumnData::I64(Some(*value)), |value| {
                    ColumnData::I32(Some(value))
                }),
            Self::Float(value) => ColumnData::F64(Some(*value)),
            Self::Text(value) => ColumnData::String(Some(value.as_str().into())),
        }
    }
}

impl rusqlite::ToSql for Parameter {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, ValueRef};
        Ok(ToSqlOutput::Borrowed(match self {
            Self::Null => ValueRef::Null,
            Self::Bool(value) => ValueRef::Integer(i64::from(*value)),
            Self::Integer(value) => ValueRef::Integer(*value),
            Self::Float(value) => ValueRef::Real(*value),
            Self::Text(value) => ValueRef::Text(value.as_bytes()),
        }))
    }
}

pub(crate) enum ScriptDatabase {
    Sqlite {
        database: Arc<Mutex<SqliteConnection>>,
        interrupt: Arc<rusqlite::InterruptHandle>,
    },
    Postgres(Postgres),
    SqlServer(SqlServer),
}

impl ScriptDatabase {
    pub(crate) async fn open(
        connection: &Connection,
        secrets: Option<&Arc<SecretStore>>,
    ) -> Result<Self, String> {
        match connection.kind {
            DatabaseKind::Sqlite => {
                let path = sqlite_path(connection)?;
                let database = tokio::task::spawn_blocking(move || SqliteConnection::open(path))
                    .await
                    .map_err(|error| format!("Could not open the SQLite database: {error}"))?
                    .map_err(|error| format!("Could not open the SQLite database: {error}"))?;
                Ok(Self::Sqlite {
                    interrupt: Arc::new(database.get_interrupt_handle()),
                    database: Arc::new(Mutex::new(database)),
                })
            }
            DatabaseKind::Postgres => {
                let session = SqlManager::open_postgres(connection, secrets).await?;
                Ok(Self::Postgres(Postgres {
                    session: Some(session),
                    active: false,
                }))
            }
            DatabaseKind::SqlServer => {
                let client = SqlManager::open_sqlserver(connection, secrets).await?;
                Ok(Self::SqlServer(SqlServer {
                    client: Some(Box::new(client)),
                    active: false,
                }))
            }
        }
    }

    pub(crate) async fn cancel(&mut self) -> Result<(), String> {
        match self {
            Self::Postgres(database) if database.active => {
                let session = database.session.as_ref().unwrap();
                let cancel = session.client.cancel_token();
                let result = tokio::time::timeout(
                    SERVER_CANCEL_TIMEOUT,
                    cancel.cancel_query(session.tls.clone()),
                )
                .await;
                database.active = false;
                result
                    .map_err(|_| "PostgreSQL cancellation timed out".to_owned())?
                    .map_err(|error| format!("Could not cancel the PostgreSQL query: {error}"))
            }
            Self::SqlServer(database) if database.active => {
                let client = database.client.as_mut().unwrap();
                let result =
                    tokio::time::timeout(SERVER_CANCEL_TIMEOUT, client.cancel_query()).await;
                database.active = false;
                result
                    .map_err(|_| {
                        "SQL Server did not acknowledge query cancellation before the timeout"
                            .to_owned()
                    })?
                    .map_err(|error| format!("Could not cancel the SQL Server query: {error}"))
            }
            _ => Ok(()),
        }
    }

    pub(crate) async fn run(&mut self, request: ScriptQuery) -> Result<ScriptResult, String> {
        let ScriptQuery {
            sql,
            parameters,
            execute,
        } = request;
        let parameters: Vec<Parameter> = serde_json::from_str(&parameters)
            .map_err(|error| format!("Invalid SQL parameters: {error}"))?;
        let mut result = match self {
            Self::Sqlite {
                database,
                interrupt,
            } => {
                let database = database.clone();
                let mut query = SqliteQuery {
                    cancelled: Arc::default(),
                    interrupt: Some(interrupt.clone()),
                };
                let cancelled = query.cancelled.clone();
                let outcome = tokio::task::spawn_blocking(move || {
                    sqlite(
                        &database.lock().unwrap(),
                        &sql,
                        &parameters,
                        execute,
                        &cancelled,
                    )
                })
                .await;
                query.interrupt = None;
                outcome.map_err(|error| format!("The SQLite query failed: {error}"))??
            }
            Self::Postgres(database) => {
                database.active = true;
                let result = postgres(
                    &database.session.as_ref().unwrap().client,
                    &sql,
                    &parameters,
                    execute,
                )
                .await;
                database.active = false;
                result?
            }
            Self::SqlServer(database) => {
                database.active = true;
                let result = sqlserver(
                    database.client.as_mut().unwrap(),
                    &sql,
                    &parameters,
                    execute,
                )
                .await;
                database.active = false;
                result?
            }
        };
        if let ScriptResult::Rows(table) = &mut result {
            table.unique_column_names();
        }
        Ok(result)
    }
}

fn last_table(tables: Vec<TableData>) -> ScriptResult {
    ScriptResult::Rows(
        tables
            .into_iter()
            .last()
            .unwrap_or_else(|| affected_rows_table(0)),
    )
}

fn sqlite(
    database: &SqliteConnection,
    sql: &str,
    parameters: &[Parameter],
    execute: bool,
    cancelled: &AtomicBool,
) -> Result<ScriptResult, String> {
    use rusqlite::fallible_iterator::FallibleIterator;
    let before = database.total_changes();
    let tables = if parameters.is_empty() {
        execute_sqlite(database, sql, cancelled)?
    } else {
        let mut batch = rusqlite::Batch::new(database, sql);
        let mut statement = batch
            .next()
            .map_err(|error| sqlite_error(&error))?
            .ok_or("Expected a SQL statement with placeholders.")?;
        if batch
            .next()
            .map_err(|error| sqlite_error(&error))?
            .is_some()
        {
            return Err("SQLite accepts parameters on a single statement at a time.".into());
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(STOPPED.into());
        }
        vec![sqlite_statement(
            database,
            &mut statement,
            rusqlite::params_from_iter(parameters),
            cancelled,
        )?]
    };
    Ok(if execute {
        ScriptResult::Affected(database.total_changes().saturating_sub(before))
    } else {
        last_table(tables)
    })
}

async fn postgres(
    client: &tokio_postgres::Client,
    sql: &str,
    parameters: &[Parameter],
    execute: bool,
) -> Result<ScriptResult, String> {
    // Execute uses the driver's statement count. A batch in the simple
    // protocol loses command tags, so SELECT row counts cannot be separated
    // from write counts.
    if execute {
        let params: Vec<&(dyn ToSql + Sync)> =
            parameters.iter().map(|parameter| parameter as _).collect();
        return client
            .execute(sql, &params)
            .await
            .map(ScriptResult::Affected)
            .map_err(|error| pg_error("PostgreSQL could not run the statement", error));
    }
    if parameters.is_empty() {
        let statement = client.prepare(sql).await;
        let columns = statement
            .as_ref()
            .map_or(&[][..], tokio_postgres::Statement::columns);
        return postgres_simple_tables(client, sql, columns)
            .await
            .map(last_table);
    }
    static NEXT_STATEMENT: AtomicU64 = AtomicU64::new(1);
    // Match the protocol-prepared plan in pg_prepared_statements without
    // relying on private driver state or on the session's other plans.
    let id = NEXT_STATEMENT.fetch_add(1, Ordering::Relaxed);
    let source = format!("/* codingpad Rust query {id} */\n{sql}");
    let statement = client
        .prepare(&source)
        .await
        .map_err(|error| pg_error("PostgreSQL could not prepare the statement", error))?;
    if parameters.len() != statement.params().len() {
        return Err(format!(
            "PostgreSQL expected {} parameters, received {}.",
            statement.params().len(),
            parameters.len()
        ));
    }
    postgres_bound_text(client, &source, parameters, statement.columns())
        .await
        .map(last_table)
}

async fn postgres_bound_text(
    client: &tokio_postgres::Client,
    prepared_source: &str,
    parameters: &[Parameter],
    columns: &[tokio_postgres::Column],
) -> Result<Vec<TableData>, String> {
    let values: Vec<_> = parameters.iter().map(Parameter::postgres_text).collect();
    // The extended protocol prepares utility statements too, but the driver
    // always asks it for binary rows. Use SQL EXECUTE on that same plan for
    // text rows. The extra trip lets the server quote the bound arguments and
    // plan name using the session's own SQL settings.
    let quoted = client.query_one(
        "SELECT pg_catalog.quote_ident(plan.name),
                (SELECT pg_catalog.string_agg(pg_catalog.quote_nullable(value), ', ' ORDER BY position)
                 FROM pg_catalog.unnest($2::text[]) WITH ORDINALITY AS args(value, position))
         FROM pg_catalog.pg_prepared_statements AS plan
         WHERE NOT plan.from_sql AND plan.statement = $1",
        &[&prepared_source, &values],
    ).await.map_err(|error| pg_error("PostgreSQL could not bind the values", error))?;
    let name = quoted.get::<_, &str>(0);
    let arguments = quoted.get::<_, &str>(1);
    let command = format!("EXECUTE {name}({arguments})");
    // The driver releases the protocol-prepared statement when its handle
    // goes out of scope, even if the query fails inside a transaction.
    postgres_simple_tables(client, &command, columns).await
}

async fn sqlserver(
    client: &mut SqlServerClient,
    sql: &str,
    parameters: &[Parameter],
    execute: bool,
) -> Result<ScriptResult, String> {
    if has_go_batch(sql) {
        return Err("GO is a client-side batch separator. Run each batch separately.".into());
    }
    let params: Vec<&dyn tiberius::ToSql> =
        parameters.iter().map(|parameter| parameter as _).collect();
    if execute {
        client
            .execute(sql, &params)
            .await
            .map(|result| ScriptResult::Affected(result.total()))
            .map_err(|error| format!("SQL Server could not run the statement: {error}"))
    } else {
        // Direct batches preserve session-local objects created without
        // parameters. Bound queries use Tiberius's sp_executesql RPC.
        let stream = if params.is_empty() {
            client.simple_query(sql).await
        } else {
            client.query(sql, &params).await
        };
        read_sqlserver_tables(stream).await.map(last_table)
    }
}

struct SqliteQuery {
    cancelled: Arc<AtomicBool>,
    interrupt: Option<Arc<rusqlite::InterruptHandle>>,
}

impl Drop for SqliteQuery {
    fn drop(&mut self) {
        if let Some(interrupt) = &self.interrupt {
            self.cancelled.store(true, Ordering::Relaxed);
            interrupt.interrupt();
        }
    }
}

pub(crate) struct Postgres {
    session: Option<PostgresSession>,
    active: bool,
}

impl Drop for Postgres {
    fn drop(&mut self) {
        let PostgresSession {
            client,
            tls,
            driver,
        } = self.session.take().unwrap();
        if self.active {
            let cancel = client.cancel_token();
            tokio::spawn(async move {
                let _ = tokio::time::timeout(SERVER_CANCEL_TIMEOUT, cancel.cancel_query(tls)).await;
                driver.abort();
            });
        } else {
            driver.abort();
        }
    }
}

pub(crate) struct SqlServer {
    client: Option<Box<SqlServerClient>>,
    active: bool,
}

impl Drop for SqlServer {
    fn drop(&mut self) {
        if self.active {
            let mut client = self.client.take().unwrap();
            // Keep the client alive long enough to send TDS Attention and wait
            // for the server to acknowledge cancellation.
            tokio::spawn(async move {
                let _ = tokio::time::timeout(SERVER_CANCEL_TIMEOUT, client.cancel_query()).await;
            });
        }
    }
}
