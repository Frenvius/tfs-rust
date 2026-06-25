use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use mysql_async::prelude::Queryable;
use mysql_async::{OptsBuilder, Pool, Row, Value};
use thiserror::Error;

use crate::config::{ConfigManager, IntegerConfig, StringConfig};

pub mod ban;
pub mod guild;
pub mod login;
pub mod manager;
pub mod tasks;

static G_DATABASE: OnceLock<Database> = OnceLock::new();

pub fn g_database() -> &'static Database {
    G_DATABASE.get().expect("database not initialized")
}

pub(crate) fn init_database(db: Database) {
    G_DATABASE
        .set(db)
        .unwrap_or_else(|_| panic!("database already initialized"));
}

pub type DbFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, DatabaseError>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq)]
pub enum DbValue {
    Null,
    Bytes(Vec<u8>),
    Int(i64),
    UInt(u64),
    Float(f32),
    Double(f64),
    Date(String),
    Time(String),
}

impl DbValue {
    fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            Self::UInt(value) => i64::try_from(*value).ok(),
            Self::Bytes(bytes) => std::str::from_utf8(bytes).ok()?.parse().ok(),
            _ => None,
        }
    }

    fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Int(value) if *value >= 0 => Some(*value as u64),
            Self::UInt(value) => Some(*value),
            Self::Bytes(bytes) => std::str::from_utf8(bytes).ok()?.parse().ok(),
            _ => None,
        }
    }

    fn as_string(&self) -> Option<String> {
        match self {
            Self::Null => None,
            Self::Bytes(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
            Self::Int(value) => Some(value.to_string()),
            Self::UInt(value) => Some(value.to_string()),
            Self::Float(value) => Some(value.to_string()),
            Self::Double(value) => Some(value.to_string()),
            Self::Date(value) | Self::Time(value) => Some(value.clone()),
        }
    }

    fn as_bytes(&self) -> Option<Vec<u8>> {
        match self {
            Self::Bytes(bytes) => Some(bytes.clone()),
            _ => None,
        }
    }
}

