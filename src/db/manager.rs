use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use mlua::Lua;
use thiserror::Error;

use super::{DatabaseEngine, DatabaseError, DbResult};

#[derive(Debug, Error)]
pub enum DatabaseManagerError {
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error("failed to read migration script `{path}`: {source}")]
    ReadMigration {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("migration Lua error in `{path}`: {source}")]
    MigrationLua {
        path: String,
        #[source]
        source: mlua::Error,
    },
}

#[derive(Debug, Default)]
struct MigrationResults {
    next_id: AtomicU32,
    results: Mutex<HashMap<u32, DbResult>>,
}

impl MigrationResults {
    fn insert(&self, result: DbResult) -> u32 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.results
            .lock()
            .expect("result registry should lock")
            .insert(id, result);
        id
    }

    fn get_number(&self, id: u32, column: &str) -> Option<i64> {
        self.results
            .lock()
            .expect("result registry should lock")
            .get(&id)?
            .get_i64(column)
    }

    fn get_string(&self, id: u32, column: &str) -> Option<String> {
        self.results
            .lock()
            .expect("result registry should lock")
            .get(&id)?
            .get_string(column)
    }

    fn next(&self, id: u32) -> bool {
        self.results
            .lock()
            .expect("result registry should lock")
            .get_mut(&id)
            .map(DbResult::next)
            .unwrap_or(false)
    }

    fn free(&self, id: u32) {
        self.results
            .lock()
            .expect("result registry should lock")
            .remove(&id);
    }
}

pub struct DatabaseManager;

impl DatabaseManager {
    pub async fn optimize_tables<D>(
        database: &D,
        schema_name: &str,
    ) -> Result<bool, DatabaseManagerError>
    where
        D: DatabaseEngine + ?Sized,
    {
        let query = format!(
            "SELECT `TABLE_NAME` FROM `information_schema`.`TABLES` WHERE `TABLE_SCHEMA` = {} AND `DATA_FREE` > 0",
            database.escape_string(schema_name)
        );
        let Some(mut result) = database.store_query(&query).await? else {
            return Ok(false);
        };

        loop {
            if let Some(table_name) = result.get_string("TABLE_NAME") {
                let optimize = format!("OPTIMIZE TABLE `{table_name}`");
                let _ = database.execute(&optimize).await?;
            }

            if !result.next() {
                break;
            }
        }

        Ok(true)
    }

    pub async fn table_exists<D>(
        database: &D,
        schema_name: &str,
        table_name: &str,
    ) -> Result<bool, DatabaseManagerError>
    where
        D: DatabaseEngine + ?Sized,
    {
        let query = format!(
            "SELECT `TABLE_NAME` FROM `information_schema`.`tables` WHERE `TABLE_SCHEMA` = {} AND `TABLE_NAME` = {} LIMIT 1",
            database.escape_string(schema_name),
            database.escape_string(table_name)
        );

        Ok(database.store_query(&query).await?.is_some())
    }

    pub async fn is_database_setup<D>(
        database: &D,
        schema_name: &str,
    ) -> Result<bool, DatabaseManagerError>
    where
        D: DatabaseEngine + ?Sized,
    {
        let query = format!(
            "SELECT `TABLE_NAME` FROM `information_schema`.`tables` WHERE `TABLE_SCHEMA` = {}",
            database.escape_string(schema_name)
        );
        Ok(database.store_query(&query).await?.is_some())
    }

    pub async fn get_database_version<D>(
        database: &D,
        schema_name: &str,
    ) -> Result<i32, DatabaseManagerError>
    where
        D: DatabaseEngine + ?Sized,
    {
        if !Self::table_exists(database, schema_name, "server_config").await? {
            database
                .execute(
                    "CREATE TABLE `server_config` (`config` VARCHAR(50) NOT NULL, `value` VARCHAR(256) NOT NULL DEFAULT '', UNIQUE(`config`)) ENGINE = InnoDB",
                )
                .await?;
            database
                .execute("INSERT INTO `server_config` VALUES ('db_version', 0)")
                .await?;
            return Ok(0);
        }

        if let Some(value) = Self::get_database_config(database, "db_version").await? {
            Ok(value)
        } else {
            Ok(-1)
        }
    }

    pub async fn get_database_config<D>(
        database: &D,
        config: &str,
    ) -> Result<Option<i32>, DatabaseManagerError>
    where
        D: DatabaseEngine + ?Sized,
    {
        let query = format!(
            "SELECT `value` FROM `server_config` WHERE `config` = {}",
            database.escape_string(config)
        );
        let Some(result) = database.store_query(&query).await? else {
            return Ok(None);
        };

        Ok(result.get_i64("value").map(|value| value as i32))
    }

