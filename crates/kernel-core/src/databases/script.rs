//! A database session owned by a single native Rust script run.
use super::{
    execute_sqlite, has_go_batch, postgres_tables, read_sqlserver_tables, sqlite_path, Connection,
    DatabaseKind, PostgresSession, SecretStore, SqlManager, SqlServerClient, SqliteConnection,
    TableData,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

pub(crate) enum ScriptDatabase {
    Sqlite {
        database: Arc<Mutex<SqliteConnection>>,
        interrupt: Arc<rusqlite::InterruptHandle>,
    },
    Postgres(Postgres),
    SqlServer(Box<SqlServerClient>),
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
                    querying: false,
                }))
            }
            DatabaseKind::SqlServer => {
                let client = SqlManager::open_sqlserver(connection, secrets).await?;
                Ok(Self::SqlServer(Box::new(client)))
            }
        }
    }

    pub(crate) async fn query(&mut self, sql: String) -> Result<TableData, String> {
        let tables = match self {
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
                    execute_sqlite(&database.lock().unwrap(), &sql, &cancelled)
                })
                .await;
                query.interrupt = None;
                outcome.map_err(|error| format!("The SQLite query failed: {error}"))??
            }
            Self::Postgres(database) => database.query(&sql).await?,
            Self::SqlServer(client) => {
                if has_go_batch(&sql) {
                    return Err(
                        "GO is a client-side batch separator. Run each batch separately.".into(),
                    );
                }
                // A direct TDS batch keeps temporary tables in the session.
                // Tiberius query() uses sp_executesql, which has its own scope.
                read_sqlserver_tables(client.simple_query(&sql).await).await?
            }
        };
        // Batches return their final result. Earlier statements still execute.
        Ok(tables
            .into_iter()
            .last()
            .unwrap_or_else(|| super::affected_rows_table(0)))
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
    querying: bool,
}

impl Postgres {
    async fn query(&mut self, sql: &str) -> Result<Vec<TableData>, String> {
        let (client, _, _) = self.session.as_ref().unwrap();
        self.querying = true;
        let result = postgres_tables(client, sql).await;
        self.querying = false;
        result
    }
}

impl Drop for Postgres {
    fn drop(&mut self) {
        let (client, tls, driver) = self.session.take().unwrap();
        if self.querying {
            // Dropping a query future alone does not cancel it on the server.
            // Keep the connection alive until the cancel request has been sent.
            let cancel = client.cancel_token();
            tokio::spawn(async move {
                let _ =
                    tokio::time::timeout(Duration::from_secs(2), cancel.cancel_query(tls)).await;
                driver.abort();
            });
        } else {
            driver.abort();
        }
    }
}
