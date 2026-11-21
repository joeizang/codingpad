//! Saved database connections and direct SQL execution.
//!
//! SQL scripts run through their connection without starting a kernel. Passwords
//! are referenced by secret name and read from the vault at connection time, so
//! connection settings never contain them.
pub mod schema;
mod script;

pub(crate) use script::{ScriptDatabase, ScriptQuery, ScriptResult};

use crate::{
    secrets::SecretStore,
    store::{parse_header, ScriptHeader},
};
use rusqlite::{types::ValueRef, Connection as SqliteConnection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
const CONNECTIONS_FILE: &str = "connections.json";
const MAX_ROWS: usize = 10_000;
const MAX_PAGE: usize = 1_000;
const DEFAULT_PAGE: usize = 100;
const STOPPED: &str = "\u{0}stopped";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    #[default]
    Prefer,
    Require,
    Disable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseKind {
    Sqlite,
    #[serde(alias = "postgresql")]
    Postgres,
    #[serde(rename = "sqlserver", alias = "mssql")]
    SqlServer,
}

/// A saved connection. Values are never stored here; `secret` names the vault
/// entry that holds the password for server databases.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub kind: DatabaseKind,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub tls: TlsMode,
    #[serde(default)]
    pub trust_server_certificate: bool,
}

impl Connection {
    fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("A connection name cannot be empty".into());
        }
        if self.name.chars().any(char::is_whitespace) {
            return Err(
                "A connection name cannot contain spaces; use letters, digits, dashes or underscores."
                    .into(),
            );
        }
        match self.kind {
            DatabaseKind::Sqlite => {
                if self
                    .path
                    .as_deref()
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                    .is_none()
                {
                    return Err("A SQLite connection needs a database file".into());
                }
            }
            DatabaseKind::Postgres => {
                for (field, value) in [
                    ("host", self.host.as_deref()),
                    ("database", self.database.as_deref()),
                    ("user", self.user.as_deref()),
                ] {
                    if value
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .is_none()
                    {
                        return Err(format!("A PostgreSQL connection needs a {field}"));
                    }
                }
            }
            DatabaseKind::SqlServer => {
                for (field, value) in [
                    ("host", self.host.as_deref()),
                    ("database", self.database.as_deref()),
                    ("user", self.user.as_deref()),
                ] {
                    if value
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .is_none()
                    {
                        return Err(format!("A SQL Server connection needs a {field}"));
                    }
                }
            }
        }
        Ok(())
    }
}

pub struct ConnectionStore {
    path: PathBuf,
    sample: Connection,
    connections: Mutex<Vec<Connection>>,
}

impl ConnectionStore {
    pub fn new(data_dir: &Path) -> Self {
        let path = data_dir.join(CONNECTIONS_FILE);
        let connections = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<Connection>>(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            sample: crate::samples::connection(data_dir),
            connections: Mutex::new(connections),
        }
    }

    pub fn list(&self) -> Vec<Connection> {
        self.connections.lock().unwrap().clone()
    }

    pub fn get(&self, id: &str) -> Option<Connection> {
        if id == self.sample.id {
            return Some(self.sample.clone());
        }
        self.connections
            .lock()
            .unwrap()
            .iter()
            .find(|connection| connection.id == id)
            .cloned()
    }

    pub fn by_name(&self, name: &str) -> Option<Connection> {
        if name == self.sample.name {
            return Some(self.sample.clone());
        }
        self.connections
            .lock()
            .unwrap()
            .iter()
            .find(|connection| connection.name == name)
            .cloned()
    }

    pub fn save(&self, mut connection: Connection) -> Result<Connection, String> {
        connection.name = connection.name.trim().to_owned();
        connection.path = connection.path.map(|path| path.trim().to_owned());
        if connection.name == crate::samples::SAMPLE_CONNECTION {
            return Err(format!(
                "\"{}\" is reserved for the built-in sample database",
                connection.name
            ));
        }
        if connection.kind == DatabaseKind::Sqlite {
            connection.secret = None;
        }
        connection.validate()?;
        let mut all = self.connections.lock().unwrap();
        if all.iter().any(|item| {
            item.name == connection.name && (connection.id.is_empty() || item.id != connection.id)
        }) {
            return Err(format!(
                "A connection named \"{}\" already exists",
                connection.name
            ));
        }
        if connection.id.is_empty() {
            connection.id = next_id();
            all.push(connection.clone());
        } else if let Some(existing) = all.iter_mut().find(|item| item.id == connection.id) {
            *existing = connection.clone();
        } else {
            return Err("That connection no longer exists".into());
        }
        save_connections(&self.path, &all)?;
        Ok(connection)
    }

    pub fn delete(&self, id: &str) -> Result<Connection, String> {
        let mut all = self.connections.lock().unwrap();
        let index = all
            .iter()
            .position(|connection| connection.id == id)
            .ok_or_else(|| "That connection no longer exists".to_owned())?;
        let connection = all.remove(index);
        save_connections(&self.path, &all)?;
        Ok(connection)
    }
}