    pub async fn register_database_config<D>(
        database: &D,
        config: &str,
        value: i32,
    ) -> Result<(), DatabaseManagerError>
    where
        D: DatabaseEngine + ?Sized,
    {
        if Self::get_database_config(database, config).await?.is_none() {
            let query = format!(
                "INSERT INTO `server_config` VALUES ({}, '{}')",
                database.escape_string(config),
                value
            );
            database.execute(&query).await?;
        } else {
            let query = format!(
                "UPDATE `server_config` SET `value` = '{}' WHERE `config` = {}",
                value,
                database.escape_string(config)
            );
            database.execute(&query).await?;
        }

        Ok(())
    }

    pub async fn update_database<D>(
        database: Arc<D>,
        migrations_dir: &Path,
        schema_name: &str,
    ) -> Result<i32, DatabaseManagerError>
    where
        D: DatabaseEngine + 'static,
    {
        let result_registry = Arc::new(MigrationResults::default());
        let lua = Lua::new();
        let db_table = lua
            .create_table()
            .map_err(|source| DatabaseManagerError::MigrationLua {
                path: String::from("<db-table>"),
                source,
            })?;

        let db_for_query = database.clone();
        db_table
            .set(
                "query",
                lua.create_async_function(move |_lua, query: String| {
                    let database = db_for_query.clone();
                    async move {
                        database
                            .execute(&query)
                            .await
                            .map_err(mlua::Error::external)
                    }
                })
                .map_err(|source| DatabaseManagerError::MigrationLua {
                    path: String::from("<db-table>"),
                    source,
                })?,
            )
            .map_err(|source| DatabaseManagerError::MigrationLua {
                path: String::from("<db-table>"),
                source,
            })?;

        let db_for_store = database.clone();
        let results_for_store = result_registry.clone();
        db_table
            .set(
                "storeQuery",
                lua.create_async_function(move |_lua, query: String| {
                    let database = db_for_store.clone();
                    let results = results_for_store.clone();
                    async move {
                        let result = database
                            .store_query(&query)
                            .await
                            .map_err(mlua::Error::external)?;
                        Ok(result.map(|result| results.insert(result)))
                    }
                })
                .map_err(|source| DatabaseManagerError::MigrationLua {
                    path: String::from("<db-table>"),
                    source,
                })?,
            )
            .map_err(|source| DatabaseManagerError::MigrationLua {
                path: String::from("<db-table>"),
                source,
            })?;

        let db_for_escape = database.clone();
        db_table
            .set(
                "escapeString",
                lua.create_function(move |_lua, value: String| {
                    Ok(db_for_escape.escape_string(&value))
                })
                .map_err(|source| DatabaseManagerError::MigrationLua {
                    path: String::from("<db-table>"),
                    source,
                })?,
            )
            .map_err(|source| DatabaseManagerError::MigrationLua {
                path: String::from("<db-table>"),
                source,
            })?;

        lua.globals()
            .set("db", db_table)
            .map_err(|source| DatabaseManagerError::MigrationLua {
                path: String::from("<globals>"),
                source,
            })?;

        let result_table =
            lua.create_table()
                .map_err(|source| DatabaseManagerError::MigrationLua {
                    path: String::from("<result-table>"),
                    source,
                })?;

        let results_for_number = result_registry.clone();
        result_table
            .set(
                "getNumber",
                lua.create_function(move |_lua, (id, column): (u32, String)| {
                    Ok(results_for_number.get_number(id, &column).unwrap_or(0))
                })
                .map_err(|source| DatabaseManagerError::MigrationLua {
                    path: String::from("<result-table>"),
                    source,
                })?,
            )
            .map_err(|source| DatabaseManagerError::MigrationLua {
                path: String::from("<result-table>"),
                source,
            })?;

        let results_for_string = result_registry.clone();
        result_table
            .set(
                "getString",
                lua.create_function(move |_lua, (id, column): (u32, String)| {
                    Ok(results_for_string
                        .get_string(id, &column)
                        .unwrap_or_default())
                })
                .map_err(|source| DatabaseManagerError::MigrationLua {
                    path: String::from("<result-table>"),
                    source,
                })?,
            )
            .map_err(|source| DatabaseManagerError::MigrationLua {
                path: String::from("<result-table>"),
                source,
            })?;

        let results_for_next = result_registry.clone();
        result_table
            .set(
                "next",
                lua.create_function(move |_lua, id: u32| Ok(results_for_next.next(id)))
                    .map_err(|source| DatabaseManagerError::MigrationLua {
                        path: String::from("<result-table>"),
                        source,
                    })?,
            )
            .map_err(|source| DatabaseManagerError::MigrationLua {
                path: String::from("<result-table>"),
                source,
            })?;

        let results_for_free = result_registry.clone();
        result_table
            .set(
                "free",
                lua.create_function(move |_lua, id: u32| {
                    results_for_free.free(id);
                    Ok(())
                })
                .map_err(|source| DatabaseManagerError::MigrationLua {
                    path: String::from("<result-table>"),
                    source,
                })?,
            )
            .map_err(|source| DatabaseManagerError::MigrationLua {
                path: String::from("<result-table>"),
                source,
            })?;

        lua.globals()
            .set("result", result_table)
            .map_err(|source| DatabaseManagerError::MigrationLua {
                path: String::from("<globals>"),
                source,
            })?;

        let mut version = Self::get_database_version(database.as_ref(), schema_name).await?;

        loop {
            let path = migrations_dir.join(format!("{version}.lua"));
            if !tokio::fs::try_exists(&path).await.map_err(|source| {
                DatabaseManagerError::ReadMigration {
                    path: path.display().to_string(),
                    source,
                }
            })? {
                break;
            }

            let script = tokio::fs::read_to_string(&path).await.map_err(|source| {
                DatabaseManagerError::ReadMigration {
                    path: path.display().to_string(),
                    source,
                }
            })?;

            lua.load(&script)
                .set_name(path.to_string_lossy().as_ref())
                .exec_async()
                .await
                .map_err(|source| DatabaseManagerError::MigrationLua {
                    path: path.display().to_string(),
                    source,
                })?;

            let update = lua
                .globals()
                .get::<mlua::Function>("onUpdateDatabase")
                .map_err(|source| DatabaseManagerError::MigrationLua {
                    path: path.display().to_string(),
                    source,
                })?;

            let applied = update.call_async::<bool>(()).await.map_err(|source| {
                DatabaseManagerError::MigrationLua {
                    path: path.display().to_string(),
                    source,
                }
            })?;

            if !applied {
                break;
            }

            version += 1;
            Self::register_database_config(database.as_ref(), "db_version", version).await?;
        }

        Ok(version)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use super::super::{DatabaseEngine, DatabaseError, DbResult, DbValue};
    use super::DatabaseManager;

    #[derive(Default)]
    struct MockDatabase {
        executed: Mutex<Vec<String>>,
        store_results: Mutex<VecDeque<Option<DbResult>>>,
    }

    impl MockDatabase {
        fn push_result(&self, result: Option<DbResult>) {
            self.store_results
                .lock()
                .expect("mock results should lock")
                .push_back(result);
        }
    }

    impl DatabaseEngine for MockDatabase {
        fn execute<'a>(
            &'a self,
            query: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<bool, DatabaseError>> + Send + 'a>> {
            Box::pin(async move {
                self.executed
                    .lock()
                    .expect("executed queries should lock")
                    .push(query.to_owned());
                Ok(true)
            })
        }

        fn store_query<'a>(
            &'a self,
            _query: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<DbResult>, DatabaseError>> + Send + 'a>>
        {
            Box::pin(async move {
                Ok(self
                    .store_results
                    .lock()
                    .expect("mock results should lock")
                    .pop_front()
                    .unwrap_or(None))
            })
        }

        fn escape_string(&self, value: &str) -> String {
            format!("'{value}'")
        }

        fn escape_blob(&self, value: &[u8]) -> String {
            format!("'{}'", String::from_utf8_lossy(value))
        }

        fn max_packet_size(&self) -> u64 {
            1_048_576
        }
    }

