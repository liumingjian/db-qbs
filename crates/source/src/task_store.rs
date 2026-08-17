use std::fs::{self, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::ColumnPrecision;

const DATABASE_FILE: &str = "db-qbs.sqlite3";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Task {
    pub task_id: String,
    pub name: String,
    pub source_sql: String,
    pub source_date_col: String,
    pub target_table: String,
    pub target_date_col: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column_precision: Option<ColumnPrecision>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskInput {
    pub name: String,
    pub source_sql: String,
    pub source_date_col: String,
    pub target_table: String,
    pub target_date_col: String,
    #[serde(default)]
    pub column_precision: Option<ColumnPrecision>,
}

pub struct TaskStore {
    connection: Connection,
}

impl TaskStore {
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(data_dir)
            .map_err(|error| format!("创建 source 数据目录失败：{error}"))?;
        let database_path = data_dir.join(DATABASE_FILE);
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&database_path)
            .map_err(|error| format!("创建 SQLite 库文件失败：{error}"))?;
        fs::set_permissions(&database_path, Permissions::from_mode(0o600))
            .map_err(|error| format!("设置 SQLite 库文件权限失败：{error}"))?;

        let connection = Connection::open(&database_path)
            .map_err(|error| format!("打开 SQLite 库文件失败：{error}"))?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS tasks (
                    task_id         TEXT PRIMARY KEY NOT NULL,
                    name            TEXT NOT NULL,
                    source_sql      TEXT NOT NULL,
                    source_date_col TEXT NOT NULL,
                    target_table    TEXT NOT NULL,
                    target_date_col TEXT NOT NULL,
                    column_precision TEXT
                );",
            )
            .map_err(|error| format!("初始化 SQLite 任务表失败：{error}"))?;
        ensure_column(&connection)?;

        Ok(Self { connection })
    }

    pub fn create(&self, input: TaskInput) -> Result<Task, String> {
        let task = Task {
            task_id: generate_task_id(),
            name: input.name,
            source_sql: input.source_sql,
            source_date_col: input.source_date_col,
            target_table: input.target_table,
            target_date_col: input.target_date_col,
            column_precision: input.column_precision,
        };
        let column_precision = column_precision_json(task.column_precision.as_ref())?;
        self.connection
            .execute(
                "INSERT INTO tasks (
                    task_id, name, source_sql, source_date_col, target_table, target_date_col,
                    column_precision
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    task.task_id,
                    task.name,
                    task.source_sql,
                    task.source_date_col,
                    task.target_table,
                    task.target_date_col,
                    column_precision,
                ],
            )
            .map_err(|error| format!("写入 SQLite 任务失败：{error}"))?;
        Ok(task)
    }

    pub fn list(&self) -> Result<Vec<Task>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT task_id, name, source_sql, source_date_col, target_table, target_date_col,
                        column_precision
                   FROM tasks
               ORDER BY rowid",
            )
            .map_err(|error| format!("准备 SQLite 任务列表查询失败：{error}"))?;
        let tasks = statement
            .query_map([], task_from_row)
            .map_err(|error| format!("查询 SQLite 任务列表失败：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取 SQLite 任务列表失败：{error}"))?;
        Ok(tasks)
    }

    pub fn get(&self, task_id: &str) -> Result<Option<Task>, String> {
        self.connection
            .query_row(
                "SELECT task_id, name, source_sql, source_date_col, target_table, target_date_col,
                        column_precision
                   FROM tasks
                  WHERE task_id = ?1",
                [task_id],
                task_from_row,
            )
            .optional()
            .map_err(|error| format!("查询 SQLite 任务失败：{error}"))
    }

    pub fn update(&self, task_id: &str, input: TaskInput) -> Result<Option<Task>, String> {
        let column_precision = column_precision_json(input.column_precision.as_ref())?;
        let updated_rows = self
            .connection
            .execute(
                "UPDATE tasks
                    SET name = ?2,
                        source_sql = ?3,
                        source_date_col = ?4,
                        target_table = ?5,
                        target_date_col = ?6,
                        column_precision = ?7
                  WHERE task_id = ?1",
                params![
                    task_id,
                    input.name,
                    input.source_sql,
                    input.source_date_col,
                    input.target_table,
                    input.target_date_col,
                    column_precision,
                ],
            )
            .map_err(|error| format!("更新 SQLite 任务失败：{error}"))?;
        if updated_rows == 0 {
            return Ok(None);
        }
        self.get(task_id)
    }

    pub fn delete(&self, task_id: &str) -> Result<Option<Task>, String> {
        let Some(task) = self.get(task_id)? else {
            return Ok(None);
        };
        self.connection
            .execute("DELETE FROM tasks WHERE task_id = ?1", [task_id])
            .map_err(|error| format!("删除 SQLite 任务失败：{error}"))?;
        Ok(Some(task))
    }
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let encoded: Option<String> = row.get("column_precision")?;
    let column_precision = encoded
        .map(|encoded| {
            let column_index = row.as_ref().column_index("column_precision")?;
            serde_json::from_str(&encoded).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    column_index,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()?;
    Ok(Task {
        task_id: row.get(0)?,
        name: row.get(1)?,
        source_sql: row.get(2)?,
        source_date_col: row.get(3)?,
        target_table: row.get(4)?,
        target_date_col: row.get(5)?,
        column_precision,
    })
}

fn ensure_column(connection: &Connection) -> Result<(), String> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'column_precision')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查 SQLite 任务列 column_precision 失败：{error}"))?;
    if exists {
        return Ok(());
    }
    connection
        .execute("ALTER TABLE tasks ADD COLUMN column_precision TEXT", [])
        .map_err(|error| format!("迁移 SQLite 任务列 column_precision 失败：{error}"))?;
    Ok(())
}