fn next_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("conn-{nanos}-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn save_connections(path: &Path, connections: &[Connection]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the settings folder: {error}"))?;
    let text = serde_json::to_string_pretty(connections)
        .map_err(|error| format!("Could not encode connections: {error}"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("Could not create a temporary file: {error}"))?;
    temp.write_all(text.as_bytes())
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    temp.persist(path)
        .map_err(|error| format!("Could not save {}: {}", path.display(), error.error))?;
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct TableData {
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
    truncated: bool,
}

#[derive(Clone, Debug)]
struct Column {
    name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlRunResult {
    pub ok: bool,
    #[serde(default)]
    pub stopped: bool,
    pub tables: Vec<Value>,
}

type Interrupt = Arc<Mutex<Option<rusqlite::InterruptHandle>>>;

type SqlServerClient = tiberius::Client<tokio_util::compat::Compat<tokio::net::TcpStream>>;
struct PostgresSession {
    client: tokio_postgres::Client,
    tls: postgres_native_tls::MakeTlsConnector,
    driver: tokio::task::JoinHandle<()>,
}

pub struct SqlManager {
    connections: Arc<ConnectionStore>,
    secrets: Arc<SecretStore>,
    results: Mutex<HashMap<String, Arc<TableData>>>,
    script_results: Mutex<HashMap<String, Vec<String>>>,
    interrupts: Mutex<HashMap<String, Interrupt>>,
    cancel_flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
    stops: Mutex<HashMap<String, tokio::sync::watch::Sender<u64>>>,
    next_handle: AtomicU64,
}

impl SqlManager {
    pub fn new(connections: Arc<ConnectionStore>, secrets: Arc<SecretStore>) -> Self {
        Self {
            connections,
            secrets,
            results: Mutex::new(HashMap::new()),
            script_results: Mutex::new(HashMap::new()),
            interrupts: Mutex::new(HashMap::new()),
            cancel_flags: Mutex::new(HashMap::new()),
            stops: Mutex::new(HashMap::new()),
            next_handle: AtomicU64::new(1),
        }
    }

    pub async fn test(&self, id: &str) -> Result<String, String> {
        let connection = self
            .connections
            .get(id)
            .ok_or_else(|| "That connection no longer exists".to_owned())?;
        match connection.kind {
            DatabaseKind::Sqlite => {
                let path = sqlite_path(&connection)?;
                if !Path::new(&path).exists() {
                    return Ok(format!(
                        "The SQLite file does not exist yet; it will be created at {path} on the first run."
                    ));
                }
                tokio::task::spawn_blocking(move || {
                    let database = SqliteConnection::open(&path)
                        .map_err(|error| format!("Could not open the SQLite database: {error}"))?;
                    database
                        .query_row("SELECT 1", [], |_| Ok(()))
                        .map_err(|error| format!("SQLite rejected the connection test: {error}"))?;
                    Ok(format!("Connected to the SQLite database at {path}."))
                })
                .await
                .map_err(|error| format!("The connection test failed: {error}"))?
            }
            DatabaseKind::Postgres => {
                let (client, _) = self.connect_postgres(&connection).await?;
                client
                    .simple_query("SELECT 1")
                    .await
                    .map_err(|error| format!("PostgreSQL rejected the connection test: {error}"))?;
                Ok(format!(
                    "Connected to PostgreSQL at {}:{}/{}.",
                    postgres_host(&connection),
                    postgres_port(&connection),
                    connection.database.as_deref().unwrap_or("")
                ))
            }
            DatabaseKind::SqlServer => {
                let mut client = self.connect_sqlserver(&connection).await?;
                client
                    .simple_query("SELECT 1")
                    .await
                    .map_err(|error| format!("SQL Server rejected the connection test: {error}"))?
                    .into_results()
                    .await
                    .map_err(|error| format!("SQL Server rejected the connection test: {error}"))?;
                Ok(format!(
                    "Connected to SQL Server at {}:{}/{}.",
                    server_host(&connection),
                    server_port(&connection, 1433),
                    connection.database.as_deref().unwrap_or("")
                ))
            }
        }
    }

    pub async fn run(
        &self,
        script_id: &str,
        source: &str,
        path: Option<&str>,
    ) -> Result<SqlRunResult, String> {
        if script_id.is_empty() {
            return Err("A script id is required".into());
        }
        // The UI calls begin_run before Run, so a Stop that arrives before this
        // future is polled still targets this run. Without begin_run, register
        // here for direct callers.
        let cancelled = {
            let mut flags = self.cancel_flags.lock().unwrap();
            flags
                .entry(script_id.to_owned())
                .or_insert_with(|| Arc::new(AtomicBool::new(false)))
                .clone()
        };
        let result = self
            .run_registered(script_id, source, path, &cancelled)
            .await;
        self.cancel_flags.lock().unwrap().remove(script_id);
        result
    }

    async fn run_registered(
        &self,
        script_id: &str,
        source: &str,
        _path: Option<&str>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<SqlRunResult, String> {
        let header = parse_header(source).ok_or_else(missing_header)?;
        let name = connection_name(&header)?;
        let connection = self.connections.by_name(&name).ok_or_else(|| {
            format!("Connection \"{name}\" was not found. Add it in Connections…")
        })?;
        self.clear_results(script_id);
        if connection.kind == DatabaseKind::SqlServer && has_go_batch(source) {
            return Err(
                "GO is a client-side batch separator, not T-SQL. Remove it or run each batch separately."
                    .into(),
            );
        }
        if cancelled.load(Ordering::Relaxed) {
            return Ok(SqlRunResult {
                ok: false,
                stopped: true,
                tables: Vec::new(),
            });
        }
        let body = source.split_once('\n').map(|(_, body)| body).unwrap_or("");
        match connection.kind {
            DatabaseKind::Sqlite => {
                self.run_sqlite(script_id, &connection, body, cancelled)
                    .await
            }
            DatabaseKind::Postgres => {
                self.run_postgres(script_id, &connection, body, cancelled)
                    .await
            }
            DatabaseKind::SqlServer => {
                self.run_sqlserver(script_id, &connection, body, cancelled)
                    .await
            }
        }
    }

    async fn run_sqlite(
        &self,
        script_id: &str,
        connection: &Connection,
        source: &str,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<SqlRunResult, String> {
        let path = sqlite_path(connection)?;
        let source = source.to_owned();
        let interrupt: Interrupt = Arc::new(Mutex::new(None));
        let cancelled = cancelled.clone();
        self.interrupts
            .lock()
            .unwrap()
            .insert(script_id.to_owned(), interrupt.clone());
        let outcome = tokio::task::spawn_blocking(move || {
            let database = SqliteConnection::open(&path)
                .map_err(|error| format!("Could not open the SQLite database: {error}"))?;
            // SQLite needs no password; the vault is only used by server
            // connections on later layers.
            *interrupt.lock().unwrap() = Some(database.get_interrupt_handle());
            // A Stop that arrived before the file opened must not be lost.
            if cancelled.load(Ordering::Relaxed) {
                return Err(STOPPED.to_string());
            }
            execute_sqlite(&database, &source, &cancelled)
        })
        .await
        .map_err(|error| format!("The query failed: {error}"))?;
        self.interrupts.lock().unwrap().remove(script_id);
        match outcome {
            Ok(tables) => {
                let mut nodes = Vec::with_capacity(tables.len());
                for table in tables {
                    let handle =
                        format!("sql-{}", self.next_handle.fetch_add(1, Ordering::Relaxed));
                    let table = Arc::new(table);
                    self.results
                        .lock()
                        .unwrap()
                        .insert(handle.clone(), table.clone());
                    self.script_results
                        .lock()
                        .unwrap()
                        .entry(script_id.to_owned())
                        .or_default()
                        .push(handle.clone());
                    nodes.push(table_node(&table, &handle, 0, DEFAULT_PAGE));
                }
                Ok(SqlRunResult {
                    ok: true,
                    stopped: false,
                    tables: nodes,
                })
            }
            Err(error) if error == STOPPED => Ok(SqlRunResult {
                ok: false,
                stopped: true,
                tables: Vec::new(),
            }),
            Err(error) => Err(error),
        }
    }

    /// Register a run before invoking `run`, so a fast Stop cannot miss it.
    /// Calling this for an idle tab does not affect the next Run; only a Stop
    /// between this call and the matching run does.
    pub fn begin_run(&self, script_id: &str) {
        self.cancel_flags
            .lock()
            .unwrap()
            .entry(script_id.to_owned())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)));
    }

    fn begin_stop(&self, script_id: &str) -> tokio::sync::watch::Receiver<u64> {
        self.stops
            .lock()
            .unwrap()
            .entry(script_id.to_owned())
            .or_insert_with(|| tokio::sync::watch::channel(0).0)
            .subscribe()
    }

    async fn connect_postgres(
        &self,
        connection: &Connection,
    ) -> Result<
        (
            tokio_postgres::Client,
            postgres_native_tls::MakeTlsConnector,
        ),
        String,
    > {
        let PostgresSession { client, tls, .. } =
            Self::open_postgres(connection, Some(&self.secrets)).await?;
        Ok((client, tls))
    }

    async fn open_postgres(
        connection: &Connection,
        secrets: Option<&Arc<SecretStore>>,
    ) -> Result<PostgresSession, String> {
        let mut config = tokio_postgres::Config::new();
        config
            .host(postgres_host(connection))
            .port(postgres_port(connection))
            .dbname(connection.database.as_deref().unwrap_or(""))
            .user(connection.user.as_deref().unwrap_or(""))
            .ssl_mode(match connection.tls {
                TlsMode::Prefer => tokio_postgres::config::SslMode::Prefer,
                TlsMode::Require => tokio_postgres::config::SslMode::Require,
                TlsMode::Disable => tokio_postgres::config::SslMode::Disable,
            });
        if let Some(secret) = connection
            .secret
            .as_deref()
            .map(str::trim)
            .filter(|secret| !secret.is_empty())
        {
            let store = secrets
                .cloned()
                .ok_or("The password vault is unavailable")?;
            let secret = secret.to_owned();
            let password = tokio::task::spawn_blocking(move || store.get(&secret))
                .await
                .map_err(|error| format!("The password read failed: {error}"))?
                .map_err(|error| {
                    format!("Could not read the PostgreSQL password from the vault: {error}")
                })?;
            config.password(password);
        }
        config.application_name("CodingPad");
        config.connect_timeout(std::time::Duration::from_secs(10));
        let connector = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(connection.trust_server_certificate)
            .build()
            .map_err(|error| format!("Could not prepare TLS for PostgreSQL: {error}"))?;
        let tls = postgres_native_tls::MakeTlsConnector::new(connector);
        let (client, connection) = config.connect(tls.clone()).await.map_err(|error| {
            let mut detail = pg_detail(&error);
            if !connection.trust_server_certificate && detail.to_ascii_lowercase().contains("tls handshake") {
                detail.push_str(". For a self-signed development server, enable \"Trust the server certificate\" in Connections.");
            }
            format!(
                "Could not connect to PostgreSQL at {}:{}/{}: {}",
                postgres_host(connection),
                postgres_port(connection),
                connection.database.as_deref().unwrap_or(""),
                detail
            )
        })?;
        let task = tokio::spawn(async move {
            let _ = connection.await;
        });
        Ok(PostgresSession {
            client,
            tls,
            driver: task,
        })
    }

    async fn run_postgres(
        &self,
        script_id: &str,
        connection: &Connection,
        source: &str,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<SqlRunResult, String> {
        let mut stop = self.begin_stop(script_id);
        if cancelled.load(Ordering::Relaxed) {
            self.stops.lock().unwrap().remove(script_id);
            return Ok(SqlRunResult {
                ok: false,
                stopped: true,
                tables: Vec::new(),
            });
        }
        let client = tokio::select! {
            result = self.connect_postgres(connection) => match result {
                Ok((client, tls)) => (client, tls),
                Err(error) => {
                    self.stops.lock().unwrap().remove(script_id);
                    return Err(error);
                }
            },
            _ = stop.changed() => {
                self.stops.lock().unwrap().remove(script_id);
                return Ok(SqlRunResult { ok: false, stopped: true, tables: Vec::new() });
            }
        };
        let (client, tls) = client;
        let cancel = client.cancel_token();
        let cancel_tls = tls.clone();
        let query = postgres_tables(&client, source);
        tokio::pin!(query);
        let outcome: Result<Vec<TableData>, (bool, String)> = tokio::select! {
            result = &mut query => result.map_err(|error| (false, error)),
            _ = stop.changed() => {
                // Dropping the future only abandons the client side. Ask the
                // server to cancel and wait for its response, all within one
                // deadline so an unresponsive server cannot hang the run.
                let deadline =
                    tokio::time::Instant::now() + std::time::Duration::from_secs(2);
                match tokio::time::timeout_at(deadline, cancel.cancel_query(cancel_tls)).await {
                    Err(_) => Err((
                        false,
                        "PostgreSQL did not accept the cancel request within 2 seconds; the statement may still be running."
                            .into(),
                    )),
                    Ok(Err(error)) => Err((
                        false,
                        format!("Could not cancel the PostgreSQL query: {}", pg_detail(&error)),
                    )),
                    Ok(Ok(())) => match tokio::time::timeout_at(deadline, &mut query).await {
                        // The statement finished before it could be cancelled.
                        Ok(Ok(tables)) => Ok(tables),
                        Ok(Err(error)) => Err((true, error)),
                        Err(_) => Err((
                            false,
                            "PostgreSQL did not confirm cancellation within 2 seconds; the statement may still be running."
                                .into(),
                        )),
                    },
                }
            }
        };
        self.stops.lock().unwrap().remove(script_id);
        let tables = match outcome {
            Ok(tables) => tables,
            Err((true, _)) if cancelled.load(Ordering::Relaxed) => {
                return Ok(SqlRunResult {
                    ok: false,
                    stopped: true,
                    tables: Vec::new(),
                })
            }
            Err((_, message)) => return Err(message),
        };
        let mut nodes = Vec::with_capacity(tables.len());
        for table in tables {
            let handle = format!("sql-{}", self.next_handle.fetch_add(1, Ordering::Relaxed));
            let table = Arc::new(table);
            self.results
                .lock()
                .unwrap()
                .insert(handle.clone(), table.clone());
            self.script_results
                .lock()
                .unwrap()
                .entry(script_id.to_owned())
                .or_default()
                .push(handle.clone());
            nodes.push(table_node(&table, &handle, 0, DEFAULT_PAGE));
        }
        Ok(SqlRunResult {
            ok: true,
            stopped: false,
            tables: nodes,
        })
    }

    async fn connect_sqlserver(&self, connection: &Connection) -> Result<SqlServerClient, String> {
        Self::open_sqlserver(connection, Some(&self.secrets)).await
    }

    async fn open_sqlserver(
        connection: &Connection,
        secrets: Option<&Arc<SecretStore>>,
    ) -> Result<SqlServerClient, String> {
        use tokio_util::compat::TokioAsyncWriteCompatExt;
        let mut config = tiberius::Config::new();
        config.host(server_host(connection));
        config.port(server_port(connection, 1433));
        config.database(connection.database.as_deref().unwrap_or(""));
        let user = connection.user.as_deref().unwrap_or("");
        let password = match connection
            .secret
            .as_deref()
            .map(str::trim)
            .filter(|secret| !secret.is_empty())
        {
            Some(secret) => {
                let store = secrets
                    .cloned()
                    .ok_or("The password vault is unavailable")?;
                let secret = secret.to_owned();
                tokio::task::spawn_blocking(move || store.get(&secret))
                    .await
                    .map_err(|error| format!("The password read failed: {error}"))?
                    .map_err(|error| {
                        format!("Could not read the SQL Server password from the vault: {error}")
                    })?
            }
            None => String::new(),
        };
        config.authentication(tiberius::AuthMethod::sql_server(user, password));
        config.encryption(match connection.tls {
            TlsMode::Prefer => tiberius::EncryptionLevel::On,
            TlsMode::Require => tiberius::EncryptionLevel::Required,
            TlsMode::Disable => tiberius::EncryptionLevel::NotSupported,
        });
        if connection.trust_server_certificate {
            config.trust_cert();
        }
        let address = format!(
            "{}:{}/{}",
            server_host(connection),
            server_port(connection, 1433),
            connection.database.as_deref().unwrap_or("")
        );
        let connect = async {
            let tcp = tokio::net::TcpStream::connect(config.get_addr())
                .await
                .map_err(|error| error.to_string())?;
            tcp.set_nodelay(true).map_err(|error| error.to_string())?;
            tiberius::Client::connect(config, tcp.compat_write())
                .await
                .map_err(|error| {
                    if matches!(error, tiberius::error::Error::Tls(_)) && !connection.trust_server_certificate {
                        format!("{error}. For a self-signed development server, enable \"Trust the server certificate\" in Connections.")
                    } else {
                        error.to_string()
                    }
                })
        };
        tokio::time::timeout(std::time::Duration::from_secs(15), connect)
            .await
            .map_err(|_| format!("Timed out connecting to SQL Server at {address}"))?
            .map_err(|error| format!("Could not connect to SQL Server at {address}: {error}"))
    }

    async fn run_sqlserver(
        &self,
        script_id: &str,
        connection: &Connection,
        source: &str,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<SqlRunResult, String> {
        let mut stop = self.begin_stop(script_id);
        if cancelled.load(Ordering::Relaxed) {
            self.stops.lock().unwrap().remove(script_id);
            return Ok(SqlRunResult {
                ok: false,
                stopped: true,
                tables: Vec::new(),
            });
        }
        let mut client = tokio::select! {
            result = self.connect_sqlserver(connection) => match result {
                Ok(client) => client,
                Err(error) => {
                    self.stops.lock().unwrap().remove(script_id);
                    return Err(error);
                }
            },
            _ = stop.changed() => {
                self.stops.lock().unwrap().remove(script_id);
                return Ok(SqlRunResult { ok: false, stopped: true, tables: Vec::new() });
            }
        };
        let mut query = Box::pin(sqlserver_tables(&mut client, source));
        let outcome = tokio::select! {
            result = &mut query => result,
            _ = stop.changed() => {
                drop(query);
                // Closing resets the TDS connection; an open transaction is
                // rolled back instead of committing after we reported Stop.
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    client.close(),
                )
                .await;
                self.stops.lock().unwrap().remove(script_id);
                return Ok(SqlRunResult { ok: false, stopped: true, tables: Vec::new() });
            }
        };
        self.stops.lock().unwrap().remove(script_id);
        let tables = outcome?;
        let mut nodes = Vec::with_capacity(tables.len());
        for table in tables {
            let handle = format!("sql-{}", self.next_handle.fetch_add(1, Ordering::Relaxed));
            let table = Arc::new(table);
            self.results
                .lock()
                .unwrap()
                .insert(handle.clone(), table.clone());
            self.script_results
                .lock()
                .unwrap()
                .entry(script_id.to_owned())
                .or_default()
                .push(handle.clone());
            nodes.push(table_node(&table, &handle, 0, DEFAULT_PAGE));
        }
        Ok(SqlRunResult {
            ok: true,
            stopped: false,
            tables: nodes,
        })
    }

    pub fn stop(&self, script_id: &str) {
        // Stop only affects a run that has begun. A Stop on an idle tab is a
        // no-op, so it cannot cancel the next Run.
        if let Some(cancelled) = self.cancel_flags.lock().unwrap().get(script_id).cloned() {
            cancelled.store(true, Ordering::Relaxed);
        }
        if let Some(interrupt) = self.interrupts.lock().unwrap().get(script_id) {
            if let Some(handle) = interrupt.lock().unwrap().as_ref() {
                handle.interrupt();
            }
        }
        if let Some(stop) = self.stops.lock().unwrap().get(script_id) {
            stop.send_modify(|version| *version += 1);
        }
    }

    /// Release every query and buffered result when the app exits.
    pub fn shutdown(&self) {
        self.results.lock().unwrap().clear();
        self.script_results.lock().unwrap().clear();
        self.interrupts.lock().unwrap().clear();
        self.cancel_flags.lock().unwrap().clear();
        self.stops.lock().unwrap().clear();
    }

    pub fn reset(&self, script_id: &str) {
        self.stop(script_id);
        self.cancel_flags.lock().unwrap().remove(script_id);
        self.clear_results(script_id);
    }

    fn clear_results(&self, script_id: &str) {
        if let Some(handles) = self.script_results.lock().unwrap().remove(script_id) {
            let mut results = self.results.lock().unwrap();
            for handle in handles {
                results.remove(&handle);
            }
        }
    }

    pub fn expand(
        &self,
        script_id: &str,
        handle: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Value, String> {
        let table = self.owned_table(script_id, handle)?;
        let offset = match cursor {
            Some(cursor) => cursor
                .parse::<usize>()
                .map_err(|_| "cursor must be a non-negative integer".to_owned())?,
            None => 0,
        };
        let limit = limit
            .map(|limit| (limit as usize).clamp(1, MAX_PAGE))
            .unwrap_or(DEFAULT_PAGE);
        Ok(table_node(&table, handle, offset, limit))
    }

    fn owned_table(&self, script_id: &str, handle: &str) -> Result<Arc<TableData>, String> {
        let belongs = self
            .script_results
            .lock()
            .unwrap()
            .get(script_id)
            .is_some_and(|handles| handles.iter().any(|item| item == handle));
        if !belongs {
            return Err("That result is no longer available. Run the script again.".into());
        }
        self.results
            .lock()
            .unwrap()
            .get(handle)
            .cloned()
            .ok_or_else(|| "That result is no longer available. Run the script again.".to_owned())
    }
}

impl SqlManager {
    /// One page of a stored result as plain cells, with full text for cells
    /// that were cut for display.
    pub fn export_page(
        &self,
        script_id: &str,
        handle: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<crate::export::ExportPage, String> {
        let table = self.owned_table(script_id, handle)?;
        let offset = match cursor {
            Some(cursor) => cursor
                .parse::<usize>()
                .map_err(|_| "cursor must be a non-negative integer".to_owned())?,
            None => 0,
        };
        Ok(table.export_page(offset, limit.max(1) as usize))
    }
}

impl TableData {
    fn unique_column_names(&mut self) {
        use std::collections::HashSet;
        let mut used = HashSet::new();
        for column in &mut self.columns {
            if used.insert(column.name.clone()) {
                continue;
            }
            let name = (2..)
                .map(|index| format!("{}_{index}", column.name))
                .find(|name| !used.contains(name))
                .unwrap();
            used.insert(name.clone());
            column.name = name;
        }
    }

    pub(crate) fn node(&self, handle: &str, offset: usize, limit: usize) -> Value {
        table_node(self, handle, offset, limit)
    }

    pub(crate) fn column_names(&self) -> impl Iterator<Item = &str> {
        self.columns.iter().map(|column| column.name.as_str())
    }

    pub(crate) fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn truncated(&self) -> bool {
        self.truncated
    }

    pub(crate) fn plain_rows(&self) -> impl Iterator<Item = impl Iterator<Item = Value> + '_> + '_ {
        self.rows
            .iter()
            .map(|row| row.iter().map(crate::export::plain_cell))
    }

    pub(crate) fn export_page(&self, offset: usize, limit: usize) -> crate::export::ExportPage {
        let offset = offset.min(self.rows.len());
        let end = offset.saturating_add(limit).min(self.rows.len());
        crate::export::ExportPage {
            columns: self.column_names().map(str::to_owned).collect(),
            rows: self
                .plain_rows()
                .skip(offset)
                .take(end - offset)
                .map(Iterator::collect)
                .collect(),
            next: (end < self.rows.len()).then(|| end.to_string()),
            total: Some(self.rows.len() as u64),
        }
    }
}

fn missing_header() -> String {
    "Add a \"-- codingpad: connection=<name>\" header naming a connection.".into()
}

fn connection_name(header: &ScriptHeader) -> Result<String, String> {
    header
        .settings
        .get("connection")
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            "The script header needs connection=<name>, for example \"-- codingpad: connection=northwind\"."
                .into()
        })
}

fn sqlite_path(connection: &Connection) -> Result<String, String> {
    connection
        .path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "This SQLite connection has no database file".into())
}

fn execute_sqlite(
    database: &SqliteConnection,
    source: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<TableData>, String> {
    use rusqlite::fallible_iterator::FallibleIterator;

    let mut batch = rusqlite::Batch::new(database, source);
    let mut tables = Vec::new();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(STOPPED.into());
        }
        let statement = batch.next().map_err(|error| sqlite_error(&error))?;
        let Some(mut statement) = statement else {
            break;
        };
        tables.push(sqlite_statement(database, &mut statement, [], cancelled)?);
    }
    if tables.is_empty() {
        tables.push(affected_rows_table(0));
    }
    Ok(tables)
}