    #[tokio::test]
    async fn get_database_version_should_bootstrap_server_config_when_missing() {
        let database = MockDatabase::default();
        database.push_result(None);

        let version = DatabaseManager::get_database_version(&database, "forgottenserver")
            .await
            .expect("database version lookup should succeed");

        assert_eq!(version, 0);
        assert_eq!(
            database
                .executed
                .lock()
                .expect("executed queries should lock")
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn update_database_should_run_lua_migrations_and_bump_version() {
        let database = Arc::new(MockDatabase::default());
        database.push_result(Some(DbResult {
            rows: vec![BTreeMap::from([(
                String::from("TABLE_NAME"),
                DbValue::Bytes(b"server_config".to_vec()),
            )])],
            index: 0,
        }));
        database.push_result(Some(DbResult {
            rows: vec![BTreeMap::from([(String::from("value"), DbValue::Int(0))])],
            index: 0,
        }));
        database.push_result(None);

        let dir = std::env::temp_dir().join("tfs-rust-migrations");
        std::fs::create_dir_all(&dir).expect("migration dir should be creatable");
        std::fs::write(
            dir.join("0.lua"),
            r#"
function onUpdateDatabase()
  db.query("ALTER TABLE `accounts` ADD `name` VARCHAR(32) NOT NULL")
  return true
end
"#,
        )
        .expect("migration file should be writable");

        let version = DatabaseManager::update_database(database.clone(), &dir, "forgottenserver")
            .await
            .expect("migration should succeed");

        assert_eq!(version, 1);
        assert!(database
            .executed
            .lock()
            .expect("executed queries should lock")
            .iter()
            .any(|query| query.contains("ALTER TABLE `accounts` ADD `name`")));

        std::fs::remove_file(dir.join("0.lua")).expect("migration file should be removable");
        std::fs::remove_dir(dir).expect("migration dir should be removable");
    }
}
