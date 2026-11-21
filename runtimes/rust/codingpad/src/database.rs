use crate::{formatting_dump, request, CacheValue};
use std::fmt::{self, Debug};
use std::io::{self, Read};

/// CodingPad's database client. It opens the connection named in the script
/// header on the first query and uses it until the run ends.
#[allow(non_upper_case_globals)]
pub const db: Database = Database { _private: () };

pub struct Database {
    _private: (),
}

impl Database {
    /// Execute SQL and return its final result set with columns in query order.
    pub fn query(&self, sql: &str) -> Result<DbRows, String> {
        let bytes = request("query", sql)?;
        DbRows::decode(&bytes)
            .map_err(|error| format!("Could not read the database result: {error}"))
    }
}

/// Read a named secret from the current CodingPad vault or session.
pub fn secret(name: &str) -> String {
    request("secret", name)
        .and_then(|bytes| String::from_utf8(bytes).map_err(|error| error.to_string()))
        .unwrap_or_else(|error| panic!("{error}"))
}

/// Database values retain nulls, primitive types and exact numbers.
#[derive(Clone, Debug, PartialEq)]
pub enum DbValue {
    Null,
    Bool(bool),
    Integer(i64),
    Unsigned(u64),
    Float(f64),
    /// A number kept in decimal notation to avoid losing precision.
    Decimal(String),
    Text(String),
}

pub struct DbRows {
    handle: u64,
    columns: Vec<String>,
    rows: Vec<Vec<DbValue>>,
    truncated: bool,
}

impl DbRows {
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    pub fn rows(&self) -> &[Vec<DbValue>] {
        &self.rows
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    fn decode(mut bytes: &[u8]) -> io::Result<Self> {
        let handle = u64::decode(&mut bytes)?;
        let truncated = bool::decode(&mut bytes)?;
        let columns = Vec::<String>::decode(&mut bytes)?;
        let count = u32::decode(&mut bytes)? as usize;
        if count > 10_000 || count.saturating_mul(columns.len()) > bytes.len() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let rows = (0..count)
            .map(|_| {
                (0..columns.len())
                    .map(|_| DbValue::decode(&mut bytes))
                    .collect()
            })
            .collect::<io::Result<Vec<_>>>()?;
        if !bytes.is_empty() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(Self {
            handle,
            columns,
            rows,
            truncated,
        })
    }
}

impl Debug for DbRows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if formatting_dump() {
            return f
                .debug_tuple("__CodingPadQuery")
                .field(&self.handle)
                .finish();
        }
        f.debug_struct("DbRows")
            .field("columns", &self.columns)
            .field("rows", &self.rows)
            .field("truncated", &self.truncated)
            .finish()
    }
}

impl DbValue {
    fn decode(input: &mut &[u8]) -> io::Result<Self> {
        let mut tag = [0];
        input.read_exact(&mut tag)?;
        Ok(match tag[0] {
            0 => Self::Null,
            1 => Self::Bool(bool::decode(input)?),
            2 => Self::Integer(i64::decode(input)?),
            3 => Self::Unsigned(u64::decode(input)?),
            4 => Self::Float(f64::decode(input)?),
            5 => Self::Decimal(String::decode(input)?),
            6 => Self::Text(String::decode(input)?),
            _ => return Err(io::ErrorKind::InvalidData.into()),
        })
    }
}