fn sqlite_statement(
    database: &SqliteConnection,
    statement: &mut rusqlite::Statement<'_>,
    parameters: impl rusqlite::Params,
    cancelled: &AtomicBool,
) -> Result<TableData, String> {
    if statement.column_count() > 0 {
        let columns: Vec<Column> = statement
            .column_names()
            .map(|name| Column {
                name: name.to_owned(),
            })
            .collect();
        let mut rows = statement
            .query(parameters)
            .map_err(|error| sqlite_error(&error))?;
        let mut values = Vec::new();
        let mut truncated = false;
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(STOPPED.into());
            }
            let row = match rows.next() {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(error) => return Err(sqlite_error(&error)),
            };
            if values.len() >= MAX_ROWS {
                truncated = true;
                break;
            }
            let mut cells = Vec::with_capacity(columns.len());
            for index in 0..columns.len() {
                let value = row.get_ref(index).map_err(|error| sqlite_error(&error))?;
                cells.push(cell_node(value, values.len(), index));
            }
            values.push(cells);
        }
        Ok(TableData {
            columns,
            rows: values,
            truncated,
        })
    } else {
        let before = database.total_changes();
        statement
            .execute(parameters)
            .map_err(|error| sqlite_error(&error))?;
        let changed = database.total_changes().saturating_sub(before);
        Ok(affected_rows_table(changed))
    }
}

fn affected_rows_table(changed: u64) -> TableData {
    TableData {
        columns: vec![Column {
            name: "rows affected".into(),
        }],
        rows: vec![vec![
            json!({"id": "sql:affected", "kind": "primitive", "v": changed}),
        ]],
        truncated: false,
    }
}

fn sqlite_error(error: &rusqlite::Error) -> String {
    if is_interrupted(error) {
        STOPPED.into()
    } else {
        format!("SQLite could not run the statement: {error}")
    }
}
fn postgres_host(connection: &Connection) -> &str {
    connection
        .host
        .as_deref()
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .unwrap_or("localhost")
}

fn postgres_port(connection: &Connection) -> u16 {
    connection.port.unwrap_or(5432)
}

