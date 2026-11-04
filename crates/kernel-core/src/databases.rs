//! Saved database connections and direct SQL execution.
//!
//! SQL scripts run through their connection without starting a kernel. Passwords
//! are referenced by secret name and read from the vault at connection time, so
//! connection settings never contain them.
use crate::{
    secrets::SecretStore,
    store::{parse_header, ScriptHeader},
};
use rusqlite::{types::ValueRef, Connection as SqliteConnection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseKind {
    Sqlite,
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
        }
        Ok(())
    }
}

pub struct ConnectionStore {
    path: PathBuf,
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
            connections: Mutex::new(connections),
        }
    }

    pub fn list(&self) -> Vec<Connection> {
        self.connections.lock().unwrap().clone()
    }

    pub fn get(&self, id: &str) -> Option<Connection> {
        self.connections
            .lock()
            .unwrap()
            .iter()
            .find(|connection| connection.id == id)
            .cloned()
    }

    pub fn by_name(&self, name: &str) -> Option<Connection> {
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

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let mut all = self.connections.lock().unwrap();
        let before = all.len();
        all.retain(|connection| connection.id != id);
        if all.len() == before {
            return Err("That connection no longer exists".into());
        }
        save_connections(&self.path, &all)
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
struct TableData {
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

pub struct SqlManager {
    connections: Arc<ConnectionStore>,
    _secrets: Arc<SecretStore>,
    results: Mutex<HashMap<String, Arc<TableData>>>,
    script_results: Mutex<HashMap<String, Vec<String>>>,
    interrupts: Mutex<HashMap<String, Interrupt>>,
    cancel_flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pending_stops: Mutex<HashSet<String>>,
    next_handle: AtomicU64,
}

impl SqlManager {
    pub fn new(connections: Arc<ConnectionStore>, secrets: Arc<SecretStore>) -> Self {
        Self {
            connections,
            _secrets: secrets,
            results: Mutex::new(HashMap::new()),
            script_results: Mutex::new(HashMap::new()),
            interrupts: Mutex::new(HashMap::new()),
            cancel_flags: Mutex::new(HashMap::new()),
            pending_stops: Mutex::new(HashSet::new()),
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
        // Register before any work so a Stop racing the connection open is kept.
        let cancelled = Arc::new(AtomicBool::new(false));
        self.cancel_flags
            .lock()
            .unwrap()
            .insert(script_id.to_owned(), cancelled.clone());
        if self.pending_stops.lock().unwrap().remove(script_id) {
            cancelled.store(true, Ordering::Relaxed);
        }
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

    pub fn stop(&self, script_id: &str) {
        let flag = self.cancel_flags.lock().unwrap().get(script_id).cloned();
        match flag {
            Some(cancelled) => cancelled.store(true, Ordering::Relaxed),
            None => {
                self.pending_stops
                    .lock()
                    .unwrap()
                    .insert(script_id.to_owned());
            }
        }
        if let Some(interrupt) = self.interrupts.lock().unwrap().get(script_id) {
            if let Some(handle) = interrupt.lock().unwrap().as_ref() {
                handle.interrupt();
            }
        }
    }

    /// Release every query and buffered result when the app exits.
    pub fn shutdown(&self) {
        self.results.lock().unwrap().clear();
        self.script_results.lock().unwrap().clear();
        self.interrupts.lock().unwrap().clear();
        self.cancel_flags.lock().unwrap().clear();
        self.pending_stops.lock().unwrap().clear();
    }

    pub fn reset(&self, script_id: &str) {
        self.stop(script_id);
        self.pending_stops.lock().unwrap().remove(script_id);
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
        let belongs = self
            .script_results
            .lock()
            .unwrap()
            .get(script_id)
            .is_some_and(|handles| handles.iter().any(|item| item == handle));
        if !belongs {
            return Err("That result is no longer available. Run the script again.".into());
        }
        let table = self
            .results
            .lock()
            .unwrap()
            .get(handle)
            .cloned()
            .ok_or_else(|| {
                "That result is no longer available. Run the script again.".to_owned()
            })?;
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
        if statement.column_count() > 0 {
            let columns: Vec<Column> = statement
                .column_names()
                .map(|name| Column {
                    name: name.to_owned(),
                })
                .collect();
            let mut rows = statement.query([]).map_err(|error| sqlite_error(&error))?;
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
            tables.push(TableData {
                columns,
                rows: values,
                truncated,
            });
        } else {
            let before = database.total_changes();
            statement
                .execute([])
                .map_err(|error| sqlite_error(&error))?;
            let changed = database.total_changes().saturating_sub(before);
            tables.push(affected_rows_table(changed));
        }
    }
    if tables.is_empty() {
        tables.push(affected_rows_table(0));
    }
    Ok(tables)
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
        ValueRef::Integer(value) if value.abs() > MAX_SAFE_INTEGER => string_node(
            id,
            value.to_string(),
            Some("int"),
            Some(value.to_string().chars().count()),
        ),
        ValueRef::Integer(value) => json!({"id": id, "kind": "primitive", "v": value}),
        ValueRef::Real(value) if value.is_finite() => {
            json!({"id": id, "kind": "primitive", "v": value})
        }
        ValueRef::Real(value) => string_node(id, value.to_string(), None, None),
        ValueRef::Text(value) => {
            let text = String::from_utf8_lossy(value);
            let total = text.chars().count();
            if total > MAX_CELL_TEXT {
                string_node(
                    id,
                    text.chars().take(MAX_CELL_TEXT).collect(),
                    None,
                    Some(total),
                )
            } else {
                string_node(id, text.into_owned(), None, None)
            }
        }
        ValueRef::Blob(value) => {
            let total = 2 + value.len() * 2;
            let text = hex(value);
            if text.chars().count() > MAX_CELL_TEXT {
                string_node(
                    id,
                    text.chars().take(MAX_CELL_TEXT).collect(),
                    Some("blob"),
                    Some(total),
                )
            } else {
                string_node(id, text, Some("blob"), None)
            }
        }
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
    let rows = table.rows[offset..end].to_vec();
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
                path: Some("/tmp/sample.db".into()),
                host: None,
                port: None,
                database: None,
                user: None,
                secret: Some("connection:sample".into()),
            })
            .unwrap();
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.by_name("sample").unwrap().id, saved.id);
        assert!(store
            .save(Connection {
                id: String::new(),
                name: "sample".into(),
                kind: DatabaseKind::Sqlite,
                path: Some("/tmp/other.db".into()),
                host: None,
                port: None,
                database: None,
                user: None,
                secret: None,
            })
            .is_err());
        let text = fs::read_to_string(directory.path().join(CONNECTIONS_FILE)).unwrap();
        assert!(text.contains("connection:sample"));
        let reloaded = ConnectionStore::new(directory.path());
        assert_eq!(reloaded.list().len(), 1);
        reloaded.delete(&saved.id).unwrap();
        assert!(reloaded.list().is_empty());
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
                "-- codingpad: connection=sample\nSELECT 9007199254740993 AS id, replace(printf('%0*d', 2000, 0), '0', 'x') AS big",
                None,
            )
            .await
            .unwrap();
        let cells = &result.tables[0]["rows"][0];
        assert_eq!(cells[0]["kind"], "string");
        assert_eq!(cells[0]["v"], "9007199254740993");
        assert_eq!(cells[1]["truncated"], true);
        assert_eq!(cells[1]["total"], 2000);
        assert_eq!(cells[1]["v"].as_str().unwrap().chars().count(), 1000);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_interrupts_a_running_query_and_an_early_stop_is_kept() {
        use std::time::Duration;
        let (_directory, _connections, _secrets, manager, _database) = setup();
        let manager = Arc::new(manager);
        let endless = "-- codingpad: connection=sample\nWITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c";
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
