//! 长驻服务：把 store 备齐、监听、把每个 tiny_http 请求交给 `Api::handle`。
//!
//! 路由和 handler 不在这里——它们在 `crate::http`，进程内可直调。这里剩下的是
//! 只有真跑起来才有意义的那一半：配置迁移、后台探测线程、SIGTERM、临时文件清扫。

use std::io;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::fs;

use chrono::Utc;
use db_qbs_shared::{LogEvent, LogLevel};
use serde_json::json;
use signal_hook::consts::SIGTERM;
use tiny_http::Server;

use crate::http::{emit, Api, AgentRegistry, Request, RunState, RUN_TASKS_DIRECTORY};
use crate::{
    fetch_agent_info, AgentStore, DatasourceStore, HistoryStore, SourceConfig, TaskStore,
    UnknownReason,
};

pub fn serve(config: SourceConfig, config_path: PathBuf) -> Result<(), String> {
    if config.listen.is_empty() {
        return Err("source 配置 listen 不能为空".to_owned());
    }

    let task_store = TaskStore::open(&config.data_dir)?;
    let datasource_store = DatasourceStore::open(&config.data_dir)?;
    migrate_legacy_oracle_datasource(&config, &datasource_store)?;
    let agent_store: AgentRegistry = Arc::new(Mutex::new(AgentStore::open(&config.data_dir)?));
    migrate_legacy_sink_base_url(&config, &agent_store, &datasource_store)?;
    let history_store = HistoryStore::open(&config.data_dir)?;
    history_store.seal_incomplete(
        UnknownReason::ProcessDisappeared,
        Utc::now(),
        config.history_retention_days,
    )?;
    clean_run_tasks(&config.data_dir)?;
    let runs = Arc::new(Mutex::new(RunState::default()));
    let server = Server::http(&config.listen)
        .map_err(|error| format!("监听 {} 失败：{error}", config.listen))?;
    emit(
        LogLevel::Info,
        LogEvent::SourceStarted,
        json!({
            "listen": config.listen,
            "message": "source 长驻编排进程已启动",
        }),
    );
    if !is_loopback(&config.listen) {
        emit(
            LogLevel::Warn,
            LogEvent::SourceStarted,
            json!({
                "listen": config.listen,
                "message": format!(
                    "本服务无鉴权；能连上者等价于持有**全部已配置数据源**的凭据与写权限：可对任一源库跑任意 SQL 并清空重写任一目标表；运行历史含源库真实业务值；当前监听地址：{}",
                    config.listen
                ),
            }),
        );
    }

    let terminated = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGTERM, Arc::clone(&terminated))
        .map_err(|error| format!("注册 SIGTERM 处理失败：{error}"))?;
    spawn_agent_probe_loop(Arc::clone(&agent_store), Arc::clone(&terminated));
    let state = Api {
        config: &config,
        config_path: &config_path,
        tasks: &task_store,
        datasources: &datasource_store,
        agents: &agent_store,
        history: &history_store,
        runs: &runs,
    };

    while !terminated.load(Ordering::Relaxed) {
        if let Some(request) = server
            .recv_timeout(Duration::from_millis(100))
            .map_err(|error| format!("接收 HTTP 请求失败：{error}"))?
        {
            handle_request(request, &state);
        }
    }
    history_store.seal_incomplete(
        UnknownReason::ServiceRestarted,
        Utc::now(),
        config.history_retention_days,
    )?;
    clean_run_tasks(&config.data_dir)?;
    Ok(())
}

/// `source.toml` 里那三个已退役字段的一次性迁移（ADR-0037 §10）。
fn migrate_legacy_oracle_datasource(
    config: &SourceConfig,
    datasources: &DatasourceStore,
) -> Result<(), String> {
    let migrated = datasources.migrate_legacy_oracle(
        config.oracle_connect_string.as_deref(),
        config.oracle_username.as_deref(),
        config.oracle_password.as_deref(),
    )?;
    if let Some(datasource_id) = migrated {
        emit(
            LogLevel::Warn,
            LogEvent::SourceStarted,
            json!({
                "datasource_id": datasource_id,
                "message": "source.toml 的 oracle_connect_string / oracle_username / oracle_password 已退役（ADR-0037 §10），本次已迁成一条名为「默认」的 Oracle 数据源；请从配置文件里删掉这三个字段。oracle_client_lib_dir 不退役",
            }),
        );
    }
    Ok(())
}

