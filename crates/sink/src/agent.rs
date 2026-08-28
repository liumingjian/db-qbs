//! 目标端 agent 的身份（ADR-0044 §2）。
//!
//! sink 进程**就是** agent。这个模块只回答一个问题：「我是谁」——一个跨重启稳定的
//! `agent_id`、一个给人看的名字、一个版本号。source 侧的注册、在线探测、以及每次运行
//! 开跑前的身份核对读的都是它（[`db_qbs_shared::AgentInfo`]）。
//!
//! **身份必须跨重启稳定**，否则它挡不住 ADR-0044 §1 要挡的那件事：source 上钉着的
//! 「这条 MySQL 数据源走 A 号 agent」在重启一次之后就自动认了任何应答者。所以 id 落盘，
//! 不是每次启动现生成。
//!
//! **id 文件不需要人来准备**：没有就现生成一个并写下去（0600），有就照读。让部署者
//! 手抄一个 uuid 进 `sink.toml` 只会多一步能抄错的活，而这一步买不到任何东西——
//! id 的取值本身无意义，有意义的只是「它不变」。

use std::fs::{self, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use db_qbs_shared::AgentInfo;
use rand::RngCore;

/// 生成的 id 是 32 位小写十六进制（128 位随机）。**读进来的 id 不做格式校验**——
/// 手改过的、来自旧版本的、甚至一句人话，只要不变，身份核对就成立。
/// 校验它等于给部署者新开一类「文件在那儿，服务起不来」的故障，换不到任何东西。
const GENERATED_ID_BYTES: usize = 16;

/// 载入身份：id 文件有就读，没有就生成一个写下去。
///
/// `configured_name` 是 `sink.toml` 里的 `agent_name`；留空时取主机名，主机名也拿不到时
/// 退到 `db-qbs-sink`。名字**不作判据**（ADR-0044 §2），只进界面，所以它怎么退化都不影响正确性。
pub fn load_or_create(id_file: &Path, configured_name: Option<&str>) -> Result<AgentInfo, String> {
    let agent_id = match fs::read_to_string(id_file) {
        Ok(text) if !text.trim().is_empty() => text.trim().to_owned(),
        Ok(_) | Err(_) => {
            let generated = generate_agent_id();
            write_agent_id(id_file, &generated)?;
            generated
        }
    };
    Ok(AgentInfo {
        agent_id,
        name: resolve_name(configured_name),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        // 身份在监听之前就载入，那时候一条 MySQL 连接都还没有（ADR-0037 §2）。
        // 版本由 `Api::agent_info` 在应答时补上，见 #257。
        mysql: None,
    })
}

fn resolve_name(configured: Option<&str>) -> String {
    if let Some(name) = configured.map(str::trim).filter(|name| !name.is_empty()) {
        return name.to_owned();
    }
    fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|hostname| hostname.trim().to_owned())
        .filter(|hostname| !hostname.is_empty())
        .unwrap_or_else(|| "db-qbs-sink".to_owned())
}

fn generate_agent_id() -> String {
    let mut bytes = [0_u8; GENERATED_ID_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 0600 落盘。id 不是秘密（它就在未鉴权的 `/v1/agent/info` 上摆着），
/// 权限收紧只是不让同机的别人改它——改掉它等于把这台 agent 从 source 上顶下线。
fn write_agent_id(id_file: &Path, agent_id: &str) -> Result<(), String> {
    if let Some(parent) = id_file.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("创建 agent 身份文件目录失败：{error}"))?;
        }
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(id_file)
        .map_err(|error| format!("写 agent 身份文件 {} 失败：{error}", id_file.display()))?;
    file.write_all(agent_id.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|error| format!("写 agent 身份文件失败：{error}"))?;
    fs::set_permissions(id_file, Permissions::from_mode(0o600))
        .map_err(|error| format!("设置 agent 身份文件权限失败：{error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "db-qbs-agent-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// 身份**跨重启稳定**：第二次载入必须拿到同一个 id。这是 ADR-0044 §1 那条
    /// 「同一个地址上换了 agent 要被抓出来」的地基，塌了整条判定就空转。
    #[test]
    fn agent_id_survives_a_restart() {
        let directory = temp_dir("stable");
        let id_file = directory.join("agent-id");

        let first = load_or_create(&id_file, None).unwrap();
        let second = load_or_create(&id_file, None).unwrap();

        assert_eq!(first.agent_id, second.agent_id);
        assert!(!first.agent_id.is_empty());
    }

    /// 空文件等于没有：现生成一个写回去，不要把空串当成一个合法身份四处传。
    #[test]
    fn empty_id_file_is_regenerated() {
        let directory = temp_dir("empty");
        let id_file = directory.join("agent-id");
        fs::write(&id_file, "  \n").unwrap();

        let info = load_or_create(&id_file, None).unwrap();

        assert!(!info.agent_id.trim().is_empty());
        assert_eq!(
            fs::read_to_string(&id_file).unwrap().trim(),
            info.agent_id,
            "生成的 id 必须落盘，否则下次重启又是一个新身份"
        );
    }

    /// 配置里的名字优先；留空退到主机名那条路径（这里只断言「非空」，
    /// 主机名在容器里是什么不该成为判据）。
    #[test]
    fn configured_name_wins_and_blank_falls_back() {
        let directory = temp_dir("name");
        let id_file = directory.join("agent-id");

        assert_eq!(
            load_or_create(&id_file, Some(" 目标端 A ")).unwrap().name,
            "目标端 A"
        );
        assert!(!load_or_create(&id_file, Some("   "))
            .unwrap()
            .name
            .is_empty());
    }
}
