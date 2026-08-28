//! 目标端 agent 注册表（ADR-0044）。
//!
//! **一台 agent = 一个 sink 进程。** 目标库只能经它访问：source 一条 MySQL 连接都不建
//! （`CONTEXT.md` 那条不对称），元数据、测连、写入三条链全部落在这里选中的那台 agent 上。
//!
//! 三条边界，改动前先读 ADR-0044：
//! 1. **没有全局兜底**（§1）——`source.toml` 的 `sink_base_url` 退役成一次性迁移。
//!    没绑 agent 的 MySQL 数据源不是「用默认那台」，而是**不能用**。
//! 2. **身份钉住地址**（§2）——注册时把 agent 自报的 `agent_id` 记下来（[`Agent::instance_id`]），
//!    之后每次探测、每次开跑都比一次。地址还通但换了个 agent 应答，判**不在线**，
//!    这正是「停了 agent 却照样同步」那类静默的抓手。
//! 3. **注册要求对方活着**（§3）——注册与改址都当场打一次 `/v1/agent/info`，打不通就不落库。
//!    库里因此不会出现「从没连通过的 agent」这种只会误导人的记录。

use std::fs::{self, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;

use db_qbs_shared::AgentInfo;
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DATABASE_FILE: &str = "db-qbs.sqlite3";

/// 注册表里的一台 agent。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Agent {
    /// source 侧这条记录的 id，与 agent 自报的身份**不是一回事**。
    pub agent_id: String,
    /// 展示名。注册时预填 agent 自报的名字，之后由人改，改了不再被探测覆盖——
    /// 界面上那一列是给人认机器的，被远端覆写会让人刚改完的名字自己变回去。
    pub name: String,
    pub base_url: String,
    /// agent 自报的稳定身份（[`AgentInfo::agent_id`]），注册时钉下。
    pub instance_id: String,
    pub version: String,
    /// 最近一次探通的时间（RFC 3339）。**从没探通过是 `None`**，
    /// 但注册本身要求探通，所以新记录一定有值。
    pub last_seen_at: Option<String>,
    pub status: AgentStatus,
    /// 最近一次探测失败的人话原因；在线时为 `None`。
    pub last_error: Option<String>,
    /// agent 上报的、它所连 MySQL 的版本（`@@version` 原样）。**`None` 是「还没报过」**——
    /// 旧版本 agent 不带这个字段，新 agent 也要等它手上有过一次目标端凭据之后才报得出来
    /// （#257）。界面上那一列读的就是它。
    pub mysql_version: Option<String>,
    /// agent 上报的 utf8mb4 默认字符序，生成建表语句时用（#257）。
    ///
    /// **`None` 时不许拿 8.0 的默认值顶上**：生成的建表语句会照旧只写
    /// `DEFAULT CHARSET=utf8mb4`，字符序交给目标库自己的默认值——那正是本票之前的行为，
    /// 也是唯一一个不掺猜测的退化。
    pub mysql_collation: Option<String>,
    /// agent 自报的并发额度（`sink.toml` 的 `max_concurrent_runs`，#260）。
    ///
    /// **`None` 是「这台 agent 没说」**——旧版本 agent 不带这个字段。调度器读到 `None`
    /// 时按**一次一个**派发（#266）：那是唯一一个绝不会撞上 `RUN_QUOTA_EXCEEDED` 的取值，
    /// 而拿 sink 的默认值 4 顶上就是猜——那台 agent 完全可能配着 2。
    pub max_concurrent_runs: Option<u32>,
}

/// agent 的在线状态。**三档，不是两档**：`Mismatch` 与 `Offline` 分开报，
/// 因为处置完全不同——一个是把服务起起来，一个是「这个地址后面站的不是你以为的那台」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Online,
    Offline,
    Mismatch,
}

impl AgentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Offline => "offline",
            Self::Mismatch => "mismatch",
        }
    }
}

/// 注册 / 改址的请求体。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInput {
    pub name: String,
    pub base_url: String,
}

pub struct AgentStore {
    connection: Connection,
}

