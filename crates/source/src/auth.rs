//! 登录、会话与口令。
//!
//! **这一层护住的是 source 的 HTTP 面，不是这套系统。** sink 那半边仍然没有任何鉴权，
//! 它握着目标库的 `DELETE`：能连上 sink 端口的人照旧可以清空重写任一目标表，
//! 完全绕过这里。部署形态（loopback / 反代）仍然是那一半唯一的防线。
//!
//! 账号只有一个（`admin`），没有注册，也没有第二个管理员。**默认口令 `admin` 长期有效**，
//! 只有人自己改才变——所以「能连上端口」与「进得来」之间隔的是两次输入，不是一道真正的墙。
//! 登录失败不限速、不冷却、不锁定，界面上也不提示还在用默认口令：三条都是所有者的裁定。
//! 忘了口令的出路只有一条，在 source 主机上跑 `db-qbs-source reset-password`。
//!
//! **会话票据明文落库**，不做二次哈希。理由不是省事：`data_dir` 的读权限本来就等于
//! 全盘失守（数据源口令的密钥就在同一个目录、同样 0600），对能读到这张表的人，
//! 多一层摘要只是多一步。它防不住的东西，别装作防得住。

use std::fs::{self, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use chrono::{DateTime, Utc};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};

const DATABASE_FILE: &str = "db-qbs.sqlite3";

/// 唯一的账号名。它不是配置项——单用户产品里「用户名」这一栏存在的意义只有一个：
/// 让登录表单看起来像个登录表单。
pub const USERNAME: &str = "admin";

/// 出厂口令。`reset-password` 把口令送回的也是它。
pub const DEFAULT_PASSWORD: &str = "admin";

/// 会话 cookie 的名字。前端一个字都碰不到它（`HttpOnly`），这里是它唯一的拼写。
pub const SESSION_COOKIE: &str = "db_qbs_session";

/// 闲置多久算过期：**滑动**，从最后一次请求起算，不是从登录起算。
///
/// 固定时长会在人盯着运行详情屏的时候把他踢出去——这套界面上有跑一小时的导入任务。
pub const SESSION_IDLE_SECONDS: i64 = 8 * 60 * 60;

/// 一次登录换来的票据：给浏览器的明文，以及它这一刻的滑动窗口还剩多久。
pub struct IssuedSession {
    pub token: String,
    pub max_age_seconds: i64,
}

pub struct AuthStore {
    connection: Connection,
}

