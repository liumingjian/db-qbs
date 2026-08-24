use std::fs::{self, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::TaskSpec;

const DATABASE_FILE: &str = "db-qbs.sqlite3";

/// 一条任务定义：**规格**（搬什么）+ **绑定**（从哪搬到哪）。
///
/// SQL 不存（ADR-0036 §2），现算。两个数据源 id 是**绑定，不是规格**（ADR-0037 §8）——
/// 它们一个都不参与规格那三个派生面，所以不进 [`TaskSpec`]；
/// 好处是同一份规格可以换绑定指到测试库或生产库，而不必改规格本身。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Task {
    pub task_id: String,
    pub name: String,
    pub source_datasource_id: String,
    pub target_datasource_id: String,
    pub spec: TaskSpec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskInput {
    pub name: String,
    pub source_datasource_id: String,
    pub target_datasource_id: String,
    pub spec: TaskSpec,
}

impl TaskInput {
    fn validate(&self) -> Result<(), String> {
        if self.source_datasource_id.trim().is_empty() {
            return Err("必须选一个源端数据源".to_owned());
        }
        if self.target_datasource_id.trim().is_empty() {
            return Err("必须选一个目标端数据源".to_owned());
        }
        self.spec.validate()
    }
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
        drop_incompatible_task_table(&connection)?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS tasks (
                    task_id              TEXT PRIMARY KEY NOT NULL,
                    name                 TEXT NOT NULL,
                    source_datasource_id TEXT NOT NULL,
                    target_datasource_id TEXT NOT NULL,
                    spec                 TEXT NOT NULL
                );",
            )
            .map_err(|error| format!("初始化 SQLite 任务表失败：{error}"))?;

        Ok(Self { connection })
    }

    pub fn create(&self, input: TaskInput) -> Result<Task, String> {
        input.validate()?;
        let task = Task {
            task_id: generate_task_id(),
            name: input.name,
            source_datasource_id: input.source_datasource_id,
            target_datasource_id: input.target_datasource_id,
            spec: input.spec,
        };
        self.connection
            .execute(
                "INSERT INTO tasks (task_id, name, source_datasource_id, target_datasource_id, spec)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    task.task_id,
                    task.name,
                    task.source_datasource_id,
                    task.target_datasource_id,
                    spec_json(&task.spec)?
                ],
            )
            .map_err(|error| format!("写入 SQLite 任务失败：{error}"))?;
        Ok(task)
    }

    pub fn list(&self) -> Result<Vec<Task>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT task_id, name, source_datasource_id, target_datasource_id, spec
                   FROM tasks ORDER BY rowid",
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
                "SELECT task_id, name, source_datasource_id, target_datasource_id, spec
                   FROM tasks WHERE task_id = ?1",
                [task_id],
                task_from_row,
            )
            .optional()
            .map_err(|error| format!("查询 SQLite 任务失败：{error}"))
    }

    pub fn update(&self, task_id: &str, input: TaskInput) -> Result<Option<Task>, String> {
        input.validate()?;
        let updated_rows = self
            .connection
            .execute(
                "UPDATE tasks
                    SET name = ?2, source_datasource_id = ?3, target_datasource_id = ?4, spec = ?5
                  WHERE task_id = ?1",
                params![
                    task_id,
                    input.name,
                    input.source_datasource_id,
                    input.target_datasource_id,
                    spec_json(&input.spec)?
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

/// 删数据源前的引用检查（ADR-0037 §7）：还有任务引着就拒绝，不做级联、不做软删。
/// 悬空引用会把失败推迟到发起运行那一刻才炸，那时用户手上只有一条「连不上」。
impl TaskStore {
    pub fn names_referencing(&self, datasource_id: &str) -> Result<Vec<String>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT name FROM tasks
                  WHERE source_datasource_id = ?1 OR target_datasource_id = ?1
                  ORDER BY rowid",
            )
            .map_err(|error| format!("准备 SQLite 数据源引用查询失败：{error}"))?;
        let names = statement
            .query_map([datasource_id], |row| row.get(0))
            .map_err(|error| format!("查询 SQLite 数据源引用失败：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取 SQLite 数据源引用失败：{error}"))?;
        Ok(names)
    }
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let encoded: String = row.get("spec")?;
    let spec = serde_json::from_str(&encoded).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            row.as_ref()
                .column_index("spec")
                .expect("the spec column was just read"),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })?;
    Ok(Task {
        task_id: row.get("task_id")?,
        name: row.get("name")?,
        source_datasource_id: row.get("source_datasource_id")?,
        target_datasource_id: row.get("target_datasource_id")?,
        spec,
    })
}

/// 形态对不上的任务表整表丢弃、换新数据结构——ADR-0036 §4 的原判，
/// 前提是第一版尚无真实用户数据。**不做就地翻译**（反解析任意 Oracle SQL 正是 ADR-0023 §2
/// 否掉的那件事），**不做 legacy 并存**（会把「两种任务形态」永久焊进发起链路）。
///
/// 判据三条，缺任一即丢：
/// - 没有 `spec` 列 —— 旧的四字段形态（ADR-0016 §2）。
/// - 没有 `source_datasource_id` 列 —— 数据源绑定之前的形态。**旧行没有可推导的取值**：
///   目标端数据源的凭据在对端的 `sink.toml` 里，source 拿不到，任何自动填都是编造
///   （ADR-0037 §7）。
/// - `spec` 列里有一行反序列化不出当前的 [`TaskSpec`] —— ADR-0038 §2 把 `columns` 从
///   `Vec<String>` 换成了 `Vec<ColumnMapping>`，表的列名没变、JSON 的形状变了，所以前两条查不出它。
///   **这不是兼容层**：不翻译、不加 serde 容错，与前两条走同一条「整表丢弃」的路
///   （ADR-0036 §4）。不加这一条的话旧行会让 `list()` 直接报错——那既不是「丢弃」，
///   也没有从界面上恢复的办法。
fn drop_incompatible_task_table(connection: &Connection) -> Result<(), String> {
    let table_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'tasks')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查 SQLite 任务表失败：{error}"))?;
    if !table_exists {
        return Ok(());
    }
    let compatible: bool = connection
        .query_row(
            "SELECT
               EXISTS(SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'spec')
               AND
               EXISTS(SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'source_datasource_id')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查 SQLite 任务表形态失败：{error}"))?;
    if compatible && every_spec_parses(connection)? {
        return Ok(());
    }
    connection
        .execute("DROP TABLE tasks", [])
        .map_err(|error| format!("丢弃旧 SQLite 任务表失败：{error}"))?;
    Ok(())
}