impl AgentStore {
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
                "CREATE TABLE IF NOT EXISTS agents (
                    agent_id     TEXT PRIMARY KEY NOT NULL,
                    name         TEXT NOT NULL,
                    base_url     TEXT NOT NULL,
                    instance_id  TEXT NOT NULL,
                    version      TEXT NOT NULL,
                    last_seen_at TEXT,
                    status       TEXT NOT NULL,
                    last_error   TEXT
                );",
            )
            .map_err(|error| format!("初始化 SQLite agent 表失败：{error}"))?;
        // #257 之前建的库没有这两列，老记录补出来就是 `NULL`：那是「这台 agent 还没报过
        // 版本」，与新 agent 从没被探到过是同一档，处置也一样。
        ensure_nullable_column(&connection, "mysql_version", "TEXT")?;
        ensure_nullable_column(&connection, "mysql_collation", "TEXT")?;
        // #266 之前建的库没有这一列，老记录补出来是 `NULL` =「这台 agent 还没报过额度」。
        ensure_nullable_column(&connection, "max_concurrent_runs", "INTEGER")?;
        Ok(Self { connection })
    }

    /// 注册一台已经探通的 agent。`info` 是刚探回来的那一份，**调用方负责先探**——
    /// 把探测放在 store 里等于让它长出一条网络依赖，测试要起 HTTP 服务才跑得动。
    pub fn register(
        &self,
        input: &AgentInput,
        info: &AgentInfo,
        now: &str,
    ) -> Result<Agent, String> {
        let (mysql_version, mysql_collation) = reported_mysql(info);
        let agent = Agent {
            agent_id: generate_agent_id(),
            name: display_name(&input.name, info),
            base_url: normalize_base_url(&input.base_url)?,
            instance_id: info.agent_id.clone(),
            version: info.version.clone(),
            last_seen_at: Some(now.to_owned()),
            status: AgentStatus::Online,
            last_error: None,
            mysql_version,
            mysql_collation,
            max_concurrent_runs: info.max_concurrent_runs,
        };
        self.connection
            .execute(
                "INSERT INTO agents (agent_id, name, base_url, instance_id, version, last_seen_at, status, last_error,
                                     mysql_version, mysql_collation, max_concurrent_runs)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9, ?10)",
                params![
                    agent.agent_id,
                    agent.name,
                    agent.base_url,
                    agent.instance_id,
                    agent.version,
                    agent.last_seen_at,
                    agent.status.as_str(),
                    agent.mysql_version,
                    agent.mysql_collation,
                    agent.max_concurrent_runs,
                ],
            )
            .map_err(|error| format!("写入 SQLite agent 失败：{error}"))?;
        Ok(agent)
    }

    /// 改名 / 改址。**改址会重新钉身份**：换机器、重装、换证书都会换出一个新的 `agent_id`，
    /// 而这是一次人明确发起的动作，不是静默替换——静默那一类由探测抓（[`Self::record_probe`]）。
    pub fn update(
        &self,
        agent_id: &str,
        input: &AgentInput,
        info: &AgentInfo,
        now: &str,
    ) -> Result<Option<Agent>, String> {
        if self.get(agent_id)?.is_none() {
            return Ok(None);
        }
        let (mysql_version, mysql_collation) = reported_mysql(info);
        let agent = Agent {
            agent_id: agent_id.to_owned(),
            name: display_name(&input.name, info),
            base_url: normalize_base_url(&input.base_url)?,
            instance_id: info.agent_id.clone(),
            version: info.version.clone(),
            last_seen_at: Some(now.to_owned()),
            status: AgentStatus::Online,
            last_error: None,
            mysql_version,
            mysql_collation,
            max_concurrent_runs: info.max_concurrent_runs,
        };
        self.connection
            .execute(
                "UPDATE agents SET name = ?2, base_url = ?3, instance_id = ?4, version = ?5,
                                   last_seen_at = ?6, status = ?7, last_error = NULL,
                                   mysql_version = ?8, mysql_collation = ?9,
                                   max_concurrent_runs = ?10
                 WHERE agent_id = ?1",
                params![
                    agent.agent_id,
                    agent.name,
                    agent.base_url,
                    agent.instance_id,
                    agent.version,
                    agent.last_seen_at,
                    agent.status.as_str(),
                    agent.mysql_version,
                    agent.mysql_collation,
                    agent.max_concurrent_runs,
                ],
            )
            .map_err(|error| format!("更新 SQLite agent 失败：{error}"))?;
        Ok(Some(agent))
    }

    pub fn list(&self) -> Result<Vec<Agent>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT agent_id, name, base_url, instance_id, version, last_seen_at, status, last_error,
                        mysql_version, mysql_collation, max_concurrent_runs
                 FROM agents ORDER BY rowid",
            )
            .map_err(|error| format!("准备 SQLite agent 列表查询失败：{error}"))?;
        let agents = statement
            .query_map([], agent_from_row)
            .map_err(|error| format!("查询 SQLite agent 列表失败：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取 SQLite agent 列表失败：{error}"))?;
        Ok(agents)
    }

    pub fn get(&self, agent_id: &str) -> Result<Option<Agent>, String> {
        self.connection
            .query_row(
                "SELECT agent_id, name, base_url, instance_id, version, last_seen_at, status, last_error,
                        mysql_version, mysql_collation, max_concurrent_runs
                 FROM agents WHERE agent_id = ?1",
                [agent_id],
                agent_from_row,
            )
            .optional()
            .map_err(|error| format!("查询 SQLite agent 失败：{error}"))
    }

    pub fn require(&self, agent_id: &str) -> Result<Agent, String> {
        self.get(agent_id)?
            .ok_or_else(|| format!("目标端 agent {agent_id} 不存在，请先在「目标端 Agent」屏注册"))
    }

    pub fn delete(&self, agent_id: &str) -> Result<Option<Agent>, String> {
        let Some(agent) = self.get(agent_id)? else {
            return Ok(None);
        };
        self.connection
            .execute("DELETE FROM agents WHERE agent_id = ?1", [agent_id])
            .map_err(|error| format!("删除 SQLite agent 失败：{error}"))?;
        Ok(Some(agent))
    }

    /// 记一次探测结果。探通且身份对得上 → `Online`；身份对不上 → `Mismatch`；
    /// 打不通 → `Offline`。**探测不改 `instance_id`**：钉住的那一份是判据本身，
    /// 让探测覆写它等于让被顶替的 agent 自己把证据擦掉。
    pub fn record_probe(
        &self,
        agent_id: &str,
        result: &Result<AgentInfo, String>,
        now: &str,
    ) -> Result<Option<Agent>, String> {
        let Some(existing) = self.get(agent_id)? else {
            return Ok(None);
        };
        // agent 自报的那几样（MySQL 版本与字符序、并发额度）只在**探通且身份对得上**
        // 那一档才更新——这条规则原来写了三遍，三个 `match` 各自重述一次身份判据。
        // 身份对不上那台报的是它自己那边的库，抄过来等于让顶替者改写被顶替者的档案；
        // 探不通就更没有新值可言，留住上一次的真值——把它抹成「未知」只会让建表语句
        // 在 agent 短暂离线期间悄悄换一份字符序。
        let reported = probed_self(result, &existing);
        let (mysql_version, mysql_collation) = match reported.map(reported_mysql) {
            Some((version @ Some(_), collation @ Some(_))) => (version, collation),
            // 探通了但这台 agent 还没报过 MySQL：那也不是新值，同样留住上一次那份。
            _ => (
                existing.mysql_version.clone(),
                existing.mysql_collation.clone(),
            ),
        };
        let max_concurrent_runs = match reported {
            Some(info) => info.max_concurrent_runs,
            None => existing.max_concurrent_runs,
        };
        let (status, last_error, version, last_seen_at) = match (reported, result) {
            (Some(info), _) => (
                AgentStatus::Online,
                None,
                info.version.clone(),
                Some(now.to_owned()),
            ),
            (None, Ok(info)) => (
                AgentStatus::Mismatch,
                Some(format!(
                    "这个地址上应答的是另一台 agent（注册时钉的是 {}，现在应答的是 {}）",
                    existing.instance_id, info.agent_id
                )),
                existing.version.clone(),
                existing.last_seen_at.clone(),
            ),
            (None, Err(error)) => (
                AgentStatus::Offline,
                Some(error.clone()),
                existing.version.clone(),
                existing.last_seen_at.clone(),
            ),
        };
        // 迁移进来的那条记录 `instance_id` 是空的（ADR-0044 §5）：第一次探通就把它补上，
        // 从那一刻起它才真正被身份钉住。
        let instance_id = match result {
            Ok(info) if existing.instance_id.is_empty() => info.agent_id.clone(),
            _ => existing.instance_id.clone(),
        };
        self.connection
            .execute(
                "UPDATE agents SET instance_id = ?2, version = ?3, last_seen_at = ?4,
                                   status = ?5, last_error = ?6,
                                   mysql_version = ?7, mysql_collation = ?8,
                                   max_concurrent_runs = ?9
                 WHERE agent_id = ?1",
                params![
                    agent_id,
                    instance_id,
                    version,
                    last_seen_at,
                    status.as_str(),
                    last_error,
                    mysql_version,
                    mysql_collation,
                    max_concurrent_runs,
                ],
            )
            .map_err(|error| format!("更新 SQLite agent 探测结果失败：{error}"))?;
        self.get(agent_id)
    }

    /// `source.toml` 的 `sink_base_url` 一次性迁移（ADR-0044 §5）。
    ///
    /// **判据是「表为空」**，与 ADR-0037 §10 那次 Oracle 迁移同一形态：迁完就有条目了，
    /// 之后再启动一律跳过。迁出来的记录 `instance_id` 是空的、状态是 `Offline`——
    /// 迁移发生在启动早期，这时候还不该去打网络；第一次探测会把它补齐。
    pub fn migrate_legacy_sink_base_url(
        &self,
        sink_base_url: Option<&str>,
    ) -> Result<Option<String>, String> {
        if !self.list()?.is_empty() {
            return Ok(None);
        }
        let Some(base_url) = sink_base_url.map(str::trim).filter(|url| !url.is_empty()) else {
            return Ok(None);
        };
        let agent_id = generate_agent_id();
        self.connection
            .execute(
                "INSERT INTO agents (agent_id, name, base_url, instance_id, version, last_seen_at, status, last_error,
                                     mysql_version, mysql_collation, max_concurrent_runs)
                 VALUES (?1, '默认', ?2, '', '', NULL, 'offline', '尚未探测', NULL, NULL, NULL)",
                params![agent_id, normalize_base_url(base_url)?],
            )
            .map_err(|error| format!("迁移 sink_base_url 失败：{error}"))?;
        Ok(Some(agent_id))
    }
}

