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

use crate::http::{
    emit, reclaim_after_restart, stop_runs_for_shutdown, AgentRegistry, Api, Request, RunState,
    RUN_TASKS_DIRECTORY,
};
use crate::scheduler::scheduler_loop;
use crate::{
    fetch_agent_info, AgentStore, AuthStore, DatasourceStore, HistoryStore, RunLogStore,
    ScheduleRegistry, ScheduleState, SourceConfig, TaskStore, UnknownReason,
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
    // 原始日志行与运行历史同库同表空间、同一份 0600。它自己管自己的保留期
    // （7 天与每任务 10 次运行两者取严），比历史那 90 天严得多。
    let run_log_store = RunLogStore::open(&config.data_dir)?;
    // 会话与口令跟任务、数据源同一个库、同一份 0600。**开在监听之前**：
    // 端口一开就得有一道门，不能有一个「表还没建好、于是先放行」的窗口。
    let auth_store = AuthStore::open(&config.data_dir)?;
    let runs = Arc::new(Mutex::new(RunState::default()));
    // 上一条命的收尾（#272）：没走完的那几行封口，它们留在目标端的目标表占用记上账、
    // 后台补发 abort。**在开门之前**做完记账那一半，那几张表因此从第一秒起就拦得住
    // 新运行——子进程随父进程一起没了，那一刀 abort 谁也没替它砍。
    reclaim_after_restart(
        &history_store,
        &run_log_store,
        &runs,
        config.history_retention_days,
    )?;
    clean_run_tasks(&config.data_dir)?;
    // 调度器的状态开在这里、由 `Api` 借着：写它的是那条调度线程，读它的是 HTTP 面
    // （`GET /api/schedule`）——排队中的任务因此在界面上看得见（#266）。
    let schedule: ScheduleRegistry = Arc::new(Mutex::new(ScheduleState::default()));
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
    // 非 loopback 监听那条警告**没有作废，只是改了措辞**：门装上了，但它只装在
    // source 这一面。sink 那半边照旧无鉴权且握着目标库的 `DELETE`；出厂口令若还没改，
    // 这道门离「没有门」也只差两次输入。这条只落日志，界面上一个字都不提（所有者裁定）。
    if !is_loopback(&config.listen) {
        let default_password = auth_store.uses_default_password()?;
        emit(
            LogLevel::Warn,
            LogEvent::SourceStarted,
            json!({
                "listen": config.listen,
                "default_password": default_password,
                "message": format!(
                    "source 的 /api/* 已要求登录。{}此外**目标端 sink 仍然无鉴权**：能连上 sink 端口的人可绕过 source，直接清空重写任一目标表。运行历史含源库真实业务值；当前监听地址：{}",
                    if default_password {
                        // 出厂口令还在，这道门离「没有门」只差两次输入，所以
                        // ADR-0037 §5 ③ 那句暴露面照旧成立，一个量词都不减。
                        "但账号 admin 仍在使用**出厂口令**，能连上者试两次即可持有**全部已配置数据源**的凭据与写权限：可对任一源库跑任意 SQL 并清空重写任一目标表。"
                    } else {
                        ""
                    },
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
        run_logs: &run_log_store,
        runs: &runs,
        schedule: &schedule,
        auth: &auth_store,
        describe_source: crate::OracleRowSource::describe,
    };

    // 多线程 accept：HTTP_WORKER_THREADS 条工作线程**共用同一个监听器**（#255）。
    //
    // 在这之前只有一条线程，一个请求全程处理完才回头 poll 下一个。而建任务那条路上
    // 全是同步阻塞：取列信息 / 十行预览最长 15 秒（`PREVIEW_CALL_TIMEOUT`）、
    // agent 探测 5 秒、发往 sink 的 `ureq` 读超时 30 秒。一次慢查询期间整个界面
    // 连任务列表都刷不出来。
    //
    // 用 `thread::scope` 而不是 `thread::spawn`：`Api` 借着栈上那几个 store，
    // 作用域线程让借用照旧成立，不必把每一份状态都塞进 `Arc`。
    //
    // 线程数取固定值，不按核数算：这里等的是**阻塞 IO**，不是 CPU，核数与它无关。
    // 也不选「每请求一线程」——那对一个能被反复戳的端口等于没有上限。
    // 8 条的含义是「同时能有 7 个慢取数在飞，第 8 个人照样刷得出任务列表」。
    // 调度线程与 HTTP 工作线程同一个作用域（#266）：它借的是同一份 `Api`，
    // 走的是和「立即运行」完全同一条派发路径。**它不参与 `workers` 的结局收集**——
    // 它不返回 `Result`，一轮出错只落日志、下一轮照跑；而 `terminated` 一置位
    // 它最多再睡一秒就退出，SIGTERM 的优雅退出时限一个毫秒都没变。
    let workers: Vec<_> = thread::scope(|scope| {
        scope.spawn(|| scheduler_loop(&state, &schedule, &terminated));
        let handles: Vec<_> = (0..HTTP_WORKER_THREADS)
            .map(|_| scope.spawn(|| accept_loop(&server, &state, &terminated)))
            .collect();
        handles
            .into_iter()
            // 一条工作线程 panic 了，剩下几条照常服务——但退出时得说出来，
            // 否则「服务半死不活」会以退出码 0 收场。
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err("HTTP 工作线程 panic 退出".to_owned()))
            })
            .collect()
    });
    // 收摊排在**收工作线程的结局之前**（#272）：在飞运行的目标表占用挂在目标端，
    // 而 abort 只有本进程发得出去——一走了之，那几张表就永远开不了第二次运行。
    // 一条工作线程 panic 过就把这一趟跳过去，等于让一次半死的服务多留一份泄漏。
    stop_runs_for_shutdown(&runs, SHUTDOWN_GRACE);
    for outcome in workers {
        outcome?;
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

/// 共用监听器的工作线程数。见 `serve` 里那段说明。
const HTTP_WORKER_THREADS: usize = 8;

/// 停服时等在飞运行收尾的上限（#272）。
///
/// 等的是「子进程死透 + 替它发一次 abort」：前者是一次信号，后者是一趟发往目标端的
/// HTTP，`ureq` 那边的读超时是 30 秒。取 40 秒是给一次这样的往返留够余量，
/// 又不至于让一次重启在系统的停服时限（systemd 默认 90 秒）里被硬杀。
/// 等不到就走人：占用会留在目标端，下一条命起来时由 [`reclaim_after_restart`] 接手。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(40);

/// 一条工作线程的一辈子：取一个请求、处理完、回头再取。
///
/// `recv_timeout` 在多条线程上同时调是 tiny_http 支持的（`Server: Send + Sync`，
/// 内部自带队列）。仍然保留 100 毫秒的超时轮询而不是无限阻塞的 `recv()`：
/// SIGTERM 之后每条线程最多再等 100 毫秒就自己走人，优雅退出的时限没有变。
fn accept_loop(server: &Server, state: &Api<'_>, terminated: &AtomicBool) -> Result<(), String> {
    while !terminated.load(Ordering::Relaxed) {
        if let Some(request) = server
            .recv_timeout(Duration::from_millis(100))
            .map_err(|error| format!("接收 HTTP 请求失败：{error}"))?
        {
            handle_request(request, state);
        }
    }
    Ok(())
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

/// 进程入口的全部业务：认参数、读配置、开跑或改口令。二进制那边只剩把 `Result` 翻成退出码。
pub fn run(args: impl Iterator<Item = String>) -> Result<(), String> {
    let command = parse_command(args)?;
    let config = crate::load_source_config(Path::new(command.config_path())).map_err(|e| e.to_string())?;
    match command {
        Command::Serve { config_path } => serve(config, PathBuf::from(config_path)),
        Command::ResetPassword { .. } => reset_password(&config),
    }
}

/// 忘了口令的**唯一**出路（所有者裁定）：在 source 主机上跑一次。
///
/// 它把口令送回出厂值并清空**所有**会话。权限等价是诚实的：能在这台主机上执行命令的人，
/// 本来就读得到 `data_dir`、拿得到数据源密钥——这条命令没有多给他任何东西。
///
/// **它要服务停着跑**吗？不必。SQLite 自己扛并发写；但正在跑的进程内没有缓存，
/// 下一个请求就会认新口令、并发现自己的票据已经没了。
fn reset_password(config: &SourceConfig) -> Result<(), String> {
    AuthStore::open(&config.data_dir)?.reset_password()?;
    emit(
        LogLevel::Warn,
        LogEvent::SourceStarted,
        json!({
            "message": "口令已重置为出厂值（admin / admin），所有会话已失效",
        }),
    );
    Ok(())
}

enum Command {
    Serve { config_path: String },
    ResetPassword { config_path: String },
}

impl Command {
    fn config_path(&self) -> &str {
        match self {
            Self::Serve { config_path } | Self::ResetPassword { config_path } => config_path,
        }
    }
}

const USAGE: &str =
    "用法：db-qbs-source --config <source.toml>
      db-qbs-source reset-password --config <source.toml>";

fn parse_command(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    match (args.next().as_deref(), args.next(), args.next(), args.next()) {
        (Some("--config"), Some(path), None, None) => Ok(Command::Serve { config_path: path }),
        (Some("reset-password"), Some(flag), Some(path), None) if flag == "--config" => {
            Ok(Command::ResetPassword { config_path: path })
        }
        _ => Err(USAGE.to_owned()),
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
