use std::fs::{self, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const DATABASE_FILE: &str = "db-qbs.sqlite3";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Task {
    pub task_id: String,
    pub name: String,
    pub source_sql: String,
    pub source_date_col: String,
    pub target_table: String,
    pub target_date_col: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskInput {
    pub name: String,
    pub source_sql: String,
    pub source_date_col: String,
    pub target_table: String,
    pub target_date_col: String,
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
                    target_date_col TEXT NOT NULL
                );",
            )
            .map_err(|error| format!("初始化 SQLite 任务表失败：{error}"))?;

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
        };
        self.connection
            .execute(
                "INSERT INTO tasks (
                    task_id, name, source_sql, source_date_col, target_table, target_date_col
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    task.task_id,
                    task.name,
                    task.source_sql,
                    task.source_date_col,
                    task.target_table,
                    task.target_date_col,
                ],
            )
            .map_err(|error| format!("写入 SQLite 任务失败：{error}"))?;
        Ok(task)
    }

    pub fn list(&self) -> Result<Vec<Task>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT task_id, name, source_sql, source_date_col, target_table, target_date_col
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
                "SELECT task_id, name, source_sql, source_date_col, target_table, target_date_col
                   FROM tasks
                  WHERE task_id = ?1",
                [task_id],
                task_from_row,
            )
            .optional()
            .map_err(|error| format!("查询 SQLite 任务失败：{error}"))
    }

    pub fn update(&self, task_id: &str, input: TaskInput) -> Result<Option<Task>, String> {
        let updated_rows = self
            .connection
            .execute(
                "UPDATE tasks
                    SET name = ?2,
                        source_sql = ?3,
                        source_date_col = ?4,
                        target_table = ?5,
                        target_date_col = ?6
                  WHERE task_id = ?1",
                params![
                    task_id,
                    input.name,
                    input.source_sql,
                    input.source_date_col,
                    input.target_table,
                    input.target_date_col,
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
    Ok(Task {
        task_id: row.get(0)?,
        name: row.get(1)?,
        source_sql: row.get(2)?,
        source_date_col: row.get(3)?,
        target_table: row.get(4)?,
        target_date_col: row.get(5)?,
    })
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
