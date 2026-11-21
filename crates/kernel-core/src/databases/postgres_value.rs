//! Binary result types needed by bound queries, which cannot use simple query.
use chrono::{DateTime, Duration, NaiveDate, Utc};
use postgres_protocol::types as binary;
use serde_json::{json, Value};
use std::{error::Error, fmt::Write};
use tokio_postgres::types::{FromSql, Kind, Type};

type DecodeError = Box<dyn Error + Sync + Send>;

pub(super) struct RawValue<'a>(pub(super) &'a [u8]);

impl<'a> FromSql<'a> for RawValue<'a> {
    fn from_sql(_: &Type, bytes: &'a [u8]) -> Result<Self, DecodeError> {
        Ok(Self(bytes))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

fn take<T, const N: usize>(
    bytes: &mut &[u8],
    decode: impl FnOnce([u8; N]) -> T,
) -> Result<T, DecodeError> {
    let (field, next) = bytes
        .split_first_chunk::<N>()
        .ok_or("incomplete PostgreSQL value")?;
    *bytes = next;
    Ok(decode(*field))
}

fn numeric(mut bytes: &[u8]) -> Result<String, DecodeError> {
    let count = usize::from(take(&mut bytes, u16::from_be_bytes)?);
    let weight = i32::from(take(&mut bytes, i16::from_be_bytes)?);
    let sign = take(&mut bytes, u16::from_be_bytes)?;
    let scale = usize::from(take(&mut bytes, u16::from_be_bytes)?);
    match sign {
        0xc000 => return Ok("NaN".into()),
        0xd000 => return Ok("Infinity".into()),
        0xf000 => return Ok("-Infinity".into()),
        0x0000 | 0x4000 => {}
        _ => return Err("invalid PostgreSQL numeric sign".into()),
    }
    if bytes.len() != count * 2 {
        return Err("invalid PostgreSQL numeric digit count".into());
    }
    // The integer weight and digits are in base 10,000. Scale is in decimal.
    let digit = |power: i32| -> u16 {
        usize::try_from(weight - power)
            .ok()
            .and_then(|index| bytes.get(index * 2..index * 2 + 2))
            .map_or(0, |chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
    };
    let mut value = if sign == 0x4000 {
        "-".into()
    } else {
        String::new()
    };
    let mut powers = (0..=weight.max(0)).rev();
    write!(value, "{}", digit(powers.next().unwrap()))?;
    for power in powers {
        write!(value, "{:04}", digit(power))?;
    }
    if scale > 0 {
        value.push('.');
        let end = value.len() + scale;
        for power in 1..=scale.div_ceil(4) as i32 {
            write!(value, "{:04}", digit(-power))?;
        }
        value.truncate(end);
    }
    Ok(value)
}

fn array(bytes: &[u8], element: &Type) -> Result<Value, DecodeError> {
    use tokio_postgres::fallible_iterator::FallibleIterator;
    let array = binary::array_from_sql(bytes)?;
    let dimensions: Vec<_> = array.dimensions().collect()?;
    if dimensions.is_empty() {
        return Ok(Value::Array(Vec::new()));
    }
    if dimensions.len() > 6 {
        return Err("invalid PostgreSQL array dimension count".into());
    }
    let mut values = array.values();
    let array = array_dimension(&dimensions, &mut values, element)?;
    if values.next()?.is_some() {
        return Err("unexpected trailing PostgreSQL array values".into());
    }
    Ok(array)
}

fn array_dimension(
    dimensions: &[binary::ArrayDimension],
    values: &mut binary::ArrayValues<'_>,
    element: &Type,
) -> Result<Value, DecodeError> {
    use tokio_postgres::fallible_iterator::FallibleIterator;
    let (dimension, inner) = dimensions.split_first().unwrap();
    let items = (0..dimension.len)
        .map(|_| {
            if inner.is_empty() {
                match values.next()?.ok_or("missing PostgreSQL array element")? {
                    Some(bytes) => decode(element, bytes),
                    None => Ok(Value::Null),
                }
            } else {
                array_dimension(inner, values, element)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Value::Array(items))
}

pub(super) fn decode(kind: &Type, bytes: &[u8]) -> Result<Value, DecodeError> {
    if let Kind::Domain(base) = kind.kind() {
        return decode(base, bytes);
    }
    if let Kind::Array(element) = kind.kind() {
        return array(bytes, element);
    }
    if matches!(kind.kind(), Kind::Enum(_)) {
        return Ok(json!(binary::text_from_sql(bytes)?));
    }
    let epoch = NaiveDate::from_ymd_opt(2000, 1, 1).unwrap();
    Ok(match kind.name() {
        "numeric" => json!(numeric(bytes)?),
        "bool" => json!(binary::bool_from_sql(bytes)?),
        "int2" => json!(binary::int2_from_sql(bytes)?),
        "int4" => json!(binary::int4_from_sql(bytes)?),
        "int8" => json!(binary::int8_from_sql(bytes)?),
        "oid" => json!(binary::oid_from_sql(bytes)?),
        "float4" => json!(binary::float4_from_sql(bytes)?),
        "float8" => json!(binary::float8_from_sql(bytes)?),
        "text" | "varchar" | "bpchar" | "name" | "unknown" => json!(binary::text_from_sql(bytes)?),
        "uuid" => json!(uuid::Uuid::from_slice(bytes)?.to_string()),
        "bytea" => json!(super::hex(bytes)),
        "json" => serde_json::from_slice(bytes)?,
        "jsonb" => {
            let (version, payload) = bytes.split_first().ok_or("empty PostgreSQL JSONB value")?;
            if *version != 1 {
                return Err("unsupported PostgreSQL JSONB version".into());
            }
            serde_json::from_slice(payload)?
        }
        "date" => {
            let days = binary::date_from_sql(bytes)?;
            match days {
                i32::MAX => json!("infinity"),
                i32::MIN => json!("-infinity"),
                days => json!(epoch
                    .checked_add_signed(Duration::days(i64::from(days)))
                    .ok_or("date out of range")?
                    .to_string()),
            }
        }
        "timestamp" | "timestamptz" => {
            let micros = binary::timestamp_from_sql(bytes)?;
            match micros {
                i64::MAX => json!("infinity"),
                i64::MIN => json!("-infinity"),
                micros => {
                    let time = epoch
                        .and_hms_opt(0, 0, 0)
                        .unwrap()
                        .checked_add_signed(Duration::microseconds(micros))
                        .ok_or("timestamp out of range")?;
                    if kind.name() == "timestamptz" {
                        json!(DateTime::<Utc>::from_naive_utc_and_offset(time, Utc).to_rfc3339())
                    } else {
                        json!(time.to_string())
                    }
                }
            }
        }
        "interval" => {
            let mut rest = bytes;
            let micros = take(&mut rest, i64::from_be_bytes)?;
            let days = take(&mut rest, i32::from_be_bytes)?;
            let months = take(&mut rest, i32::from_be_bytes)?;
            if !rest.is_empty() {
                return Err("invalid PostgreSQL interval length".into());
            }
            json!(format!("{months} months {days} days {micros} microseconds"))
        }
        _ => return Err("cast this PostgreSQL type to text in the query".into()),
    })
}
