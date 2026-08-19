//! 数据源：source 侧 SQLite 是两类数据源的唯一真相源（ADR-0037 §1）。
//!
//! 三条边界，改动前先读 ADR-0037：
//! 1. **口令不明文落盘**（§3）——`settings` 里的 `password` 存的是 [`crate::secret`] 的密文。
//! 2. **口令只出不回**（§5）——API 面走 [`DatasourceView`]，连密文都不回；
//!    界面上只看得到「已设置 / 未设置」。改口令是只写操作：提交空串表示「不改」。
//! 3. **`oracle_client_lib_dir` 不是数据源级字段**（§6）——ODPI-C 一个进程只初始化一次，
//!    第二个值会被**静默忽略**，所以它留在 `source.toml` 作进程级配置。

use std::fs::{self, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use db_qbs_shared::TargetConnection;
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::secret::SecretBox;
use crate::OracleAccess;

const DATABASE_FILE: &str = "db-qbs.sqlite3";

/// 一条数据源的连接设置。**落盘时 `password` 是密文**，读出来仍是密文——
/// 解密只在 [`DatasourceStore::oracle_access`] / [`DatasourceStore::target_connection`]
/// 这两处发生，明文的活动范围因此收得住。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DatasourceSettings {
    Oracle {
        connect_string: String,
        username: String,
        #[serde(default)]
        password: String,
    },
    Mysql {
        host: String,
        port: u16,
        username: String,
        #[serde(default)]
        password: String,
        database: String,
    },
}

impl DatasourceSettings {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Oracle { .. } => "oracle",
            Self::Mysql { .. } => "mysql",
        }
    }

    fn password(&self) -> &str {
        match self {
            Self::Oracle { password, .. } | Self::Mysql { password, .. } => password,
        }
    }

    fn set_password(&mut self, value: String) {
        match self {
            Self::Oracle { password, .. } | Self::Mysql { password, .. } => *password = value,
        }
    }

    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Oracle {
                connect_string,
                username,
                ..
            } => {
                if connect_string.trim().is_empty() {
                    return Err("Oracle 数据源的 connect_string 不能为空".to_owned());
                }
                if username.trim().is_empty() {
                    return Err("Oracle 数据源的 username 不能为空".to_owned());
                }
            }
            Self::Mysql {
                host,
                port,
                database,
                ..
            } => {
                if host.trim().is_empty() {
                    return Err("MySQL 数据源的 host 不能为空".to_owned());
                }
                if *port == 0 {
                    return Err("MySQL 数据源的 port 不能为 0".to_owned());
                }
                if database.trim().is_empty() {
                    return Err("MySQL 数据源的 database 不能为空".to_owned());
                }
            }
        }
        Ok(())
    }
}

/// 库里的一条数据源。`settings.password` 是密文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datasource {
    pub datasource_id: String,
    pub name: String,
    pub settings: DatasourceSettings,
}

/// 请求体。`password` 留空表示「不改」（新建时表示「没有口令」）。
///
/// **不加 `deny_unknown_fields`**：serde 的 `flatten` 与它不兼容（未知字段会先被 flatten 吞掉），
/// 二选一时选 `flatten`——形状与 [`DatasourceView`] 对称，web 两边共用一个 `kind` 判别。
#[derive(Debug, Clone, Deserialize)]
pub struct DatasourceInput {
    pub name: String,
    #[serde(flatten)]
    pub settings: DatasourceSettings,
}

/// API 面的数据源。**没有 `password` 字段，连密文都没有**（ADR-0037 §5）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatasourceView {
    pub datasource_id: String,
    pub name: String,
    #[serde(flatten)]
    pub settings: DatasourceSettingsView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DatasourceSettingsView {
    Oracle {
        connect_string: String,
        username: String,
        has_password: bool,
    },
    Mysql {
        host: String,
        port: u16,
        username: String,
        database: String,
        has_password: bool,
    },
}

impl Datasource {
    pub fn view(&self) -> DatasourceView {
        let has_password = !self.settings.password().is_empty();
        let settings = match &self.settings {
            DatasourceSettings::Oracle {
                connect_string,
                username,
                ..
            } => DatasourceSettingsView::Oracle {
                connect_string: connect_string.clone(),
                username: username.clone(),
                has_password,
            },
            DatasourceSettings::Mysql {
                host,
                port,
                username,
                database,
                ..
            } => DatasourceSettingsView::Mysql {
                host: host.clone(),
                port: *port,
                username: username.clone(),
                database: database.clone(),
                has_password,
            },
        };
        DatasourceView {
            datasource_id: self.datasource_id.clone(),
            name: self.name.clone(),
            settings,
        }
    }
}