/// `source.toml` 里 `sink_base_url` 的一次性迁移（ADR-0044 §5）。
///
/// 迁出来的那台 agent 状态是「未探测」，并把**当时还没绑定 agent 的 MySQL 数据源**
/// 全部指到它——现存部署因此在升级后第一次启动就能照常跑，不需要人先去注册一遍。
/// 之后这条路径永远不再触发（判据是 agent 表为空）。
fn migrate_legacy_sink_base_url(
    config: &SourceConfig,
    agents: &AgentRegistry,
    datasources: &DatasourceStore,
) -> Result<(), String> {
    let migrated = agents
        .lock()
        .map_err(|_| "agent 注册表锁已损坏".to_owned())?
        .migrate_legacy_sink_base_url(config.sink_base_url.as_deref())?;
    let Some(agent_id) = migrated else {
        return Ok(());
    };
    let patched = datasources.backfill_missing_agent_id(&agent_id)?;
    emit(
        LogLevel::Warn,
        LogEvent::SourceStarted,
        json!({
            "agent_id": agent_id,
            "datasources_patched": patched,
            "message": "source.toml 的 sink_base_url 已退役（ADR-0044 §5），本次已迁成一条名为「默认」的目标端 agent，并把还没绑定 agent 的 MySQL 数据源指向它；请从配置文件里删掉这个字段，并在「目标端 Agent」屏确认它在线",
        }),
    );
    Ok(())
}

/// 后台探测：每 15 秒把注册表里每台 agent 打一遍 `/v1/agent/info`（ADR-0044 §3）。
///
/// **它是「停了 agent，界面上就看得见」的那一半**。另一半是每次真要用到 agent 时的当场核对
/// （测连、元数据、发起运行）——只靠后台探测会有最长 15 秒的窗口期，只靠当场核对则
/// 列表上永远显示着上一次的旧状态。两个都要。
fn spawn_agent_probe_loop(agents: AgentRegistry, terminated: Arc<AtomicBool>) {
    const PROBE_INTERVAL: Duration = Duration::from_secs(15);
    thread::spawn(move || {
        while !terminated.load(Ordering::Relaxed) {
            let registered = match agents.lock() {
                Ok(store) => store.list().unwrap_or_default(),
                Err(_) => return,
            };
            for agent in registered {
                if terminated.load(Ordering::Relaxed) {
                    return;
                }
                // 探测本身在锁外做：一台掉线的 agent 要等满连接超时，
                // 攥着锁等于让整个界面陪它一起卡住。
                let probed = fetch_agent_info(&agent.base_url);
                let Ok(store) = agents.lock() else {
                    return;
                };
                let _ = store.record_probe(
                    &agent.agent_id,
                    &probed,
                    &Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                );
            }
            // 睡成小段，好让 SIGTERM 之后进程能及时退出。
            for _ in 0..PROBE_INTERVAL.as_secs() {
                if terminated.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    });
}

fn clean_run_tasks(data_dir: &Path) -> Result<(), String> {
    let directory = data_dir.join(RUN_TASKS_DIRECTORY);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("读取临时任务目录失败：{error}")),
    };
    for entry in entries {
        let path = entry
            .map_err(|error| format!("读取临时任务文件失败：{error}"))?
            .path();
        if path.is_file() {
            fs::remove_file(&path)
                .map_err(|error| format!("清扫临时任务文件 {} 失败：{error}", path.display()))?;
        }
    }
    Ok(())
}

fn is_loopback(listen: &str) -> bool {
    let Ok(mut addresses) = listen.to_socket_addrs() else {
        return false;
    };
    let Some(first) = addresses.next() else {
        return false;
    };
    first.ip().is_loopback() && addresses.all(|address| address.ip().is_loopback())
}

/// 一个 tiny_http 请求的全程：翻译进来、交给 `Api::handle`、翻译回去。
///
/// 服务里**唯一**一处还认识 `tiny_http` 的地方；判断怎么回，全在 `Api::handle` 里。
fn handle_request(mut request: tiny_http::Request, state: &Api<'_>) {
    let parsed = Request::from_tiny_http(&mut request);
    let response = state.handle(&parsed).into_tiny_http();

    if let Err(error) = request.respond(response) {
        emit(
            LogLevel::Error,
            LogEvent::HttpResponseFailed,
            json!({ "message": format!("HTTP 响应写入失败：{error}") }),
        );
    }
}

/// 进程入口的全部业务：认参数、读配置、开跑。二进制那边只剩把 `Result` 翻成退出码。
pub fn run(args: impl Iterator<Item = String>) -> Result<(), String> {
    let config_path = parse_config_path(args)?;
    let config = crate::load_source_config(Path::new(&config_path)).map_err(|e| e.to_string())?;
    serve(config, PathBuf::from(config_path))
}

fn parse_config_path(mut args: impl Iterator<Item = String>) -> Result<String, String> {
    match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--config"), Some(path), None) => Ok(path),
        _ => Err("用法：db-qbs-source --config <source.toml>".to_owned()),
    }
}

/// 起不来时的那一行日志。二进制够不到 `emit`（它是 crate 内的），所以出口在这里。
pub fn report_startup_failure(message: &str) {
    emit(
        LogLevel::Error,
        LogEvent::SourceConfigFailed,
        json!({ "message": message }),
    );
}