fn column_precision_json(value: Option<&ColumnPrecision>) -> Result<Option<String>, String> {
    value
        .map(|value| {
            serde_json::to_string(value)
                .map_err(|error| format!("序列化任务 column_precision 失败：{error}"))
        })
        .transpose()
}

fn generate_task_id() -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut random_bytes = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut random_bytes);
    let mut task_id = String::with_capacity(32);
    for byte in random_bytes {
        task_id.push(HEX[(byte >> 4) as usize] as char);
        task_id.push(HEX[(byte & 0x0f) as usize] as char);
    }
    task_id
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn column_precision_round_trips_as_one_json_column() {
        let directory = temp_directory();
        let mut column_precision = ColumnPrecision::new();
        column_precision.insert("N_AMT".to_owned(), [20, 4]);
        column_precision.insert("N_RATE".to_owned(), [38, -30]);

        let store = TaskStore::open(&directory).unwrap();
        let created = store
            .create(TaskInput {
                name: "holdings".to_owned(),
                source_sql: "SELECT N_AMT, N_RATE FROM HOLDINGS".to_owned(),
                source_date_col: "D_BIZ".to_owned(),
                target_table: "HOLDINGS".to_owned(),
                target_date_col: "D_BIZ".to_owned(),
                column_precision: Some(column_precision.clone()),
            })
            .unwrap();

        assert_eq!(created.column_precision, Some(column_precision.clone()));
        assert_eq!(store.get(&created.task_id).unwrap(), Some(created.clone()));
        assert_eq!(store.list().unwrap(), vec![created.clone()]);

        let connection = Connection::open(directory.join(DATABASE_FILE)).unwrap();
        let encoded: String = connection
            .query_row(
                "SELECT column_precision FROM tasks WHERE task_id = ?1",
                [&created.task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(encoded, r#"{"N_AMT":[20,4],"N_RATE":[38,-30]}"#);

        let cleared = store
            .update(
                &created.task_id,
                TaskInput {
                    name: created.name,
                    source_sql: created.source_sql,
                    source_date_col: created.source_date_col,
                    target_table: created.target_table,
                    target_date_col: created.target_date_col,
                    column_precision: None,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(cleared.column_precision, None);

        drop(connection);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn existing_task_table_is_migrated_without_precision_values() {
        let directory = temp_directory();
        let database = directory.join(DATABASE_FILE);
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE tasks (
                    task_id TEXT PRIMARY KEY NOT NULL,
                    name TEXT NOT NULL,
                    source_sql TEXT NOT NULL,
                    source_date_col TEXT NOT NULL,
                    target_table TEXT NOT NULL,
                    target_date_col TEXT NOT NULL
                );
                INSERT INTO tasks VALUES (
                    'legacy', 'legacy task', 'SELECT 1', 'D_BIZ', 'ORDERS', 'D_BIZ'
                );",
            )
            .unwrap();
        drop(connection);

        let store = TaskStore::open(&directory).unwrap();
        assert_eq!(
            store.get("legacy").unwrap().unwrap().column_precision,
            None
        );

        let connection = Connection::open(database).unwrap();
        let has_column: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'column_precision')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_column);

        drop(connection);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn temp_directory() -> PathBuf {
        let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "db-qbs-source-task-store-test-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }
}