pub struct DatasourceStore {
    connection: Connection,
    secrets: SecretBox,
}

impl DatasourceStore {
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
                "CREATE TABLE IF NOT EXISTS datasources (
                    datasource_id TEXT PRIMARY KEY NOT NULL,
                    name          TEXT NOT NULL,
                    kind          TEXT NOT NULL,
                    settings      TEXT NOT NULL
                );",
            )
            .map_err(|error| format!("初始化 SQLite 数据源表失败：{error}"))?;

        Ok(Self {
            connection,
            secrets: SecretBox::open(data_dir)?,
        })
    }

    pub fn create(&self, input: DatasourceInput) -> Result<Datasource, String> {
        let datasource = self.sealed_datasource(generate_datasource_id(), input)?;
        self.insert(&datasource)?;
        Ok(datasource)
    }

    pub fn list(&self) -> Result<Vec<Datasource>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT datasource_id, name, settings FROM datasources ORDER BY rowid")
            .map_err(|error| format!("准备 SQLite 数据源列表查询失败：{error}"))?;
        let datasources = statement
            .query_map([], datasource_from_row)
            .map_err(|error| format!("查询 SQLite 数据源列表失败：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取 SQLite 数据源列表失败：{error}"))?;
        Ok(datasources)
    }

    pub fn get(&self, datasource_id: &str) -> Result<Option<Datasource>, String> {
        self.connection
            .query_row(
                "SELECT datasource_id, name, settings FROM datasources WHERE datasource_id = ?1",
                [datasource_id],
                datasource_from_row,
            )
            .optional()
            .map_err(|error| format!("查询 SQLite 数据源失败：{error}"))
    }

    /// 改口令是**只写**的：`password` 留空 = 沿用库里那份密文（ADR-0037 §5）。
    /// 这条与「响应不回口令」是一对——不回读就必须有一个「不改」的表达方式，
    /// 否则每次改个端口都要用户重打一遍口令。
    pub fn update(
        &self,
        datasource_id: &str,
        input: DatasourceInput,
    ) -> Result<Option<Datasource>, String> {
        let Some(existing) = self.get(datasource_id)? else {
            return Ok(None);
        };
        let mut datasource = self.sealed_datasource(datasource_id.to_owned(), input)?;
        if datasource.settings.password().is_empty() {
            datasource
                .settings
                .set_password(existing.settings.password().to_owned());
        }
        self.connection
            .execute(
                "UPDATE datasources SET name = ?2, kind = ?3, settings = ?4 WHERE datasource_id = ?1",
                params![
                    datasource.datasource_id,
                    datasource.name,
                    datasource.settings.kind(),
                    settings_json(&datasource.settings)?
                ],
            )
            .map_err(|error| format!("更新 SQLite 数据源失败：{error}"))?;
        Ok(Some(datasource))
    }

    pub fn delete(&self, datasource_id: &str) -> Result<Option<Datasource>, String> {
        let Some(datasource) = self.get(datasource_id)? else {
            return Ok(None);
        };
        self.connection
            .execute(
                "DELETE FROM datasources WHERE datasource_id = ?1",
                [datasource_id],
            )
            .map_err(|error| format!("删除 SQLite 数据源失败：{error}"))?;
        Ok(Some(datasource))
    }

    /// 解出一份可用的 Oracle 连接信息。`client_lib_dir` 来自进程级配置（ADR-0037 §6）。
    pub fn oracle_access(
        &self,
        datasource_id: &str,
        client_lib_dir: &str,
    ) -> Result<OracleAccess, String> {
        let datasource = self.require(datasource_id)?;
        match datasource.settings {
            DatasourceSettings::Oracle {
                connect_string,
                username,
                password,
            } => Ok(OracleAccess {
                connect_string,
                username,
                password: self.unseal(&password)?,
                client_lib_dir: client_lib_dir.to_owned(),
            }),
            DatasourceSettings::Mysql { .. } => {
                Err(format!("数据源 {} 是 MySQL，不能当源端用", datasource.name))
            }
        }
    }

    /// 解出一份可用的目标端连接信息——**这是明文口令唯一进入内存的另一处**，
    /// 它随 `POST /v1/runs` 过线给 sink（ADR-0037 §1）。
    pub fn target_connection(&self, datasource_id: &str) -> Result<TargetConnection, String> {
        let datasource = self.require(datasource_id)?;
        match datasource.settings {
            DatasourceSettings::Mysql {
                host,
                port,
                username,
                password,
                database,
            } => Ok(TargetConnection {
                host,
                port,
                username,
                password: self.unseal(&password)?,
                database,
            }),
            DatasourceSettings::Oracle { .. } => Err(format!(
                "数据源 {} 是 Oracle，不能当目标端用",
                datasource.name
            )),
        }
    }

    /// 把**表单里当前填的值**解成一份可用的 Oracle 连接信息（ADR-0039 §3）。
    ///
    /// 与 [`Self::oracle_access`] 的差别只有一处：那一条按 id 读库，这一条吃调用方给的草稿——
    /// 「测通才让存」要测的是**还没存进去的那组值**，按 id 读根本读不到。
    pub fn draft_oracle_access(
        &self,
        datasource_id: Option<&str>,
        settings: &DatasourceSettings,
        client_lib_dir: &str,
    ) -> Result<OracleAccess, String> {
        settings.validate()?;
        match settings {
            DatasourceSettings::Oracle {
                connect_string,
                username,
                password,
            } => Ok(OracleAccess {
                connect_string: connect_string.clone(),
                username: username.clone(),
                password: self.draft_password(datasource_id, password)?,
                client_lib_dir: client_lib_dir.to_owned(),
            }),
            DatasourceSettings::Mysql { .. } => {
                Err("这条数据源是 MySQL，不能当源端用".to_owned())
            }
        }
    }

    /// 同上，目标端那一侧。
    pub fn draft_target_connection(
        &self,
        datasource_id: Option<&str>,
        settings: &DatasourceSettings,
    ) -> Result<TargetConnection, String> {
        settings.validate()?;
        match settings {
            DatasourceSettings::Mysql {
                host,
                port,
                username,
                password,
                database,
            } => Ok(TargetConnection {
                host: host.clone(),
                port: *port,
                username: username.clone(),
                password: self.draft_password(datasource_id, password)?,
                database: database.clone(),
            }),
            DatasourceSettings::Oracle { .. } => {
                Err("这条数据源是 Oracle，不能当目标端用".to_owned())
            }
        }
    }

    /// 草稿口令的解释规则：填了就用填的，**留空则取库里存的那一份**。
    ///
    /// 这与 [`Self::update`] 的「留空 = 不改」是**同一条规则**，ADR-0039 §3 明写两处不许分岔——
    /// 分岔的后果是「测的那份口令」与「存下去的那份口令」不是同一个，而测连正是为了防这件事。
    /// 新建态（`datasource_id` 为 `None`）留空就是真的没有口令。
    fn draft_password(
        &self,
        datasource_id: Option<&str>,
        provided: &str,
    ) -> Result<String, String> {
        if !provided.is_empty() {
            return Ok(provided.to_owned());
        }
        let Some(datasource_id) = datasource_id else {
            return Ok(String::new());
        };
        self.unseal(self.require(datasource_id)?.settings.password())
    }

    /// `source.toml` 里那三个已退役的 Oracle 字段的一次性迁移（ADR-0037 §10）。
    ///
    /// **判据是「表为空」**，所以它只可能发生一次：迁完就有条目了，之后再启动一律跳过。
    /// 返回迁出来的数据源 id，`None` 表示没迁（表非空，或字段不齐）。
    pub fn migrate_legacy_oracle(
        &self,
        connect_string: Option<&str>,
        username: Option<&str>,
        password: Option<&str>,
    ) -> Result<Option<String>, String> {
        if !self.list()?.is_empty() {
            return Ok(None);
        }
        let (Some(connect_string), Some(username)) = (connect_string, username) else {
            return Ok(None);
        };
        let datasource = self.create(DatasourceInput {
            name: "默认".to_owned(),
            settings: DatasourceSettings::Oracle {
                connect_string: connect_string.to_owned(),
                username: username.to_owned(),
                password: password.unwrap_or_default().to_owned(),
            },
        })?;
        Ok(Some(datasource.datasource_id))
    }

    fn require(&self, datasource_id: &str) -> Result<Datasource, String> {
        self.get(datasource_id)?
            .ok_or_else(|| format!("数据源 {datasource_id} 不存在"))
    }

    fn unseal(&self, sealed: &str) -> Result<String, String> {
        if sealed.is_empty() {
            return Ok(String::new());
        }
        self.secrets.open_secret(sealed)
    }

    fn sealed_datasource(
        &self,
        datasource_id: String,
        input: DatasourceInput,
    ) -> Result<Datasource, String> {
        if input.name.trim().is_empty() {
            return Err("数据源名称不能为空".to_owned());
        }
        input.settings.validate()?;
        let mut settings = input.settings;
        if !settings.password().is_empty() {
            settings.set_password(self.secrets.seal(settings.password())?);
        }
        Ok(Datasource {
            datasource_id,
            name: input.name,
            settings,
        })
    }

    fn insert(&self, datasource: &Datasource) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO datasources (datasource_id, name, kind, settings) VALUES (?1, ?2, ?3, ?4)",
                params![
                    datasource.datasource_id,
                    datasource.name,
                    datasource.settings.kind(),
                    settings_json(&datasource.settings)?
                ],
            )
            .map_err(|error| format!("写入 SQLite 数据源失败：{error}"))?;
        Ok(())
    }
}