fn has_go_batch(source: &str) -> bool {
    fn is_separator(line: &str) -> bool {
        let mut words = line.split_whitespace();
        if !words
            .next()
            .is_some_and(|word| word.eq_ignore_ascii_case("GO"))
        {
            return false;
        }
        match words.next() {
            None => true,
            Some(count) => {
                count.bytes().all(|byte| byte.is_ascii_digit()) && words.next().is_none()
            }
        }
    }

    let mut chars = source.chars().peekable();
    let mut line = String::new();
    let mut quote = None;
    let mut block_depth = 0;
    let mut line_comment = false;
    while let Some(ch) = chars.next() {
        if ch == '\n' {
            if is_separator(&line) {
                return true;
            }
            line.clear();
            line_comment = false;
            if quote.is_some() {
                line.push('?');
            }
            continue;
        }
        if line_comment {
            continue;
        }
        if let Some(end) = quote {
            if ch == end {
                if chars.peek() == Some(&end) {
                    chars.next();
                } else {
                    quote = None;
                }
            }
        } else if block_depth > 0 {
            if ch == '/' && chars.peek() == Some(&'*') {
                chars.next();
                block_depth += 1;
            } else if ch == '*' && chars.peek() == Some(&'/') {
                chars.next();
                block_depth -= 1;
            }
        } else {
            match ch {
                '-' if chars.peek() == Some(&'-') => {
                    chars.next();
                    line_comment = true;
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    block_depth = 1;
                    line.push(' ');
                }
                '\'' | '"' | '[' => {
                    quote = Some(if ch == '[' { ']' } else { ch });
                    line.push('?');
                }
                _ => line.push(ch),
            }
        }
    }
    is_separator(&line)
}

fn server_host(connection: &Connection) -> &str {
    connection
        .host
        .as_deref()
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .unwrap_or("localhost")
}

fn server_port(connection: &Connection, default: u16) -> u16 {
    connection.port.unwrap_or(default)
}

async fn sqlserver_tables(
    client: &mut SqlServerClient,
    source: &str,
) -> Result<Vec<TableData>, String> {
    read_sqlserver_tables(client.query(source, &[]).await).await
}

async fn read_sqlserver_tables(
    stream: tiberius::Result<tiberius::QueryStream<'_>>,
) -> Result<Vec<TableData>, String> {
    use futures_util::TryStreamExt;

    let stream =
        stream.map_err(|error| format!("SQL Server could not run the statement: {error}"))?;
    tokio::pin!(stream);
    let mut tables: Vec<TableData> = Vec::new();
    let mut current: Option<(Vec<Column>, Vec<Vec<Value>>, bool)> = None;
    while let Some(item) = stream
        .as_mut()
        .try_next()
        .await
        .map_err(|error| format!("SQL Server could not read the results: {error}"))?
    {
        if let Some(metadata) = item.as_metadata() {
            if let Some((columns, rows, truncated)) = current.take() {
                tables.push(TableData {
                    columns,
                    rows,
                    truncated,
                });
            }
            current = Some((
                metadata
                    .columns()
                    .iter()
                    .map(|column| Column {
                        name: column.name().to_owned(),
                    })
                    .collect(),
                Vec::new(),
                false,
            ));
        } else if let Some(row) = item.as_row() {
            if current.is_none() {
                current = Some((
                    row.columns()
                        .iter()
                        .map(|column| Column {
                            name: column.name().to_owned(),
                        })
                        .collect(),
                    Vec::new(),
                    false,
                ));
            }
            let (_, rows, truncated) = current.as_mut().unwrap();
            if rows.len() >= MAX_ROWS {
                // Keep draining so later result sets and writes still happen.
                *truncated = true;
                continue;
            }
            let mut cells = Vec::with_capacity(row.columns().len());
            for (index, (column, value)) in row.cells().enumerate() {
                cells.push(sqlserver_cell(row, index, column, value, rows.len()));
            }
            rows.push(cells);
        }
    }
    if let Some((columns, rows, truncated)) = current {
        tables.push(TableData {
            columns,
            rows,
            truncated,
        });
    }
    if tables.is_empty() {
        tables.push(affected_table_text("Statement completed.".into()));
    }
    Ok(tables)
}

fn sqlserver_datetime_text(value: chrono::NaiveDateTime, kind: tiberius::ColumnType) -> String {
    match kind {
        tiberius::ColumnType::Datetime
        | tiberius::ColumnType::Datetimen
        | tiberius::ColumnType::Datetime4 => (value + chrono::Duration::microseconds(500))
            .format("%Y-%m-%dT%H:%M:%S%.3f")
            .to_string(),
        _ => value.format("%Y-%m-%dT%H:%M:%S%.f").to_string(),
    }
}

fn sqlserver_cell(
    row: &tiberius::Row,
    index: usize,
    column: &tiberius::Column,
    value: &tiberius::ColumnData<'_>,
    row_index: usize,
) -> Value {
    use tiberius::{ColumnData, ColumnType};
    let id = format!("sql:r{row_index}c{index}");
    // sql_variant columns carry the concrete type in the value, not the metadata.
    let kind = match value {
        ColumnData::Date(_) => ColumnType::Daten,
        ColumnData::Time(_) => ColumnType::Timen,
        ColumnData::DateTime(_) => ColumnType::Datetime,
        ColumnData::SmallDateTime(_) => ColumnType::Datetime4,
        ColumnData::DateTime2(_) => ColumnType::Datetime2,
        ColumnData::DateTimeOffset(_) => ColumnType::DatetimeOffsetn,
        _ => column.column_type(),
    };
    match kind {
        ColumnType::Daten => {
            if let Ok(value) = row.try_get::<chrono::NaiveDate, _>(index) {
                return value.map_or_else(
                    || null_node(&id),
                    |value| bounded_text(&id, value.format("%Y-%m-%d").to_string(), Some("date")),
                );
            }
        }
        ColumnType::Timen => {
            if let Ok(value) = row.try_get::<chrono::NaiveTime, _>(index) {
                return value.map_or_else(
                    || null_node(&id),
                    |value| {
                        bounded_text(&id, value.format("%H:%M:%S%.f").to_string(), Some("time"))
                    },
                );
            }
        }
        ColumnType::Datetime2
        | ColumnType::Datetime
        | ColumnType::Datetimen
        | ColumnType::Datetime4 => {
            if let Ok(value) = row.try_get::<chrono::NaiveDateTime, _>(index) {
                return value.map_or_else(
                    || null_node(&id),
                    |value| {
                        bounded_text(&id, sqlserver_datetime_text(value, kind), Some("datetime"))
                    },
                );
            }
        }
        ColumnType::DatetimeOffsetn => {
            if let Ok(value) = row.try_get::<chrono::DateTime<chrono::FixedOffset>, _>(index) {
                return value.map_or_else(
                    || null_node(&id),
                    |value| bounded_text(&id, value.to_rfc3339(), Some("datetimeoffset")),
                );
            }
        }
        ColumnType::Money | ColumnType::Money4 => {
            if let ColumnData::F64(Some(value)) = value {
                return bounded_text(&id, format!("{value:.4}"), Some("money"));
            }
        }
        _ => {}
    }
    match value {
        ColumnData::U8(value) => {
            value.map_or_else(|| null_node(&id), |value| integer_node(&id, value as i64))
        }
        ColumnData::I16(value) => {
            value.map_or_else(|| null_node(&id), |value| integer_node(&id, value as i64))
        }
        ColumnData::I32(value) => {
            value.map_or_else(|| null_node(&id), |value| integer_node(&id, value as i64))
        }
        ColumnData::I64(value) => {
            value.map_or_else(|| null_node(&id), |value| integer_node(&id, value))
        }
        ColumnData::F32(value) => match value {
            None => null_node(&id),
            Some(value) if !value.is_finite() => bounded_text(&id, value.to_string(), None),
            Some(value) => primitive_node(
                &id,
                json!(value.to_string().parse::<f64>().unwrap_or(*value as f64)),
            ),
        },
        ColumnData::F64(value) => match value {
            None => null_node(&id),
            Some(value) if value.is_nan() => bounded_text(&id, "NaN".into(), None),
            Some(value) if value.is_infinite() => bounded_text(
                &id,
                if value.is_sign_negative() {
                    "-Infinity".into()
                } else {
                    "Infinity".into()
                },
                None,
            ),
            Some(value) => primitive_node(&id, json!(value)),
        },
        ColumnData::Bit(value) => {
            value.map_or_else(|| null_node(&id), |value| primitive_node(&id, json!(value)))
        }
        ColumnData::String(value) => value.as_ref().map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, value.to_string(), Some("nvarchar")),
        ),
        ColumnData::Guid(value) => value.map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, value.to_string(), Some("uniqueidentifier")),
        ),
        ColumnData::Binary(value) => value.as_ref().map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, hex(value), Some("varbinary")),
        ),
        ColumnData::Numeric(value) => value.as_ref().map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, value.to_string(), Some("numeric")),
        ),
        ColumnData::Xml(value) => value.as_ref().map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, value.to_string(), Some("xml")),
        ),
        ColumnData::DateTime(value) => value.map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, format!("{value:?}"), Some("datetime")),
        ),
        ColumnData::SmallDateTime(value) => value.map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, format!("{value:?}"), Some("smalldatetime")),
        ),
        ColumnData::Time(value) => value.map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, format!("{value:?}"), Some("time")),
        ),
        ColumnData::Date(value) => value.map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, format!("{value:?}"), Some("date")),
        ),
        ColumnData::DateTime2(value) => value.map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, format!("{value:?}"), Some("datetime2")),
        ),
        ColumnData::DateTimeOffset(value) => value.map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, format!("{value:?}"), Some("datetimeoffset")),
        ),
    }
}

async fn postgres_tables(
    client: &tokio_postgres::Client,
    source: &str,
) -> Result<Vec<TableData>, String> {
    match client.prepare(source).await {
        Ok(statement) if !statement.columns().is_empty() => {
            if statement
                .columns()
                .iter()
                .all(|column| supported_pg_type(column.type_()))
            {
                postgres_query(client, &statement).await
            } else {
                // A type we cannot decode still has a text form; use the simple
                // protocol so values and NULLs are shown instead of placeholders.
                postgres_simple_tables(client, source, statement.columns()).await
            }
        }
        Ok(statement) => {
            let changed = client
                .execute(&statement, &[])
                .await
                .map_err(|error| pg_error("PostgreSQL could not run the statement", error))?;
            Ok(vec![affected_table(changed)])
        }
        Err(_) => {
            // Multiple statements cannot be prepared together. Run them with
            // simple query so setup scripts still work.
            postgres_simple_tables(client, source, &[]).await
        }
    }
}

