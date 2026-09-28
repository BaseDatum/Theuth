//! A SQLite storage-class value as `Change.value` carries it: one tag byte
//! (`i` integer, `r` real, `t` text, `b` blob) and the payload. `None` in the
//! change is SQL NULL. Types survive the round trip so an INTEGER column stays
//! an integer on the receiving engine.

use sqlx::query::Query;
use sqlx::sqlite::{Sqlite, SqliteArguments};
use sqlx::{Row as _, sqlite::SqliteRow};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Val {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Val {
    /// Read the `value` column of a `_replica_log` row by its `typeof()` tag.
    pub(crate) fn from_log(row: &SqliteRow, vtype: &str) -> anyhow::Result<Self> {
        Ok(match vtype {
            "null" => Self::Null,
            "integer" => Self::Int(row.try_get("value")?),
            "real" => Self::Real(row.try_get("value")?),
            "text" => Self::Text(row.try_get("value")?),
            "blob" => Self::Blob(row.try_get("value")?),
            other => anyhow::bail!("unknown sqlite storage class {other:?}"),
        })
    }

    /// Read column `idx` of any row by the storage class the value actually has.
    pub(crate) fn from_row(row: &SqliteRow, idx: usize) -> anyhow::Result<Self> {
        use sqlx::{TypeInfo as _, ValueRef as _};
        let raw = row.try_get_raw(idx)?;
        if raw.is_null() {
            return Ok(Self::Null);
        }
        Ok(match raw.type_info().name() {
            "INTEGER" => Self::Int(row.try_get(idx)?),
            "REAL" => Self::Real(row.try_get(idx)?),
            "TEXT" => Self::Text(row.try_get(idx)?),
            "BLOB" => Self::Blob(row.try_get(idx)?),
            other => anyhow::bail!("unexpected sqlite storage class {other:?}"),
        })
    }

    pub(crate) fn encode(&self) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        match self {
            Self::Null => return None,
            Self::Int(i) => {
                out.push(b'i');
                out.extend_from_slice(&i.to_be_bytes());
            }
            Self::Real(r) => {
                out.push(b'r');
                out.extend_from_slice(&r.to_be_bytes());
            }
            Self::Text(t) => {
                out.push(b't');
                out.extend_from_slice(t.as_bytes());
            }
            Self::Blob(b) => {
                out.push(b'b');
                out.extend_from_slice(b);
            }
        }
        Some(out)
    }

    pub(crate) fn decode(bytes: Option<&[u8]>) -> anyhow::Result<Self> {
        let Some(bytes) = bytes else {
            return Ok(Self::Null);
        };
        let (tag, body) = bytes
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("empty value"))?;
        Ok(match tag {
            b'i' => Self::Int(i64::from_be_bytes(body.try_into()?)),
            b'r' => Self::Real(f64::from_be_bytes(body.try_into()?)),
            b't' => Self::Text(std::str::from_utf8(body)?.to_owned()),
            b'b' => Self::Blob(body.to_vec()),
            _ => anyhow::bail!("unknown value tag {tag:#x}"),
        })
    }

    /// Bind as the next positional parameter.
    pub(crate) fn bind<'q>(
        self,
        q: Query<'q, Sqlite, SqliteArguments>,
    ) -> Query<'q, Sqlite, SqliteArguments> {
        match self {
            Self::Null => q.bind(None::<i64>),
            Self::Int(i) => q.bind(i),
            Self::Real(r) => q.bind(r),
            Self::Text(t) => q.bind(t),
            Self::Blob(b) => q.bind(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Val;

    #[test]
    fn round_trips_every_storage_class() {
        for v in [
            Val::Null,
            Val::Int(-42),
            Val::Real(1.5),
            Val::Text("héllo".into()),
            Val::Blob(vec![0, 255, 1]),
        ] {
            assert_eq!(Val::decode(v.encode().as_deref()).unwrap(), v);
        }
    }
}