fn datasource_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Datasource> {
    let encoded: String = row.get("settings")?;
    let settings = serde_json::from_str(&encoded).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            row.as_ref()
                .column_index("settings")
                .expect("the settings column was just read"),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })?;
    Ok(Datasource {
        datasource_id: row.get("datasource_id")?,
        name: row.get("name")?,
        settings,
    })
}

fn settings_json(settings: &DatasourceSettings) -> Result<String, String> {
    serde_json::to_string(settings).map_err(|error| format!("序列化数据源设置失败：{error}"))
}

fn generate_datasource_id() -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut random_bytes = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut random_bytes);
    let mut datasource_id = String::with_capacity(32);
    for byte in random_bytes {
        datasource_id.push(HEX[(byte >> 4) as usize] as char);
        datasource_id.push(HEX[(byte & 0x0f) as usize] as char);
    }
    datasource_id
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn mysql_input() -> DatasourceInput {
        DatasourceInput {
            name: "目标库".to_owned(),
            settings: DatasourceSettings::Mysql {
                host: "127.0.0.1".to_owned(),
                port: 3306,
                username: "sink".to_owned(),
                password: "change-me".to_owned(),
                database: "qbs".to_owned(),
            },
        }
    }

    #[test]
    fn a_stored_password_is_ciphertext_and_never_reaches_the_api_view() {
        let directory = temp_directory();
        let store = DatasourceStore::open(&directory).unwrap();
        let created = store.create(mysql_input()).unwrap();

        let stored: String = store
            .connection
            .query_row(
                "SELECT settings FROM datasources WHERE datasource_id = ?1",
                [&created.datasource_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!stored.contains("change-me"), "{stored}");

        let view = serde_json::to_string(&created.view()).unwrap();
        assert!(!view.contains("change-me"), "{view}");
        // 前引号不能省：`has_password":` 自己就含子串 `password":`，省掉就成了永假断言。
        assert!(!view.contains("\"password\":"), "{view}");
        assert!(view.contains("\"has_password\":true"), "{view}");

        // 解出来的那一份仍是原文——过线给 sink 的就是它。
        assert_eq!(
            store
                .target_connection(&created.datasource_id)
                .unwrap()
                .password,
            "change-me"
        );

        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_empty_password_on_update_keeps_the_stored_one() {
        let directory = temp_directory();
        let store = DatasourceStore::open(&directory).unwrap();
        let created = store.create(mysql_input()).unwrap();

        let mut input = mysql_input();
        input.settings = DatasourceSettings::Mysql {
            host: "10.0.0.9".to_owned(),
            port: 3307,
            username: "sink".to_owned(),
            password: String::new(),
            database: "qbs".to_owned(),
        };
        let updated = store
            .update(&created.datasource_id, input)
            .unwrap()
            .unwrap();

        let target = store.target_connection(&updated.datasource_id).unwrap();
        assert_eq!(target.host, "10.0.0.9");
        assert_eq!(target.port, 3307);
        assert_eq!(target.password, "change-me");

        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_datasource_of_the_wrong_kind_is_refused_by_name() {
        let directory = temp_directory();
        let store = DatasourceStore::open(&directory).unwrap();
        let created = store.create(mysql_input()).unwrap();

        let error = store
            .oracle_access(&created.datasource_id, "/opt/oracle")
            .unwrap_err();
        assert_eq!(error, "数据源 目标库 是 MySQL，不能当源端用");

        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_legacy_oracle_fields_migrate_once_and_only_into_an_empty_table() {
        let directory = temp_directory();
        let store = DatasourceStore::open(&directory).unwrap();

        let migrated = store
            .migrate_legacy_oracle(
                Some("//oracle:1521/XE"),
                Some("source_user"),
                Some("secret"),
            )
            .unwrap()
            .expect("空表 + 字段齐备时必须迁一条出来");
        let access = store.oracle_access(&migrated, "/opt/oracle").unwrap();
        assert_eq!(access.username, "source_user");
        assert_eq!(access.password, "secret");

        // 第二次启动：表非空，跳过。
        assert_eq!(
            store
                .migrate_legacy_oracle(Some("//other:1521/XE"), Some("other"), Some("other"))
                .unwrap(),
            None
        );
        assert_eq!(store.list().unwrap().len(), 1);

        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn temp_directory() -> PathBuf {
        let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "db-qbs-source-datasource-test-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }
}