async fn postgres_query(
    client: &tokio_postgres::Client,
    statement: &tokio_postgres::Statement,
) -> Result<Vec<TableData>, String> {
    use futures_util::TryStreamExt;

    let columns: Vec<(String, tokio_postgres::types::Type)> = statement
        .columns()
        .iter()
        .map(|column| (column.name().to_owned(), column.type_().clone()))
        .collect();
    let rows = client
        .query_raw(statement, std::iter::empty::<&str>())
        .await
        .map_err(|error| pg_error("PostgreSQL could not run the query", error))?;
    tokio::pin!(rows);
    let mut values = Vec::new();
    let mut truncated = false;
    loop {
        let row = rows
            .as_mut()
            .try_next()
            .await
            .map_err(|error| pg_error("PostgreSQL could not read the results", error))?;
        let Some(row) = row else {
            break;
        };
        if values.len() >= MAX_ROWS {
            // Keep reading so later result sets and writes still happen.
            truncated = true;
            continue;
        }
        let mut cells = Vec::with_capacity(columns.len());
        for (index, (_, kind)) in columns.iter().enumerate() {
            cells.push(pg_cell(&row, index, kind, values.len()));
        }
        values.push(cells);
    }
    Ok(vec![TableData {
        columns: columns
            .into_iter()
            .map(|(name, _)| Column { name })
            .collect(),
        rows: values,
        truncated,
    }])
}

fn supported_pg_type(kind: &tokio_postgres::types::Type) -> bool {
    matches!(
        kind.name(),
        "bool"
            | "int2"
            | "int4"
            | "int8"
            | "oid"
            | "float4"
            | "float8"
            | "time"
            | "uuid"
            | "json"
            | "jsonb"
            | "bytea"
            | "text"
            | "varchar"
            | "bpchar"
            | "name"
            | "char"
            | "unknown"
    )
}

async fn postgres_simple_tables(
    client: &tokio_postgres::Client,
    source: &str,
    column_types: &[tokio_postgres::Column],
) -> Result<Vec<TableData>, String> {
    use futures_util::TryStreamExt;

    let messages = client
        .simple_query_raw(source)
        .await
        .map_err(|error| pg_error("PostgreSQL could not run the query", error))?;
    tokio::pin!(messages);
    let mut tables: Vec<TableData> = Vec::new();
    let mut current: Option<(Vec<Column>, Vec<Vec<Value>>, bool)> = None;
    while let Some(message) = messages
        .as_mut()
        .try_next()
        .await
        .map_err(|error| pg_error("PostgreSQL could not read the results", error))?
    {
        match message {
            tokio_postgres::SimpleQueryMessage::RowDescription(columns) => {
                if let Some((columns, rows, truncated)) = current.take() {
                    tables.push(TableData {
                        columns,
                        rows,
                        truncated,
                    });
                }
                current = Some((
                    columns
                        .iter()
                        .map(|column| Column {
                            name: column.name().to_owned(),
                        })
                        .collect(),
                    Vec::new(),
                    false,
                ));
            }
            tokio_postgres::SimpleQueryMessage::Row(row) => {
                if let Some((_, rows, truncated)) = current.as_mut() {
                    if rows.len() >= MAX_ROWS {
                        *truncated = true;
                        continue;
                    }
                    let mut cells = Vec::with_capacity(row.columns().len());
                    for (index, _) in row.columns().iter().enumerate() {
                        cells.push(pg_text_cell(
                            row.get(index),
                            column_types.get(index).map(|column| column.type_()),
                            rows.len(),
                            index,
                        ));
                    }
                    rows.push(cells);
                }
            }
            tokio_postgres::SimpleQueryMessage::CommandComplete(count) => {
                if let Some((columns, rows, truncated)) = current.take() {
                    tables.push(TableData {
                        columns,
                        rows,
                        truncated,
                    });
                } else if count > 0 {
                    tables.push(affected_table(count));
                }
            }
            _ => {}
        }
    }
    if let Some((columns, rows, truncated)) = current {
        tables.push(TableData {
            columns,
            rows,
            truncated,
        });
    }
    if tables.is_empty() {
        tables.push(affected_table_text("Statement completed.".into()));
    }
    Ok(tables)
}

fn affected_table(changed: u64) -> TableData {
    TableData {
        columns: vec![Column {
            name: "rows affected".into(),
        }],
        rows: vec![vec![
            json!({"id": "sql:affected", "kind": "primitive", "v": changed}),
        ]],
        truncated: false,
    }
}

fn affected_table_text(text: String) -> TableData {
    TableData {
        columns: vec![Column {
            name: "result".into(),
        }],
        rows: vec![vec![
            json!({"id": "sql:result", "kind": "string", "v": text}),
        ]],
        truncated: false,
    }
}

fn pg_detail(error: &tokio_postgres::Error) -> String {
    if let Some(database) = error.as_db_error() {
        return format!(
            "{} (SQLSTATE {})",
            database.message(),
            database.code().code()
        );
    }
    let mut detail = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        let text = cause.to_string();
        if !text.is_empty() && !detail.contains(&text) {
            detail = format!("{detail}: {text}");
        }
        source = cause.source();
    }
    detail
}

fn pg_error(context: &str, error: tokio_postgres::Error) -> String {
    format!("{context}: {}", pg_detail(&error))
}

fn null_node(id: &str) -> Value {
    json!({"id": id, "kind": "null"})
}

fn primitive_node(id: &str, value: Value) -> Value {
    json!({"id": id, "kind": "primitive", "v": value})
}

fn integer_node(id: &str, value: i64) -> Value {
    if value.unsigned_abs() > MAX_SAFE_INTEGER as u64 {
        string_node(id.to_owned(), value.to_string(), Some("int8"), None)
    } else {
        primitive_node(id, json!(value))
    }
}

fn bounded_text(id: &str, text: String, kind: Option<&str>) -> Value {
    let total = text.chars().count();
    if total > MAX_CELL_TEXT {
        let mut node = string_node(
            id.to_owned(),
            text.chars().take(MAX_CELL_TEXT).collect(),
            kind,
            Some(total),
        );
        // Kept on the host for exports; stripped before a page reaches the UI.
        node[FULL_TEXT] = Value::String(text);
        node
    } else {
        string_node(id.to_owned(), text, kind, None)
    }
}

/// Host-only field holding a truncated cell's complete text.
const FULL_TEXT: &str = "full";

fn pg_cell(
    row: &tokio_postgres::Row,
    column: usize,
    kind: &tokio_postgres::types::Type,
    row_index: usize,
) -> Value {
    let id = format!("sql:r{row_index}c{column}");
    if let Ok(value) = row.try_get::<_, Option<bool>>(column) {
        return value.map_or_else(|| null_node(&id), |value| primitive_node(&id, json!(value)));
    }
    if let Ok(value) = row.try_get::<_, Option<i16>>(column) {
        return value.map_or_else(|| null_node(&id), |value| integer_node(&id, value as i64));
    }
    if let Ok(value) = row.try_get::<_, Option<i32>>(column) {
        return value.map_or_else(|| null_node(&id), |value| integer_node(&id, value as i64));
    }
    if let Ok(value) = row.try_get::<_, Option<i64>>(column) {
        return value.map_or_else(|| null_node(&id), |value| integer_node(&id, value));
    }
    if let Ok(value) = row.try_get::<_, Option<u32>>(column) {
        return value.map_or_else(|| null_node(&id), |value| integer_node(&id, value as i64));
    }
    if let Ok(value) = row.try_get::<_, Option<f32>>(column) {
        return match value {
            None => null_node(&id),
            Some(value) if !value.is_finite() => bounded_text(&id, value.to_string(), None),
            Some(value) => primitive_node(
                &id,
                json!(value.to_string().parse::<f64>().unwrap_or(value as f64)),
            ),
        };
    }
    if let Ok(value) = row.try_get::<_, Option<f64>>(column) {
        return match value {
            None => null_node(&id),
            Some(value) if value.is_nan() => bounded_text(&id, "NaN".into(), None),
            Some(value) if value.is_infinite() => bounded_text(
                &id,
                if value.is_sign_negative() {
                    "-Infinity".into()
                } else {
                    "Infinity".into()
                },
                None,
            ),
            Some(value) => primitive_node(&id, json!(value)),
        };
    }
    if let Ok(value) = row.try_get::<_, Option<chrono::NaiveTime>>(column) {
        return value.map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, value.to_string(), Some("time")),
        );
    }
    if let Ok(value) = row.try_get::<_, Option<uuid::Uuid>>(column) {
        return value.map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, value.to_string(), Some("uuid")),
        );
    }
    if let Ok(value) = row.try_get::<_, Option<serde_json::Value>>(column) {
        return value.map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, value.to_string(), Some(kind.name())),
        );
    }
    if let Ok(value) = row.try_get::<_, Option<Vec<u8>>>(column) {
        return value.map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, hex(&value), Some("bytea")),
        );
    }
    if let Ok(value) = row.try_get::<_, Option<String>>(column) {
        return value.map_or_else(
            || null_node(&id),
            |value| bounded_text(&id, value, Some(kind.name())),
        );
    }
    bounded_text(&id, format!("<{}>", kind.name()), None)
}

fn pg_text_cell(
    text: Option<&str>,
    kind: Option<&tokio_postgres::types::Type>,
    row: usize,
    column: usize,
) -> Value {
    let id = format!("sql:r{row}c{column}");
    let Some(text) = text else {
        return null_node(&id);
    };
    // Simple-query rows are text, but a prepared single statement still supplies
    // types. Preserve primitive cells even when another column needs text decoding.
    match kind.map(|kind| kind.name()) {
        Some("bool") => match text {
            "t" => return primitive_node(&id, json!(true)),
            "f" => return primitive_node(&id, json!(false)),
            _ => {}
        },
        Some("int2" | "int4" | "int8") => {
            if let Ok(value) = text.parse::<i64>() {
                return integer_node(&id, value);
            }
        }
        Some("oid") => {
            if let Ok(value) = text.parse::<u32>() {
                return integer_node(&id, value as i64);
            }
        }
        Some("float4" | "float8") => {
            if let Ok(value) = text.parse::<f64>() {
                if value.is_finite() {
                    return primitive_node(&id, json!(value));
                }
            }
        }
        _ => {}
    }
    let type_name = kind
        .filter(|kind| kind.name() == "numeric")
        .map(|kind| kind.name());
    bounded_text(&id, text.to_owned(), type_name)
}

fn is_interrupted(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == rusqlite::ErrorCode::OperationInterrupted
    )
}

const MAX_SAFE_INTEGER: i64 = 2_i64.pow(53) - 1;
const MAX_CELL_TEXT: usize = 1_000;