impl From<Value> for DbValue {
    fn from(value: Value) -> Self {
        match value {
            Value::NULL => Self::Null,
            Value::Bytes(bytes) => Self::Bytes(bytes),
            Value::Int(value) => Self::Int(value),
            Value::UInt(value) => Self::UInt(value),
            Value::Float(value) => Self::Float(value),
            Value::Double(value) => Self::Double(value),
            Value::Date(year, month, day, hour, minute, second, micros) => Self::Date(format!(
                "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{:06}",
                micros
            )),
            Value::Time(negative, days, hours, minutes, seconds, micros) => Self::Time(format!(
                "{}{} {:02}:{:02}:{:02}.{:06}",
                if negative { "-" } else { "" },
                days,
                hours,
                minutes,
                seconds,
                micros
            )),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DbResult {
    rows: Vec<BTreeMap<String, DbValue>>,
    index: usize,
}

impl DbResult {
    pub fn from_rows(rows: Vec<Row>) -> Option<Self> {
        if rows.is_empty() {
            return None;
        }

        let mut result_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let columns = row
                .columns_ref()
                .iter()
                .map(|column| column.name_str().to_string())
                .collect::<Vec<_>>();
            let values = row.unwrap();
            let mapped = columns
                .into_iter()
                .zip(values.into_iter().map(DbValue::from))
                .collect::<BTreeMap<_, _>>();
            result_rows.push(mapped);
        }

        Some(Self {
            rows: result_rows,
            index: 0,
        })
    }

    pub fn get_i64(&self, column: &str) -> Option<i64> {
        self.rows.get(self.index)?.get(column)?.as_i64()
    }

    pub fn get_u64(&self, column: &str) -> Option<u64> {
        self.rows.get(self.index)?.get(column)?.as_u64()
    }

    pub fn get_string(&self, column: &str) -> Option<String> {
        self.rows.get(self.index)?.get(column)?.as_string()
    }

    pub fn get_bytes(&self, column: &str) -> Option<Vec<u8>> {
        self.rows.get(self.index)?.get(column)?.as_bytes()
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> bool {
        if self.index + 1 >= self.rows.len() {
            return false;
        }

        self.index += 1;
        true
    }
}

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("database connection error: {0}")]
    Mysql(#[from] mysql_async::Error),
    #[error("database result is missing expected column `{0}`")]
    MissingColumn(String),
    #[error("database is not connected")]
    NotConnected,
}

pub trait DatabaseEngine: Send + Sync {
    fn execute<'a>(&'a self, query: &'a str) -> DbFuture<'a, bool>;
    fn store_query<'a>(&'a self, query: &'a str) -> DbFuture<'a, Option<DbResult>>;
    fn escape_string(&self, value: &str) -> String;
    fn escape_blob(&self, value: &[u8]) -> String;
    fn max_packet_size(&self) -> u64;
}

#[derive(Debug)]
pub struct Database {
    pool: Pool,
    max_packet_size: AtomicU64,
}

impl Database {
    pub async fn connect(config: &ConfigManager) -> Result<Self, DatabaseError> {
        let mut builder = OptsBuilder::default()
            .ip_or_hostname(config.get_string(StringConfig::MysqlHost).to_owned())
            .user(Some(config.get_string(StringConfig::MysqlUser).to_owned()))
            .pass(Some(config.get_string(StringConfig::MysqlPass).to_owned()))
            .db_name(Some(config.get_string(StringConfig::MysqlDb).to_owned()))
            .tcp_port(config.get_number(IntegerConfig::SqlPort) as u16);

        let socket = config.get_string(StringConfig::MysqlSock);
        if !socket.is_empty() {
            builder = builder.socket(Some(socket.to_owned()));
        }

        let pool = Pool::new(builder);
        let mut conn = pool.get_conn().await?;
        let rows: Vec<Row> = conn
            .query("SHOW VARIABLES LIKE 'max_allowed_packet'")
            .await?;
        drop(conn);

        let max_packet_size = DbResult::from_rows(rows)
            .and_then(|result| result.get_u64("Value"))
            .unwrap_or(1_048_576);

        Ok(Self {
            pool,
            max_packet_size: AtomicU64::new(max_packet_size),
        })
    }
}

impl DatabaseEngine for Database {
    fn execute<'a>(&'a self, query: &'a str) -> DbFuture<'a, bool> {
        Box::pin(async move {
            let mut conn = self.pool.get_conn().await?;
            conn.query_drop(query).await?;
            Ok(true)
        })
    }

    fn store_query<'a>(&'a self, query: &'a str) -> DbFuture<'a, Option<DbResult>> {
        Box::pin(async move {
            let mut conn = self.pool.get_conn().await?;
            let rows: Vec<Row> = conn.query(query).await?;
            Ok(DbResult::from_rows(rows))
        })
    }

    fn escape_string(&self, value: &str) -> String {
        self.escape_blob(value.as_bytes())
    }

    fn escape_blob(&self, value: &[u8]) -> String {
        let mut escaped = String::with_capacity((value.len() * 2) + 2);
        escaped.push('\'');

        for byte in value {
            match *byte {
                0 => escaped.push_str("\\0"),
                b'\n' => escaped.push_str("\\n"),
                b'\r' => escaped.push_str("\\r"),
                b'\\' => escaped.push_str("\\\\"),
                b'\'' => escaped.push_str("\\'"),
                b'"' => escaped.push_str("\\\""),
                0x1A => escaped.push_str("\\Z"),
                other => escaped.push(other as char),
            }
        }

        escaped.push('\'');
        escaped
    }

    fn max_packet_size(&self) -> u64 {
        self.max_packet_size.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::DatabaseEngine;
    use super::{Database, DbResult, DbValue};

    #[test]
    fn escape_string_should_quote_and_escape_mysql_special_bytes() {
        let database = Database {
            pool: mysql_async::Pool::new("mysql://root:root@127.0.0.1/test"),
            max_packet_size: std::sync::atomic::AtomicU64::new(1_048_576),
        };

        assert_eq!(database.escape_string("O'Reilly\n"), "'O\\'Reilly\\n'");
    }

    #[test]
    fn db_result_should_read_values_from_the_current_row() {
        let result = DbResult {
            rows: vec![BTreeMap::from([
                (String::from("Value"), DbValue::UInt(123)),
                (String::from("Name"), DbValue::Bytes(b"alice".to_vec())),
            ])],
            index: 0,
        };

        assert_eq!(result.get_u64("Value"), Some(123));
        assert_eq!(result.get_string("Name").as_deref(), Some("alice"));
    }
}