/// 补一列可空的列（#257 的两列 TEXT、#266 的一列 INTEGER）。
///
/// 判据是 `pragma_table_info`，与 `run_history` 那两处同一条路子。类型是参数不是两份
/// 拷贝：两份除了 `TEXT` / `INTEGER` 一个字不差，而差的那个字恰恰是唯一该看见的东西。
fn ensure_nullable_column(
    connection: &Connection,
    name: &str,
    column_type: &str,
) -> Result<(), String> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('agents') WHERE name = ?1)",
            [name],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查 SQLite agent 列 {name} 失败：{error}"))?;
    if exists {
        return Ok(());
    }
    connection
        .execute(
            &format!("ALTER TABLE agents ADD COLUMN {name} {column_type}"),
            [],
        )
        .map_err(|error| format!("迁移 SQLite agent 列 {name} 失败：{error}"))?;
    Ok(())
}

/// 这次探测是不是**这台 agent 自己**应答的：探通，且身份与注册时钉下的那一份对得上。
///
/// 空的 `instance_id` 是迁移进来的老记录（ADR-0044 §5），第一次探通就认。
/// 「谁有资格更新缓存下来的自报值」只有这一处判据——它原来在 `record_probe` 里
/// 被三个 `match` 各自重述了一遍。
fn probed_self<'a>(
    result: &'a Result<AgentInfo, String>,
    existing: &Agent,
) -> Option<&'a AgentInfo> {
    match result {
        Ok(info) if info.agent_id == existing.instance_id || existing.instance_id.is_empty() => {
            Some(info)
        }
        _ => None,
    }
}