fn cell_node(value: ValueRef<'_>, row: usize, column: usize) -> Value {
    let id = format!("sql:r{row}c{column}");
    match value {
        ValueRef::Null => json!({"id": id, "kind": "null"}),
        ValueRef::Integer(value) if value.unsigned_abs() > MAX_SAFE_INTEGER as u64 => {
            string_node(id, value.to_string(), Some("int"), None)
        }
        ValueRef::Integer(value) => json!({"id": id, "kind": "primitive", "v": value}),
        ValueRef::Real(value) if value.is_finite() => {
            json!({"id": id, "kind": "primitive", "v": value})
        }
        ValueRef::Real(value) => string_node(id, value.to_string(), None, None),
        ValueRef::Text(value) => {
            bounded_text(&id, String::from_utf8_lossy(value).into_owned(), None)
        }
        ValueRef::Blob(value) => bounded_text(&id, hex(value), Some("blob")),
    }
}

fn string_node(id: String, value: String, kind: Option<&str>, total: Option<usize>) -> Value {
    let mut node = json!({"id": id, "kind": "string", "v": value});
    if let Some(kind) = kind {
        node["type"] = Value::String(kind.into());
    }
    if let Some(total) = total {
        node["truncated"] = Value::Bool(true);
        node["total"] = Value::Number(total.into());
    }
    node
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(2 + bytes.len() * 2);
    text.push_str("0x");
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn table_node(table: &TableData, handle: &str, offset: usize, limit: usize) -> Value {
    let offset = offset.min(table.rows.len());
    let end = (offset + limit).min(table.rows.len());
    let rows = table.rows[offset..end]
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| {
                    let mut cell = cell.clone();
                    if let Some(object) = cell.as_object_mut() {
                        object.remove(FULL_TEXT);
                    }
                    cell
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let next = (end < table.rows.len()).then(|| end.to_string());
    let mut node = json!({
        "id": format!("{handle}:{offset}"),
        "kind": "table",
        "type": if table.truncated { "query (truncated)" } else { "query" },
        "columns": table.columns.iter().map(|column| json!({"name": column.name})).collect::<Vec<_>>(),
        "rows": rows,
        "total": table.rows.len(),
        "next": next,
        "handle": handle,
    });
    if table.truncated {
        node["truncated"] = Value::Bool(true);
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{SecretOrigin, Vault};
    use std::collections::BTreeMap;

    #[test]
    fn sqlserver_go_detection_respects_sql_tokens() {
        for sql in [
            "GO",
            " go 5 ",
            "SELECT 1;\nGO 5\nSELECT 2",
            "/* comment */ GO 2 -- repeat",
            "GO\r\n",
        ] {
            assert!(has_go_batch(sql), "{sql:?}");
        }
        for sql in [
            "SELECT 'a\nGO\nb'",
            "/*\nGO\n*/ SELECT 1",
            "/* outer /* inner */\nGO 2\n*/",
            "SELECT [a\nGO\nb]",
            "SELECT \"a\nGO\nb\"",
            "-- GO",
            "SELECT 'it''s\nGO\nfine'",
            "GO away",
            "GOTO label",
        ] {
            assert!(!has_go_batch(sql), "{sql:?}");
        }
    }

    #[test]
    fn sqlserver_datetime_rounds_storage_ticks_to_milliseconds() {
        let base = chrono::NaiveDate::from_ymd_opt(2020, 1, 2)
            .unwrap()
            .and_hms_opt(3, 4, 5)
            .unwrap();
        for (ticks, fraction) in [
            (0, "000"),
            (1, "003"),
            (2, "007"),
            (37, "123"),
            (299, "997"),
        ] {
            let value = base + chrono::Duration::nanoseconds(ticks * 1_000_000_000 / 300);
            assert_eq!(
                sqlserver_datetime_text(value, tiberius::ColumnType::Datetime),
                format!("2020-01-02T03:04:05.{fraction}")
            );
        }
        let precise = base + chrono::Duration::nanoseconds(123_456_700);
        assert_eq!(
            sqlserver_datetime_text(precise, tiberius::ColumnType::Datetime2),
            "2020-01-02T03:04:05.123456700"
        );
    }

    #[test]
    fn postgres_text_fallback_preserves_known_primitives_and_exact_text() {
        use tokio_postgres::types::Type;
        for (text, kind, expected) in [
            ("t", Type::BOOL, json!(true)),
            ("f", Type::BOOL, json!(false)),
            ("-32768", Type::INT2, json!(-32768)),
            ("2147483647", Type::INT4, json!(2147483647)),
            ("1", Type::INT8, json!(1)),
            ("4294967295", Type::OID, json!(4294967295_u64)),
            ("1.1", Type::FLOAT4, json!(1.1)),
            ("1.25", Type::FLOAT8, json!(1.25)),
            ("NaN", Type::FLOAT8, json!("NaN")),
            ("Infinity", Type::FLOAT8, json!("Infinity")),
            ("-Infinity", Type::FLOAT8, json!("-Infinity")),
            (
                "-9223372036854775808",
                Type::INT8,
                json!("-9223372036854775808"),
            ),
            ("9007199254740993", Type::INT8, json!("9007199254740993")),
            ("12.50", Type::NUMERIC, json!("12.50")),
            ("infinity", Type::DATE, json!("infinity")),
            ("-infinity", Type::TIMESTAMP, json!("-infinity")),
            ("t", Type::TEXT, json!("t")),
        ] {
            assert_eq!(
                pg_text_cell(Some(text), Some(&kind), 0, 0)["v"],
                expected,
                "{kind}: {text}"
            );
            assert_eq!(pg_text_cell(None, Some(&kind), 0, 0)["kind"], "null");
        }
        assert_eq!(pg_text_cell(Some("1"), None, 0, 0)["v"], "1");
    }

    struct MemoryVault(Mutex<BTreeMap<String, String>>);
    impl Vault for MemoryVault {
        fn get(&self, name: &str) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(name).cloned())
        }
        fn set(&self, name: &str, value: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .insert(name.to_owned(), value.to_owned());
            Ok(())
        }
        fn delete(&self, name: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(name);
            Ok(())
        }
    }

    fn setup() -> (
        tempfile::TempDir,
        Arc<ConnectionStore>,
        Arc<SecretStore>,
        SqlManager,
        String,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("sample.db");
        let sqlite = SqliteConnection::open(&database).unwrap();
        sqlite
            .execute_batch(
                "CREATE TABLE people (name TEXT, score INTEGER);
                 INSERT INTO people VALUES ('Ada', 10), ('Grace', 20);",
            )
            .unwrap();
        let connections = Arc::new(ConnectionStore::new(directory.path()));
        connections
            .save(Connection {
                id: String::new(),
                name: "sample".into(),
                kind: DatabaseKind::Sqlite,
                tls: TlsMode::Prefer,
                trust_server_certificate: false,
                path: Some(database.to_string_lossy().into_owned()),
                host: None,
                port: None,
                database: None,
                user: None,
                secret: None,
            })
            .unwrap();
        let secrets = Arc::new(SecretStore::with_vault(
            directory.path(),
            Arc::new(MemoryVault(Mutex::new(BTreeMap::new()))),
        ));
        let manager = SqlManager::new(connections.clone(), secrets.clone());
        (
            directory,
            connections,
            secrets,
            manager,
            database.to_string_lossy().into_owned(),
        )
    }

    #[test]
    fn connection_store_persists_without_passwords() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConnectionStore::new(directory.path());
        let saved = store
            .save(Connection {
                id: String::new(),
                name: "sample".into(),
                kind: DatabaseKind::Sqlite,
                tls: TlsMode::Prefer,
                trust_server_certificate: false,
                path: Some("/tmp/sample.db".into()),
                host: None,
                port: None,
                database: None,
                user: None,
                secret: Some("connection:sample".into()),
            })
            .unwrap();
        assert_eq!(store.by_name("sample").unwrap().secret, None);
        store
            .save(Connection {
                id: String::new(),
                name: "pg".into(),
                kind: DatabaseKind::Postgres,
                tls: TlsMode::Prefer,
                trust_server_certificate: false,
                path: None,
                host: Some("db.example.com".into()),
                port: None,
                database: Some("sample".into()),
                user: Some("reader".into()),
                secret: Some("connection:pg".into()),
            })
            .unwrap();
        assert_eq!(store.list().len(), 2);
        assert_eq!(store.by_name("sample").unwrap().id, saved.id);
        assert!(store
            .save(Connection {
                id: String::new(),
                name: "sample".into(),
                kind: DatabaseKind::Sqlite,
                tls: TlsMode::Prefer,
                trust_server_certificate: false,
                path: Some("/tmp/other.db".into()),
                host: None,
                port: None,
                database: None,
                user: None,
                secret: None,
            })
            .is_err());
        let text = fs::read_to_string(directory.path().join(CONNECTIONS_FILE)).unwrap();
        assert!(text.contains("connection:pg"));
        assert!(!text.contains("connection:sample"));
        let reloaded = ConnectionStore::new(directory.path());
        assert_eq!(reloaded.list().len(), 2);
        reloaded.delete(&saved.id).unwrap();
        assert_eq!(reloaded.list().len(), 1);
    }

    #[tokio::test]
    async fn sqlite_queries_page_and_report_affected_rows() {
        let (_directory, _connections, _secrets, manager, _database) = setup();
        let result = manager
            .run(
                "sql",
                "-- codingpad: connection=sample\nSELECT name, score FROM people ORDER BY name",
                None,
            )
            .await
            .unwrap();
        assert!(result.ok);
        let table = &result.tables[0];
        assert_eq!(table["columns"].as_array().unwrap().len(), 2);
        assert_eq!(table["rows"][0][0]["v"], "Ada");
        assert_eq!(table["total"], 2);
        assert!(table["next"].is_null());
        let handle = table["handle"].as_str().unwrap().to_owned();
        let page = manager.expand("sql", &handle, Some("1"), Some(1)).unwrap();
        assert_eq!(page["rows"].as_array().unwrap().len(), 1);
        assert_eq!(page["rows"][0][1]["v"], 20);

        let changed = manager
            .run(
                "sql",
                "-- codingpad: connection=sample\nINSERT INTO people VALUES ('Barbara', 30)",
                None,
            )
            .await
            .unwrap();
        assert_eq!(changed.tables[0]["rows"][0][0]["v"], 1);
    }

    #[tokio::test]
    async fn sqlite_handles_comments_multiple_statements_and_returning() {
        let (_directory, _connections, _secrets, manager, _database) = setup();
        let run = |sql: &'static str| {
            let manager = &manager;
            async move {
                manager
                    .run(
                        "sql",
                        &format!("-- codingpad: connection=sample\n{sql}"),
                        None,
                    )
                    .await
                    .unwrap()
            }
        };

        let comment = run("-- top rows\nSELECT name FROM people ORDER BY name").await;
        assert_eq!(comment.tables.len(), 1);
        assert_eq!(comment.tables[0]["rows"].as_array().unwrap().len(), 2);

        let block = run("/* c */ SELECT name FROM people ORDER BY name").await;
        assert_eq!(block.tables.len(), 1);
        assert_eq!(block.tables[0]["rows"].as_array().unwrap().len(), 2);

        let batch =
            run("INSERT INTO people VALUES ('X', 1); SELECT name FROM people ORDER BY name").await;
        assert_eq!(batch.tables.len(), 2);
        assert_eq!(batch.tables[0]["rows"][0][0]["v"], 1);
        assert_eq!(batch.tables[1]["rows"].as_array().unwrap().len(), 3);

        let returning = run("INSERT INTO people VALUES ('Y', 2) RETURNING name").await;
        assert_eq!(returning.tables.len(), 1);
        assert_eq!(returning.tables[0]["rows"][0][0]["v"], "Y");

        let two = run("SELECT 1 AS a; SELECT 99 AS b;").await;
        assert_eq!(two.tables.len(), 2);
        assert_eq!(two.tables[0]["rows"][0][0]["v"], 1);
        assert_eq!(two.tables[1]["rows"][0][0]["v"], 99);
    }

    #[tokio::test]
    async fn sqlite_cells_keep_large_integers_exact_and_cap_large_text() {
        let (_directory, _connections, _secrets, manager, _database) = setup();
        let result = manager
            .run(
                "sql",
                "-- codingpad: connection=sample\nSELECT 9007199254740993 AS id, replace(printf('%0*d', 2000, 0), '0', 'x') AS big, -9223372036854775807 - 1 AS minimum",
                None,
            )
            .await
            .unwrap();
        let cells = &result.tables[0]["rows"][0];
        assert_eq!(cells[0]["kind"], "string");
        assert_eq!(cells[0]["v"], "9007199254740993");
        assert!(cells[0].get("truncated").is_none());
        assert_eq!(cells[2]["v"], "-9223372036854775808");
        assert_eq!(cells[1]["truncated"], true);
        assert_eq!(cells[1]["total"], 2000);
        assert_eq!(cells[1]["v"].as_str().unwrap().chars().count(), 1000);
        assert!(
            cells[1].get("full").is_none(),
            "full text stays on the host"
        );
        let handle = result.tables[0]["handle"].as_str().unwrap();
        let page = manager.export_page("sql", handle, None, 1000).unwrap();
        assert_eq!(page.columns, ["id", "big", "minimum"]);
        assert_eq!(page.rows[0][0], json!({"n": "9007199254740993"}));
        assert_eq!(page.rows[0][1].as_str().unwrap().chars().count(), 2000);
        assert_eq!(page.next, None);
        assert!(manager.export_page("other", handle, None, 1000).is_err());
    }

    #[tokio::test]
    async fn sqlite_results_export_every_row_in_pages() {
        let (_directory, _connections, _secrets, manager, _database) = setup();
        let result = manager
            .run(
                "sql",
                "-- codingpad: connection=sample\nWITH RECURSIVE t(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM t WHERE n < 2500) SELECT n FROM t",
                None,
            )
            .await
            .unwrap();
        let handle = result.tables[0]["handle"].as_str().unwrap();
        let first = manager.export_page("sql", handle, None, 1000).unwrap();
        assert_eq!(first.rows.len(), 1000);
        assert_eq!(first.next.as_deref(), Some("1000"));
        assert_eq!(first.total, Some(2500));
        let last = manager
            .export_page("sql", handle, Some("2000"), 1000)
            .unwrap();
        assert_eq!(last.rows.len(), 500);
        assert_eq!(last.rows[499][0], json!(2500));
        assert_eq!(last.next, None);
    }

    #[tokio::test]
    async fn an_idle_stop_does_not_cancel_the_next_run() {
        let (_directory, _connections, _secrets, manager, _database) = setup();
        manager
            .run("sql", "-- codingpad: connection=sample\nSELECT 1", None)
            .await
            .unwrap();
        manager.stop("sql");
        let result = manager
            .run(
                "sql",
                "-- codingpad: connection=sample\nINSERT INTO people VALUES ('Idle', 1)",
                None,
            )
            .await
            .unwrap();
        assert!(result.ok, "an idle Stop must not affect the next Run");
        assert!(!result.stopped);
        let count = manager
            .run(
                "sql",
                "-- codingpad: connection=sample\nSELECT count(*) FROM people WHERE name = 'Idle'",
                None,
            )
            .await
            .unwrap();
        assert_eq!(count.tables[0]["rows"][0][0]["v"], 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_interrupts_a_running_query_and_an_early_stop_is_kept() {
        use std::time::Duration;
        let (_directory, _connections, _secrets, manager, _database) = setup();
        let manager = Arc::new(manager);
        let endless = "-- codingpad: connection=sample\nWITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c";
        manager.begin_run("sql");
        let run = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.run("sql", endless, None).await })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        manager.stop("sql");
        let result = tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("stop should interrupt the query")
            .unwrap()
            .unwrap();
        assert!(result.stopped);
        assert!(!result.ok);

        manager.begin_run("sql");
        let run = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.run("sql", endless, None).await })
        };
        tokio::task::yield_now().await;
        manager.stop("sql");
        let result = tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .expect("an early stop should also interrupt the query")
            .unwrap()
            .unwrap();
        assert!(result.stopped);
    }

    #[tokio::test]
    async fn sql_errors_and_missing_connections_are_clear() {
        let (_directory, _connections, _secrets, manager, _database) = setup();
        assert!(manager
            .run("sql", "SELECT 1", None)
            .await
            .unwrap_err()
            .contains("connection=<name>"));
        assert!(manager
            .run("sql", "-- codingpad: connection=missing\nSELECT 1", None)
            .await
            .unwrap_err()
            .contains("Connections"));
        assert!(manager
            .run(
                "sql",
                "-- codingpad: connection=sample\nSELECT nope FROM people",
                None
            )
            .await
            .unwrap_err()
            .contains("no such column"));
    }

    #[tokio::test]
    async fn test_connection_reports_missing_and_existing_files() {
        let (_directory, connections, _secrets, manager, _database) = setup();
        let id = connections.by_name("sample").unwrap().id;
        assert!(manager.test(&id).await.unwrap().contains("Connected"));
        let missing = connections
            .save(Connection {
                id: String::new(),
                name: "missing".into(),
                kind: DatabaseKind::Sqlite,
                tls: TlsMode::Prefer,
                trust_server_certificate: false,
                path: Some("/definitely/not/here.db".into()),
                host: None,
                port: None,
                database: None,
                user: None,
                secret: None,
            })
            .unwrap();
        assert!(manager
            .test(&missing.id)
            .await
            .unwrap()
            .contains("will be created"));
    }

    #[tokio::test]
    async fn stopping_clears_running_results_and_keeps_new_runs_working() {
        let (_directory, _connections, _secrets, manager, _database) = setup();
        manager
            .run("sql", "-- codingpad: connection=sample\nSELECT 1", None)
            .await
            .unwrap();
        manager.reset("sql");
        assert!(manager.expand("sql", "sql-1", None, None).is_err());
        let result = manager
            .run("sql", "-- codingpad: connection=sample\nSELECT 2", None)
            .await
            .unwrap();
        assert!(result.ok);
    }

    #[test]
    fn postgres_connections_require_server_fields() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConnectionStore::new(directory.path());
        let connection = Connection {
            id: String::new(),
            name: "pg".into(),
            kind: DatabaseKind::Postgres,
            tls: TlsMode::Prefer,
            trust_server_certificate: false,
            path: None,
            host: None,
            port: None,
            database: Some("northwind".into()),
            user: Some("reader".into()),
            secret: None,
        };
        assert!(store.save(connection).unwrap_err().contains("host"));
        let saved = store
            .save(Connection {
                id: String::new(),
                name: "pg".into(),
                kind: DatabaseKind::Postgres,
                tls: TlsMode::Prefer,
                trust_server_certificate: false,
                path: None,
                host: Some("db.example.com".into()),
                port: Some(5433),
                database: Some("northwind".into()),
                user: Some("reader".into()),
                secret: Some("connection:pg".into()),
            })
            .unwrap();
        assert_eq!(saved.port, Some(5433));
        let text = fs::read_to_string(directory.path().join(CONNECTIONS_FILE)).unwrap();
        assert!(text.contains("connection:pg"));
        assert!(!text.contains("password"));
    }

    #[tokio::test]
    #[ignore = "requires CODINGPAD_TEST_POSTGRES_HOST/PORT/DATABASE/USER/PASSWORD"]
    async fn postgres_integration_round_trip_and_cancel() {
        use std::time::Duration;

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
        let directory = tempfile::tempdir().unwrap();
        let secrets = Arc::new(SecretStore::with_vault(
            directory.path(),
            Arc::new(MemoryVault(Mutex::new(BTreeMap::new()))),
        ));
        secrets.set_session("connection:pg", &password).unwrap();
        let connections = Arc::new(ConnectionStore::new(directory.path()));
        connections
            .save(Connection {
                id: String::new(),
                name: "pg".into(),
                kind: DatabaseKind::Postgres,
                tls: TlsMode::Prefer,
                trust_server_certificate: false,
                path: None,
                host: Some(host),
                port: Some(port.parse().unwrap()),
                database: Some(database),
                user: Some(user),
                secret: Some("connection:pg".into()),
            })
            .unwrap();
        let manager = Arc::new(SqlManager::new(connections, secrets));
        let header = "-- codingpad: connection=pg\n";
        let table = format!("codingpad_m3_2_{}", std::process::id());

        manager
            .run(
                "pg",
                &format!("{header}DROP TABLE IF EXISTS {table}; CREATE TABLE {table} (n integer PRIMARY KEY)"),
                None,
            )
            .await
            .unwrap();
        manager
            .run(
                "pg-schema",
                &format!(
                    "{header}CREATE TABLE {table}_child (parent integer REFERENCES {table}(n))"
                ),
                None,
            )
            .await
            .unwrap();
        // Keep a different backend's temporary namespace alive during discovery.
        let (temp_client, _) = manager
            .connect_postgres(&manager.connections.by_name("pg").unwrap())
            .await
            .unwrap();
        temp_client
            .batch_execute("CREATE TEMP TABLE hidden_temp(id integer)")
            .await
            .unwrap();
        let schema = manager.schema("pg").await.unwrap();
        assert!(!schema.tables.iter().any(|item| item.name == "hidden_temp"));
        drop(temp_client);
        let child = schema
            .tables
            .iter()
            .find(|item| item.name == format!("{table}_child"))
            .unwrap();
        assert_eq!(child.columns[0].name, "parent");
        assert_eq!(child.columns[0].references[0].table, table);
        assert_eq!(child.columns[0].references[0].column, "n");
        assert!(!child.columns[0].data_type.is_empty());
        manager
            .run(
                "pg-schema",
                &format!("{header}DROP TABLE {table}_child"),
                None,
            )
            .await
            .unwrap();
        let typed = manager
            .run(
                "pg",
                &format!(
                    "{header}SELECT 1::int8 AS n, 12.50::numeric AS amount, now()::date AS day, \
                     gen_random_uuid() AS id, '{{\"a\":1}}'::jsonb AS data, NULL::numeric AS missing, \
                     1.1::float4 AS f, 'NaN'::float8 AS nan, true AS b, \
                     '-9223372036854775808'::int8 AS min, 9007199254740993::int8 AS big, \
                     '123456789012345678901234567890.123456789'::numeric AS precise, \
                     'infinity'::date AS forever, '-infinity'::timestamp AS past"
                ),
                None,
            )
            .await
            .unwrap();
        let cells = &typed.tables[0]["rows"][0];
        assert_eq!(cells[0]["v"], 1);
        assert_eq!(cells[1]["v"], "12.50");
        assert!(cells[2]["v"].as_str().unwrap().len() >= 10);
        assert!(cells[3]["v"].as_str().unwrap().contains('-'));
        assert!(cells[4]["v"].as_str().unwrap().contains("\"a\""));
        assert_eq!(cells[5]["kind"], "null");
        assert_eq!(cells[6]["v"], 1.1);
        assert_eq!(cells[7]["v"], "NaN");
        assert_eq!(cells[8]["v"], true);
        assert_eq!(cells[9]["v"], "-9223372036854775808");
        assert_eq!(cells[10]["v"], "9007199254740993");
        assert_eq!(cells[11]["v"], "123456789012345678901234567890.123456789");
        assert_eq!(cells[12]["v"], "infinity");
        assert_eq!(cells[13]["v"], "-infinity");

        let stop = {
            let manager = manager.clone();
            let source = format!("{header}INSERT INTO {table} SELECT 1 FROM pg_sleep(5)");
            tokio::spawn(async move { manager.run("pg-stop", &source, None).await })
        };
        tokio::time::sleep(Duration::from_millis(300)).await;
        manager.stop("pg-stop");
        let stopped = tokio::time::timeout(Duration::from_secs(5), stop)
            .await
            .expect("PostgreSQL must confirm cancellation")
            .unwrap()
            .unwrap();
        assert!(stopped.stopped);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let check = manager
            .run(
                "pg-check",
                &format!("{header}SELECT count(*) AS n FROM {table}"),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            check.tables[0]["rows"][0][0]["v"], 0,
            "a stopped INSERT must not commit"
        );
        manager
            .run("pg", &format!("{header}DROP TABLE {table}"), None)
            .await
            .unwrap();
    }

    #[test]
    fn sqlserver_connections_require_server_fields_and_keep_the_trust_flag() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConnectionStore::new(directory.path());
        let connection = Connection {
            id: String::new(),
            name: "mssql".into(),
            kind: DatabaseKind::SqlServer,
            tls: TlsMode::Prefer,
            path: None,
            host: Some("db.example.com".into()),
            port: None,
            database: Some("northwind".into()),
            user: Some("sa".into()),
            secret: None,
            trust_server_certificate: true,
        };
        assert!(store.save(connection).is_ok());
        let saved = store.by_name("mssql").unwrap();
        assert!(saved.trust_server_certificate);
        let text = fs::read_to_string(directory.path().join(CONNECTIONS_FILE)).unwrap();
        assert!(text.contains("trustServerCertificate"));
        assert!(!text.contains("password"));
        let missing = Connection {
            id: String::new(),
            name: "broken".into(),
            kind: DatabaseKind::SqlServer,
            tls: TlsMode::Prefer,
            path: None,
            host: Some("db.example.com".into()),
            port: None,
            database: None,
            user: Some("sa".into()),
            secret: None,
            trust_server_certificate: false,
        };
        assert!(store.save(missing).unwrap_err().contains("database"));
    }

    #[tokio::test]
    #[ignore = "requires CODINGPAD_TEST_SQLSERVER_HOST/PORT/DATABASE/USER/PASSWORD"]
    async fn sqlserver_integration_round_trip_and_cancel() {
        use std::time::Duration;

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
        let directory = tempfile::tempdir().unwrap();
        let secrets = Arc::new(SecretStore::with_vault(
            directory.path(),
            Arc::new(MemoryVault(Mutex::new(BTreeMap::new()))),
        ));
        secrets.set_session("connection:mssql", &password).unwrap();
        let connections = Arc::new(ConnectionStore::new(directory.path()));
        connections
            .save(Connection {
                id: String::new(),
                name: "mssql".into(),
                kind: DatabaseKind::SqlServer,
                tls: TlsMode::Prefer,
                path: None,
                host: Some(host),
                port: Some(port.parse().unwrap()),
                database: Some(database),
                user: Some(user),
                secret: Some("connection:mssql".into()),
                trust_server_certificate: std::env::var("CODINGPAD_TEST_SQLSERVER_TRUST")
                    .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
            })
            .unwrap();
        let manager = Arc::new(SqlManager::new(connections, secrets));
        let header = "-- codingpad: connection=mssql\n";
        let table = format!("codingpad_m3_3_{}", std::process::id());

        manager
            .run(
                "mssql",
                &format!(
                    "{header}IF OBJECT_ID('{table}', 'U') IS NOT NULL DROP TABLE {table}; \
                     CREATE TABLE {table} (n int PRIMARY KEY)"
                ),
                None,
            )
            .await
            .unwrap();
        manager
            .run(
                "mssql-schema",
                &format!("{header}CREATE TABLE {table}_child (parent int REFERENCES {table}(n))"),
                None,
            )
            .await
            .unwrap();
        let schema = manager.schema("mssql").await.unwrap();
        let child = schema
            .tables
            .iter()
            .find(|item| item.name == format!("{table}_child"))
            .unwrap();
        assert_eq!(child.columns[0].name, "parent");
        assert_eq!(child.columns[0].references[0].table, table);
        assert_eq!(child.columns[0].references[0].column, "n");
        assert!(!child.columns[0].data_type.is_empty());
        manager
            .run(
                "mssql-schema",
                &format!("{header}DROP TABLE {table}_child"),
                None,
            )
            .await
            .unwrap();
        let typed = manager
            .run(
                "mssql",
                &format!(
                    "{header}SELECT CAST(1 AS bigint) AS n, CAST(12.5 AS decimal(10,2)) AS amount, \
                     CAST('2020-01-02' AS date) AS day, NEWID() AS id, N'hello' AS name, \
                     CAST(0x0102 AS varbinary(2)) AS bin, CAST(1 AS bit) AS flag, \
                     CAST(12.50 AS money) AS money, CAST('<a>1</a>' AS xml) AS xml, \
                     CAST('2020-01-02T03:04:05+02:00' AS datetimeoffset) AS offset, \
                     CAST(9007199254740993 AS bigint) AS big"
                ),
                None,
            )
            .await
            .unwrap();
        let cells = &typed.tables[0]["rows"][0];
        assert_eq!(cells[0]["v"], 1);
        assert!(cells[1]["v"].as_str().unwrap().contains("12.5"));
        assert!(cells[2]["v"].as_str().unwrap().contains("2020"));
        assert!(cells[3]["v"].as_str().unwrap().contains('-'));
        assert_eq!(cells[4]["v"], "hello");
        assert!(cells[5]["v"].as_str().unwrap().starts_with("0x"));
        assert_eq!(cells[6]["v"], true);
        assert_eq!(cells[7]["v"], "12.5000");
        assert_eq!(cells[8]["v"], "<a>1</a>");
        assert!(cells[9]["v"].as_str().unwrap().contains("+02:00"));
        assert_eq!(cells[10]["v"], "9007199254740993");

        let dates = manager
            .run(
                "mssql-dates",
                &format!(
                    "{header}SELECT \
            CAST('2020-01-02T03:04:05.007' AS datetime) AS a, \
            CAST('2020-01-02T03:04:05.997' AS datetime) AS b, \
            CAST(CAST('2020-01-02' AS date) AS sql_variant) AS c"
                ),
                None,
            )
            .await
            .unwrap();
        let cells = &dates.tables[0]["rows"][0];
        assert_eq!(cells[0]["v"], "2020-01-02T03:04:05.007");
        assert_eq!(cells[1]["v"], "2020-01-02T03:04:05.997");
        assert_eq!(cells[2]["v"], "2020-01-02");

        let stop = {
            let manager = manager.clone();
            let source = format!(
                "{header}BEGIN TRAN; INSERT INTO {table} VALUES (1); \
                 WAITFOR DELAY '00:00:05'; COMMIT;"
            );
            tokio::spawn(async move { manager.run("mssql-stop", &source, None).await })
        };
        tokio::time::sleep(Duration::from_millis(300)).await;
        manager.stop("mssql-stop");
        let stopped = tokio::time::timeout(Duration::from_secs(5), stop)
            .await
            .expect("SQL Server stop must return")
            .unwrap()
            .unwrap();
        assert!(stopped.stopped);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let check = manager
            .run(
                "mssql-check",
                &format!("{header}SELECT count(*) AS n FROM {table}"),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            check.tables[0]["rows"][0][0]["v"], 0,
            "a stopped transaction must roll back"
        );
        manager
            .run("mssql", &format!("{header}DROP TABLE {table}"), None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn sqlserver_go_batches_are_rejected_before_connecting() {
        let directory = tempfile::tempdir().unwrap();
        let secrets = Arc::new(SecretStore::with_vault(
            directory.path(),
            Arc::new(MemoryVault(Mutex::new(BTreeMap::new()))),
        ));
        let connections = Arc::new(ConnectionStore::new(directory.path()));
        connections
            .save(Connection {
                id: String::new(),
                name: "mssql".into(),
                kind: DatabaseKind::SqlServer,
                tls: TlsMode::Prefer,
                trust_server_certificate: false,
                path: None,
                host: Some("127.0.0.1".into()),
                port: Some(1433),
                database: Some("master".into()),
                user: Some("sa".into()),
                secret: None,
            })
            .unwrap();
        let manager = SqlManager::new(connections, secrets);
        let error = manager
            .run(
                "mssql",
                "-- codingpad: connection=mssql\nSELECT 1\nGO\nSELECT 2",
                None,
            )
            .await
            .unwrap_err();
        assert!(error.contains("GO"));
        assert!(error.contains("client-side batch separator"));
    }

    #[test]
    fn secrets_are_available_to_server_connections() {
        // SQLite needs no password, but the manager keeps the shared store so the
        // PostgreSQL and SQL Server implementations read the same vault entries.
        let (_directory, _connections, secrets, _manager, _database) = setup();
        assert_eq!(
            secrets.set("connection:sample", "hunter2").unwrap(),
            SecretOrigin::Vault
        );
        assert_eq!(secrets.get("connection:sample").unwrap(), "hunter2");
    }
}