/// 每一行的 `spec` 都还能反序列化成当前的 [`TaskSpec`]？一行不行就等于整表形态对不上。
fn every_spec_parses(connection: &Connection) -> Result<bool, String> {
    let mut statement = connection
        .prepare("SELECT spec FROM tasks")
        .map_err(|error| format!("准备 SQLite 任务规格形态查询失败：{error}"))?;
    let mut rows = statement
        .query([])
        .map_err(|error| format!("查询 SQLite 任务规格形态失败：{error}"))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("读取 SQLite 任务规格形态失败：{error}"))?
    {
        let encoded: String = row
            .get(0)
            .map_err(|error| format!("读取 SQLite 任务规格失败：{error}"))?;
        if serde_json::from_str::<TaskSpec>(&encoded).is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn spec_json(spec: &TaskSpec) -> Result<String, String> {
    serde_json::to_string(spec).map_err(|error| format!("序列化任务规格失败：{error}"))
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
    use crate::task_spec::{ColumnMapping, Comparison, Condition, ValueSource, ValueType};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn mapping(column: &str) -> ColumnMapping {
        ColumnMapping {
            source: column.to_owned(),
            target: column.to_owned(),
        }
    }

    fn sample_spec() -> TaskSpec {
        TaskSpec {
            source_sql: None,
            dblink: Some("FA".to_owned()),
            owner: "HTBR45".to_owned(),
            table: "T_R_FR_ASTSTAT".to_owned(),
            columns: vec![mapping("ID"), mapping("D_BIZ")],
            primary_key: vec!["ID".to_owned()],
            conditions: vec![Condition {
                column: "D_BIZ".to_owned(),
                operator: Comparison::Eq,
                value_type: ValueType::Date,
                parameter: "biz_date".to_owned(),
                value_source: ValueSource::Runtime,
                constant: String::new(),
            }],
            order_by: Vec::new(),
            target_table: "T_POSITION".to_owned(),
        }
    }

    #[test]
    fn spec_round_trips_through_one_json_column() {
        let directory = temp_directory();
        let store = TaskStore::open(&directory).unwrap();
        let created = store
            .create(TaskInput {
                name: "holdings".to_owned(),
                source_datasource_id: "src1".to_owned(),
                target_datasource_id: "tgt1".to_owned(),
                spec: sample_spec(),
            })
            .unwrap();

        assert_eq!(created.spec, sample_spec());
        assert_eq!(store.get(&created.task_id).unwrap(), Some(created.clone()));
        assert_eq!(store.list().unwrap(), vec![created]);

        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_invalid_spec_is_refused_before_it_reaches_sqlite() {
        let directory = temp_directory();
        let store = TaskStore::open(&directory).unwrap();
        let mut spec = sample_spec();
        spec.primary_key = vec!["MISSING".to_owned()];

        let error = store
            .create(TaskInput {
                name: "holdings".to_owned(),
                source_datasource_id: "src1".to_owned(),
                target_datasource_id: "tgt1".to_owned(),
                spec,
            })
            .unwrap_err();
        assert_eq!(error, "主键列 MISSING 不在选中的列里");
        assert!(store.list().unwrap().is_empty());

        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_legacy_four_field_task_table_is_dropped_whole() {
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
        assert!(store.list().unwrap().is_empty());
        assert_eq!(store.get("legacy").unwrap(), None);

        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_task_row_whose_spec_predates_column_mapping_is_dropped_whole() {
        // ADR-0038 §2 换了 `columns` 的形状，表的列名一个没变——所以按列名查形态查不出它。
        // 判据是「spec 反序列化不出来」，处置与前两条一样：整表丢弃（ADR-0036 §4）。
        let directory = temp_directory();
        let database = directory.join(DATABASE_FILE);
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE tasks (
                    task_id              TEXT PRIMARY KEY NOT NULL,
                    name                 TEXT NOT NULL,
                    source_datasource_id TEXT NOT NULL,
                    target_datasource_id TEXT NOT NULL,
                    spec                 TEXT NOT NULL
                );
                INSERT INTO tasks VALUES (
                    'stale', 'stale task', 'src1', 'tgt1',
                    '{\"owner\":\"HTBR45\",\"table\":\"T\",\"target_table\":\"M\",\"columns\":[\"ID\"],\"primary_key\":[\"ID\"]}'
                );",
            )
            .unwrap();
        drop(connection);

        let store = TaskStore::open(&directory).unwrap();
        // 报错就说明没丢干净：那既不是「丢弃」，也没有从界面上恢复的办法。
        assert!(store.list().unwrap().is_empty());
        assert_eq!(store.get("stale").unwrap(), None);

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