/// agent 自报的 MySQL 版本与字符序，收成入库的两列。没报过就是两个 `None`——
/// **不补默认值**（见 [`Agent::mysql_collation`]）。
fn reported_mysql(info: &AgentInfo) -> (Option<String>, Option<String>) {
    match info.mysql.as_ref() {
        Some(mysql) => (
            Some(mysql.version.clone()),
            Some(mysql.utf8mb4_collation.clone()),
        ),
        None => (None, None),
    }
}

/// 名字留空就用 agent 自报的那一个。**名字不作判据**，所以随便退化都不伤正确性。
fn display_name(provided: &str, info: &AgentInfo) -> String {
    let provided = provided.trim();
    if !provided.is_empty() {
        return provided.to_owned();
    }
    if info.name.trim().is_empty() {
        return "未命名 agent".to_owned();
    }
    info.name.trim().to_owned()
}

/// 地址的规范形式：去掉尾巴上的 `/`，并按 [`crate::HttpSinkClient`] 那条同样的规矩校验。
///
/// **只收 http**：与 `protocol.rs` 里那条「非 http 一律拒」保持一字不差——机密性由部署者
/// 自建的隧道给（ADR-0041 §4），产品这一侧不假装自己有 TLS。
pub fn normalize_base_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err("agent 地址不能为空".to_owned());
    }
    let parsed = url::Url::parse(trimmed).map_err(|error| format!("agent 地址无效：{error}"))?;
    if parsed.scheme() != "http" {
        return Err("agent 地址必须是 http://（TLS 由部署者自建的隧道提供）".to_owned());
    }
    if parsed.host_str().is_none() {
        return Err("agent 地址必须带主机名".to_owned());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("agent 地址不能带 query 或 fragment".to_owned());
    }
    Ok(trimmed.trim_end_matches('/').to_owned())
}