impl AuthStore {
    /// 打开（必要时初始化）口令与会话两张表。
    ///
    /// **首次打开时把默认口令写进去**：没有这一步，全新部署会变成「登录墙立着、
    /// 但没有任何一组凭据进得去「。
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
                // `CHECK (id = 1)` 是这张表的全部形态：单用户不是一句注释，是一条约束。
                "CREATE TABLE IF NOT EXISTS credentials (
                    id            INTEGER PRIMARY KEY CHECK (id = 1),
                    password_hash TEXT NOT NULL
                );
                 CREATE TABLE IF NOT EXISTS sessions (
                    token         TEXT PRIMARY KEY NOT NULL,
                    created_at    INTEGER NOT NULL,
                    last_seen_at  INTEGER NOT NULL
                 );",
            )
            .map_err(|error| format!("初始化 SQLite 登录表失败：{error}"))?;

        let store = Self { connection };
        if store.password_hash()?.is_none() {
            store.write_password_hash(&hash_password(DEFAULT_PASSWORD)?)?;
        }
        Ok(store)
    }

    /// 口令对不对。用户名不对也一样是 `false`——**两种失败不分开报**，
    /// 因为分开报只会告诉试口令的人「账号叫 admin」，而那件事本来就写在文档里。
    pub fn verify_password(&self, username: &str, password: &str) -> Result<bool, String> {
        if username != USERNAME {
            return Ok(false);
        }
        let Some(stored) = self.password_hash()? else {
            return Ok(false);
        };
        verify_password(&stored, password)
    }

    /// 发一张新票据。**不动别的会话**：同一个账号允许多处同时登着（办公室一份、家里一份），
    /// 新登录踢掉旧登录在单账号产品里只会自伤——它防不住任何人，只会把自己踢下线。
    pub fn issue_session(&self, now: DateTime<Utc>) -> Result<IssuedSession, String> {
        let token = generate_token();
        let stamp = now.timestamp();
        self.connection
            .execute(
                "INSERT INTO sessions (token, created_at, last_seen_at) VALUES (?1, ?2, ?2)",
                params![token, stamp],
            )
            .map_err(|error| format!("写入会话失败：{error}"))?;
        Ok(IssuedSession {
            token,
            max_age_seconds: SESSION_IDLE_SECONDS,
        })
    }

    /// 认一张票据，认得过就**顺手把滑动窗口往前推**。
    ///
    /// 过期的当场删掉再判否：留着它只会让这张表长成一个没人清的坟场，
    /// 而「过期」与「从来没有过」对调用方是同一个答案。
    pub fn authenticate(&self, token: &str, now: DateTime<Utc>) -> Result<bool, String> {
        let stamp = now.timestamp();
        let last_seen: Option<i64> = self
            .connection
            .query_row(
                "SELECT last_seen_at FROM sessions WHERE token = ?1",
                params![token],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("读取会话失败：{error}"))?;
        let Some(last_seen) = last_seen else {
            return Ok(false);
        };
        if stamp - last_seen >= SESSION_IDLE_SECONDS {
            self.forget(token)?;
            return Ok(false);
        }
        self.connection
            .execute(
                "UPDATE sessions SET last_seen_at = ?2 WHERE token = ?1",
                params![token, stamp],
            )
            .map_err(|error| format!("刷新会话失败：{error}"))?;
        Ok(true)
    }

    /// 退出登录：只销这一张票，别处登着的不受影响（见 [`AuthStore::issue_session`]）。
    pub fn forget(&self, token: &str) -> Result<(), String> {
        self.connection
            .execute("DELETE FROM sessions WHERE token = ?1", params![token])
            .map_err(|error| format!("删除会话失败：{error}"))?;
        Ok(())
    }

    /// 改口令。**要先输当前口令**——改密入口挂在一个已经登录的会话后面，
    /// 而「已经登录」证明的是这台浏览器有票据，不是坐在它前面的还是同一个人。
    ///
    /// 改完**除了 `keep` 之外的会话全部失效**：改口令这个动作的常见动机就是
    /// 「我怀疑别处有人登着」，留着那些票据等于这次改密什么也没做。
    pub fn change_password(&self, current: &str, next: &str, keep: &str) -> Result<(), String> {
        if !self.verify_password(USERNAME, current)? {
            return Err("当前口令不正确".to_owned());
        }
        validate_new_password(next)?;
        self.write_password_hash(&hash_password(next)?)?;
        self.connection
            .execute("DELETE FROM sessions WHERE token <> ?1", params![keep])
            .map_err(|error| format!("清理其它会话失败：{error}"))?;
        Ok(())
    }

    /// `reset-password` 的全部动作：口令回到默认值，**所有**会话一并作废。
    ///
    /// 会话一起清是这条命令的一半：跑它的场景是「我进不去了」，而进不去的人无从判断
    /// 此刻还有谁的浏览器攥着一张有效票据。
    pub fn reset_password(&self) -> Result<(), String> {
        self.write_password_hash(&hash_password(DEFAULT_PASSWORD)?)?;
        self.connection
            .execute("DELETE FROM sessions", [])
            .map_err(|error| format!("清理会话失败：{error}"))?;
        Ok(())
    }

    /// 还在用出厂口令吗。只有启动日志读它——**界面上一个字都不提**（所有者裁定）。
    pub fn uses_default_password(&self) -> Result<bool, String> {
        self.verify_password(USERNAME, DEFAULT_PASSWORD)
    }

    fn password_hash(&self) -> Result<Option<String>, String> {
        self.connection
            .query_row(
                "SELECT password_hash FROM credentials WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("读取口令失败：{error}"))
    }

    fn write_password_hash(&self, hash: &str) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO credentials (id, password_hash) VALUES (1, ?1)
                 ON CONFLICT (id) DO UPDATE SET password_hash = excluded.password_hash",
                params![hash],
            )
            .map_err(|error| format!("写入口令失败：{error}"))?;
        Ok(())
    }
}

/// 新口令的全部规矩：**非空**。
///
/// 没有长度下限、没有字符类要求，因为出厂口令就是 `admin` 且长期有效——
/// 在那之上立一条「新口令至少八位」的规矩，拦不住任何人，只会让改口令这件事更烦。
/// 空口令另说：它不是弱口令，它是把登录表单变成一个纯装饰。
pub fn validate_new_password(password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("新口令不能为空".to_owned());
    }
    Ok(())
}

/// 从 `Cookie` 请求头里取出会话票据。
///
/// 手写而不是拉一个 cookie 库：这里只需要认一个名字，而 `Cookie` 头的形态
/// （`a=1; b=2`）在这个用途上没有暗礁。
pub fn session_token_from_cookie_header(header: &str) -> Option<&str> {
    header.split(';').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name.trim() == SESSION_COOKIE).then(|| value.trim())
    })
}

/// 登录成功时那一条 `Set-Cookie`。
///
/// **没有 `Secure`**：现场部署是明文 HTTP（loopback 或反代后面），带上 `Secure`
/// 会让 cookie 根本存不下来，登录当场变成一个永远登不进去的表单。明文链路上
/// 票据可读这件事是真的，它的防线是 TLS 由部署者在反代上终结，不是这一行。
pub fn session_cookie_header(token: &str, max_age_seconds: i64) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age_seconds}"
    )
}

/// 退出登录时那一条：同样的属性，`Max-Age=0`。属性对不上浏览器就不认这次删除。
pub fn cleared_cookie_header() -> String {
    format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

fn generate_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| format!("计算口令散列失败：{error}"))
}

/// 存下来的散列认不出来时判**否**，不判错。
///
/// 这一格只会在库被人手动改花了的时候出现，而那时唯一安全的回答是「这个口令不对」——
/// 把它当成内部错误放行，等于给一条坏掉的记录开后门。
fn verify_password(stored: &str, password: &str) -> Result<bool, String> {
    let Ok(parsed) = PasswordHash::new(stored) else {
        return Ok(false);
    };
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}
