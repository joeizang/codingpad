use super::{wire, Dumps};
use crate::{
    attached_connection,
    databases::{
        Connection, ConnectionStore, ScriptDatabase, ScriptQuery, ScriptResult, TableData,
    },
    secrets::SecretStore,
};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::Value;
use std::{
    io::{self, Write},
    sync::Arc,
};
use tokio::sync::Mutex;

const MAX_REPLY: usize = 8 * 1024 * 1024;

pub(crate) struct Services {
    connection: Result<Connection, String>,
    secrets: Option<Arc<SecretStore>>,
    database: Mutex<Option<ScriptDatabase>>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Request {
    Query { value: String, parameters: String },
    Execute { value: String, parameters: String },
    Secret { value: String },
}

pub(super) enum Reply {
    Rows(TableData),
    Affected(u64),
    Secret(String),
    Error(String),
}

impl Services {
    pub(crate) fn new(
        source: &str,
        connections: Option<&ConnectionStore>,
        secrets: Option<Arc<SecretStore>>,
    ) -> Self {
        let connection = attached_connection(source)
            .ok_or_else(|| {
                "This Rust script has no database connection. Add a codingpad: connection header."
                    .to_owned()
            })
            .and_then(|name| {
                connections
                    .and_then(|store| store.by_name(&name))
                    .ok_or_else(|| {
                        format!("Connection \"{name}\" was not found. Add it in Connections…")
                    })
            });
        Self {
            connection,
            secrets,
            database: Mutex::new(None),
        }
    }

    pub(super) fn request(&self, request: Request) -> BoxFuture<'_, Reply> {
        Box::pin(async move {
            let result = match request {
                Request::Query { value, parameters } => self.query(value, parameters, false).await,
                Request::Execute { value, parameters } => self.query(value, parameters, true).await,
                Request::Secret { value } => self.secret(value).await.map(Reply::Secret),
            };
            result.unwrap_or_else(Reply::Error)
        })
    }

    pub(super) async fn close_database(&self) -> Result<(), String> {
        if let Some(mut database) = self.database.lock().await.take() {
            database.cancel().await?;
        }
        Ok(())
    }

    async fn query(&self, sql: String, parameters: String, execute: bool) -> Result<Reply, String> {
        let connection = self.connection.as_ref().map_err(Clone::clone)?;
        let mut database = self.database.lock().await;
        if database.is_none() {
            *database = Some(ScriptDatabase::open(connection, self.secrets.as_ref()).await?);
        }
        match database
            .as_mut()
            .unwrap()
            .run(ScriptQuery {
                sql,
                parameters,
                execute,
            })
            .await?
        {
            ScriptResult::Rows(rows) => Ok(Reply::Rows(rows)),
            ScriptResult::Affected(count) => Ok(Reply::Affected(count)),
        }
    }

    async fn secret(&self, name: String) -> Result<String, String> {
        let store = self
            .secrets
            .clone()
            .ok_or("The CodingPad password vault is unavailable.")?;
        tokio::task::spawn_blocking(move || store.get_for_script(&name))
            .await
            .map_err(|error| format!("Could not read the secret: {error}"))?
    }
}

impl Reply {
    pub(super) fn into_frame(self, dumps: &mut Dumps) -> Vec<u8> {
        match self {
            Self::Rows(rows) => {
                let handle = dumps.next_query();
                match encode_rows(handle, &rows) {
                    Ok(frame) => {
                        dumps.store_query(handle, rows, frame.len());
                        frame
                    }
                    Err(_) => error_reply(
                        "The Rust query result exceeds 8 MiB. Select fewer rows or columns.",
                    ),
                }
            }
            Self::Affected(count) => frame(0, &count.to_le_bytes()),
            Self::Secret(value) if value.len() <= MAX_REPLY => frame(0, value.as_bytes()),
            Self::Secret(_) => error_reply("The named secret exceeds the Rust reply size limit."),
            Self::Error(error) => error_reply(&error),
        }
    }
}

pub(super) fn error_reply(error: &str) -> Vec<u8> {
    let end = error.floor_char_boundary(error.len().min(MAX_REPLY));
    frame(1, &error.as_bytes()[..end])
}

fn frame(status: u8, payload: &[u8]) -> Vec<u8> {
    let mut reply = Vec::with_capacity(5 + payload.len());
    reply.push(status);
    reply.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    reply.extend_from_slice(payload);
    reply
}

struct RowFrame(Vec<u8>);

impl Write for RowFrame {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_REPLY + 5 - self.0.len() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.0.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode_rows(handle: u64, table: &TableData) -> io::Result<Vec<u8>> {
    // Same little-endian string and length encoding as Rust's CacheValue.
    let mut reply = RowFrame(vec![0; 5]);
    reply.write_all(&handle.to_le_bytes())?;
    reply.write_all(&[u8::from(table.truncated())])?;
    reply.write_all(&(table.column_names().count() as u32).to_le_bytes())?;
    for name in table.column_names() {
        text(&mut reply, name)?;
    }
    reply.write_all(&(table.row_count() as u32).to_le_bytes())?;
    for row in table.plain_rows() {
        for value in row {
            match value {
                Value::Null => reply.write_all(&[wire::NULL])?,
                Value::Bool(value) => reply.write_all(&[wire::BOOL, u8::from(value)])?,
                Value::Number(number) => {
                    if let Some(value) = number.as_i64() {
                        reply.write_all(&[wire::INTEGER])?;
                        reply.write_all(&value.to_le_bytes())?;
                    } else if let Some(value) = number.as_u64() {
                        reply.write_all(&[wire::UNSIGNED])?;
                        reply.write_all(&value.to_le_bytes())?;
                    } else if let Some(value) = number.as_f64() {
                        reply.write_all(&[wire::FLOAT])?;
                        reply.write_all(&value.to_le_bytes())?;
                    }
                }
                Value::String(value) => {
                    reply.write_all(&[wire::TEXT])?;
                    text(&mut reply, &value)?;
                }
                value => {
                    if let Some(number) = value.get("n").and_then(Value::as_str) {
                        if let Ok(integer) = number.parse::<i64>() {
                            reply.write_all(&[wire::INTEGER])?;
                            reply.write_all(&integer.to_le_bytes())?;
                        } else {
                            reply.write_all(&[wire::DECIMAL])?;
                            text(&mut reply, number)?;
                        }
                    } else {
                        reply.write_all(&[wire::TEXT])?;
                        text(&mut reply, &value.to_string())?;
                    }
                }
            }
        }
    }
    let length = (reply.0.len() - 5) as u32;
    reply.0[1..5].copy_from_slice(&length.to_le_bytes());
    Ok(reply.0)
}

fn text(reply: &mut RowFrame, value: &str) -> io::Result<()> {
    reply.write_all(&(value.len() as u32).to_le_bytes())?;
    reply.write_all(value.as_bytes())
}