/// 打一次 `GET /v1/agent/info`。**超时短**（连 5s、读 5s）：它挂在界面的同步路径上，
/// 一台掉线的 agent 不该把整屏拖住。
pub fn fetch_agent_info(base_url: &str) -> Result<AgentInfo, String> {
    let url = format!("{}/v1/agent/info", base_url.trim_end_matches('/'));
    let http = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(5))
        .redirects(0)
        .build();
    match http.get(&url).call() {
        Ok(response) => response
            .into_json::<AgentInfo>()
            .map_err(|error| format!("这个地址回的不是 agent 身份：{error}")),
        Err(ureq::Error::Status(status, response)) => Err(response
            .into_json::<Value>()
            .ok()
            .and_then(|body| Some(body.get("error")?.get("message")?.as_str()?.to_owned()))
            .unwrap_or_else(|| {
                format!("这个地址回了 HTTP {status}，它多半不是 db-qbs 的目标端 agent")
            })),
        Err(ureq::Error::Transport(error)) => Err(format!("连不上 agent：{error}")),
    }
}

fn agent_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Agent> {
    let status: String = row.get("status")?;
    Ok(Agent {
        agent_id: row.get("agent_id")?,
        name: row.get("name")?,
        base_url: row.get("base_url")?,
        instance_id: row.get("instance_id")?,
        version: row.get("version")?,
        last_seen_at: row.get("last_seen_at")?,
        status: match status.as_str() {
            "online" => AgentStatus::Online,
            "mismatch" => AgentStatus::Mismatch,
            _ => AgentStatus::Offline,
        },
        last_error: row.get("last_error")?,
        mysql_version: row.get("mysql_version")?,
        mysql_collation: row.get("mysql_collation")?,
        max_concurrent_runs: row.get("max_concurrent_runs")?,
    })
}

fn generate_agent_id() -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut random_bytes = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut random_bytes);
    let mut agent_id = String::with_capacity(32);
    for byte in random_bytes {
        agent_id.push(HEX[(byte >> 4) as usize] as char);
        agent_id.push(HEX[(byte & 0x0f) as usize] as char);
    }
    agent_id
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use db_qbs_shared::MysqlServerInfo;

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "db-qbs-agent-store-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn info(agent_id: &str) -> AgentInfo {
        AgentInfo {
            agent_id: agent_id.to_owned(),
            name: "target-a".to_owned(),
            version: "0.1.0".to_owned(),
            mysql: None,
            max_concurrent_runs: None,
        }
    }

    fn info_with_mysql(agent_id: &str, version: &str, collation: &str) -> AgentInfo {
        AgentInfo {
            mysql: Some(MysqlServerInfo {
                version: version.to_owned(),
                utf8mb4_collation: collation.to_owned(),
            }),
            ..info(agent_id)
        }
    }

    fn input(base_url: &str) -> AgentInput {
        AgentInput {
            name: "目标端 A".to_owned(),
            base_url: base_url.to_owned(),
        }
    }

    #[test]
    fn registering_pins_the_reported_identity() {
        let store = AgentStore::open(&temp_dir()).unwrap();

        let agent = store
            .register(
                &input("http://127.0.0.1:8080/"),
                &info("aaa"),
                "2026-08-24T00:00:00Z",
            )
            .unwrap();

        assert_eq!(agent.instance_id, "aaa");
        assert_eq!(agent.base_url, "http://127.0.0.1:8080", "尾斜杠要归一");
        assert_eq!(agent.status, AgentStatus::Online);
        assert_eq!(store.list().unwrap(), vec![agent]);
    }

    /// 本票的核心判定（ADR-0044 §2）：地址还通、但应答的是另一台 agent，判 `Mismatch`
    /// 而不是 `Online`。判成在线就等于回到了「停掉 agent 照样同步」那个世界。
    #[test]
    fn a_different_agent_on_the_same_address_is_a_mismatch() {
        let store = AgentStore::open(&temp_dir()).unwrap();
        let agent = store
            .register(
                &input("http://127.0.0.1:8080"),
                &info("aaa"),
                "2026-08-24T00:00:00Z",
            )
            .unwrap();

        let probed = store
            .record_probe(&agent.agent_id, &Ok(info("bbb")), "2026-08-24T00:01:00Z")
            .unwrap()
            .unwrap();

        assert_eq!(probed.status, AgentStatus::Mismatch);
        assert_eq!(probed.instance_id, "aaa", "钉住的身份不许被探测覆写");
        assert_eq!(
            probed.last_seen_at.as_deref(),
            Some("2026-08-24T00:00:00Z"),
            "对不上的那次不算「见过」"
        );
        assert!(probed.last_error.unwrap().contains("bbb"));
    }

    #[test]
    fn an_unreachable_agent_goes_offline() {
        let store = AgentStore::open(&temp_dir()).unwrap();
        let agent = store
            .register(
                &input("http://127.0.0.1:8080"),
                &info("aaa"),
                "2026-08-24T00:00:00Z",
            )
            .unwrap();

        let probed = store
            .record_probe(
                &agent.agent_id,
                &Err("连不上 agent：connection refused".to_owned()),
                "2026-08-24T00:01:00Z",
            )
            .unwrap()
            .unwrap();

        assert_eq!(probed.status, AgentStatus::Offline);
        assert!(probed.last_error.unwrap().contains("connection refused"));
    }

    /// 迁移进来的那条记录没有身份（§5），第一次探通要把它补上——否则它永远处在
    /// 「谁应答都算数」的状态，那条判定就是个摆设。
    #[test]
    fn migrated_agent_adopts_the_identity_on_first_successful_probe() {
        let directory = temp_dir();
        let store = AgentStore::open(&directory).unwrap();
        let agent_id = store
            .migrate_legacy_sink_base_url(Some("http://127.0.0.1:8080"))
            .unwrap()
            .unwrap();

        let probed = store
            .record_probe(&agent_id, &Ok(info("aaa")), "2026-08-24T00:01:00Z")
            .unwrap()
            .unwrap();

        assert_eq!(probed.instance_id, "aaa");
        assert_eq!(probed.status, AgentStatus::Online);
        assert_eq!(
            store
                .migrate_legacy_sink_base_url(Some("http://127.0.0.1:9999"))
                .unwrap(),
            None,
            "表非空就不再迁，迁移只发生一次"
        );
    }

    /// #257：agent 报回来的 MySQL 版本与字符序要落库，界面与建表语句都读这一份。
    #[test]
    fn a_reported_mysql_version_is_recorded_on_register_and_refreshed_on_probe() {
        let store = AgentStore::open(&temp_dir()).unwrap();

        let agent = store
            .register(
                &input("http://127.0.0.1:8080"),
                &info_with_mysql("aaa", "8.0.36", "utf8mb4_0900_ai_ci"),
                "2026-08-24T00:00:00Z",
            )
            .unwrap();
        assert_eq!(agent.mysql_version.as_deref(), Some("8.0.36"));
        assert_eq!(agent.mysql_collation.as_deref(), Some("utf8mb4_0900_ai_ci"));

        // 同一台 agent 被指到另一台 MySQL 上（5.7），探一次就该换成新报的那一份。
        let probed = store
            .record_probe(
                &agent.agent_id,
                &Ok(info_with_mysql("aaa", "5.7.44-log", "utf8mb4_general_ci")),
                "2026-08-24T00:01:00Z",
            )
            .unwrap()
            .unwrap();
        assert_eq!(probed.mysql_version.as_deref(), Some("5.7.44-log"));
        assert_eq!(
            probed.mysql_collation.as_deref(),
            Some("utf8mb4_general_ci")
        );
    }

    /// #257 的「不静默猜测」那一半：agent 没报版本，库里就是空的，
    /// 生成建表语句的那一端据此走既有的 8.0 行为（不写 `COLLATE`）。
    #[test]
    fn an_agent_that_reports_no_mysql_version_leaves_the_column_empty() {
        let store = AgentStore::open(&temp_dir()).unwrap();

        let agent = store
            .register(
                &input("http://127.0.0.1:8080"),
                &info("aaa"),
                "2026-08-24T00:00:00Z",
            )
            .unwrap();

        assert_eq!(agent.mysql_version, None);
        assert_eq!(agent.mysql_collation, None);
    }

    /// 探不通、或者应答的是另一台 agent，都不许改写已经记下的那一份——
    /// 一次离线不该让建表语句悄悄换一份字符序。
    #[test]
    fn a_failed_probe_keeps_the_last_known_mysql_version() {
        let store = AgentStore::open(&temp_dir()).unwrap();
        let agent = store
            .register(
                &input("http://127.0.0.1:8080"),
                &info_with_mysql("aaa", "5.7.44", "utf8mb4_general_ci"),
                "2026-08-24T00:00:00Z",
            )
            .unwrap();

        let offline = store
            .record_probe(
                &agent.agent_id,
                &Err("连不上 agent：connection refused".to_owned()),
                "2026-08-24T00:01:00Z",
            )
            .unwrap()
            .unwrap();
        assert_eq!(offline.mysql_version.as_deref(), Some("5.7.44"));

        let mismatched = store
            .record_probe(
                &agent.agent_id,
                &Ok(info_with_mysql("bbb", "8.0.36", "utf8mb4_0900_ai_ci")),
                "2026-08-24T00:02:00Z",
            )
            .unwrap()
            .unwrap();
        assert_eq!(mismatched.status, AgentStatus::Mismatch);
        assert_eq!(
            mismatched.mysql_collation.as_deref(),
            Some("utf8mb4_general_ci"),
            "顶替者报的版本不许覆盖被顶替者的档案"
        );
    }

    #[test]
    fn base_url_must_be_http_with_a_host() {
        assert!(normalize_base_url("https://target:8080").is_err());
        assert!(normalize_base_url("http://target:8080?x=1").is_err());
        assert!(normalize_base_url("  ").is_err());
        assert_eq!(
            normalize_base_url("http://target:8080/").unwrap(),
            "http://target:8080"
        );
    }
}
