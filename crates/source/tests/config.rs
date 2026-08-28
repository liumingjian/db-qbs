use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use db_qbs_source::{load_source_config, load_task_config, TaskConfig};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[test]
fn unknown_source_and_task_fields_are_named() {
    let directory = temp_directory();
    let source = write(
        &directory,
        "source.toml",
        &format!("{}sink_timeout = 30\n", valid_source()),
    );
    let task = write(
        &directory,
        "task.toml",
        &format!("{}\n[spec.granularity]\nunit = \"DAY\"\n", valid_task()),
    );

    let source_error = load_source_config(&source).unwrap_err().to_string();
    let task_error = load_task_config(&task).unwrap_err().to_string();

    assert!(source_error.contains("sink_timeout"));
    assert!(task_error.contains("granularity"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn missing_and_malformed_files_name_the_file_kind_and_path() {
    let directory = temp_directory();
    let missing_source = directory.join("missing-source.toml");
    let malformed_task = write(&directory, "broken-task.toml", "[spec]\ncolumns = [\n");

    let source_error = load_source_config(&missing_source).unwrap_err().to_string();
    let task_error = load_task_config(&malformed_task).unwrap_err().to_string();

    assert!(source_error.contains("source config file"));
    assert!(source_error.contains("missing-source.toml"));
    assert!(task_error.contains("task file"));
    assert!(task_error.contains("broken-task.toml"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn source_service_settings_require_listen_and_apply_documented_defaults() {
    let directory = temp_directory();
    let missing_listen = write(
        &directory,
        "missing-listen.toml",
        &valid_source().replace("listen = \"127.0.0.1:8088\"\n", ""),
    );
    let configured = write(&directory, "source.toml", valid_source());

    let error = load_source_config(&missing_listen).unwrap_err().to_string();
    assert!(error.contains("listen"), "{error}");

    let config = load_source_config(&configured).unwrap();
    assert_eq!(config.listen, "127.0.0.1:8088");
    assert_eq!(config.data_dir, PathBuf::from("/var/lib/db-qbs-source"));
    assert_eq!(config.history_retention_days, 90);
    assert_eq!(
        config.run_executable.file_name().unwrap(),
        "db-qbs-source-run"
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn the_task_file_carries_a_spec_and_this_run_parameters_and_nothing_else() {
    let directory = temp_directory();
    let path = write(&directory, "task.toml", valid_task());

    let loaded = load_task_config(&path).unwrap();
    assert_eq!(loaded.spec.owner, "APP");
    assert_eq!(loaded.spec.primary_key, vec!["ID".to_owned()]);
    // 写入模式随任务定义落进临时任务文件（#261）——子进程按它走。
    assert_eq!(loaded.spec.write_mode, db_qbs_source::WriteMode::Append);
    // 映射逐条读进来：目标字段是规格里的一等字段，不是从源列名推出来的。
    assert_eq!(
        loaded
            .spec
            .columns
            .iter()
            .map(|mapping| (mapping.source.as_str(), mapping.target.as_str()))
            .collect::<Vec<_>>(),
        vec![("ID", "ID"), ("D_BIZ", "D_BIZ")]
    );
    assert_eq!(
        loaded.spec.where_clause.as_deref(),
        Some("D_BIZ = DATE '2026-08-14'")
    );
    // 两端连接由编排进程解好写进来——子进程不碰数据源库、也不碰密钥。
    assert_eq!(loaded.oracle.username, "source");
    assert_eq!(loaded.target.database, "qbs");
    // 目标端 agent 同样由编排进程解好写进来（ADR-0044 §4）：子进程不读进程级的全局地址，
    // 也因此不存在「任务文件说 A、进程配置说 B」这种两个真相源。
    assert_eq!(loaded.agent.base_url, "http://127.0.0.1:8080");
    assert_eq!(loaded.agent.instance_id, "6f1a9c2d");
    // SQL 由规格现算——两端算的是同一份，不存在「存下来的那份对不上」。
    // 过滤片段原样落在 `WHERE` 后面，不再有第二半（取值）需要对照着读。
    assert!(loaded
        .source_sql()
        .ends_with(" WHERE D_BIZ = DATE '2026-08-14'"));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn column_precision_is_not_a_task_definition_field_any_more() {
    // 它随取列请求走、用完即弃。任务文件里再出现就是配置错，要点名。
    let directory = temp_directory();
    let path = write(
        &directory,
        "task.toml",
        &format!(
            "{}\n[spec.column_precision]\nN_AMT = [20, 4]\n",
            valid_task()
        ),
    );

    let error = load_task_config(&path).unwrap_err().to_string();
    assert!(error.contains("column_precision"), "{error}");

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_materialized_task_file_survives_a_serialize_reload_round_trip() {
    // 临时任务定义是父进程写、子进程读的。`columns` 是 array-of-tables，必须排在所有标量
    // 之后，否则 `toml::to_string` 直接失败——这条用例就是那道闸。`where_clause` 是标量，
    // 所以它在结构体里排在 `columns` 之前；挪到后面这条用例就会红。
    let directory = temp_directory();
    let path = write(&directory, "task.toml", valid_task());
    let loaded = load_task_config(&path).unwrap();

    let serialized = toml::to_string(&loaded).unwrap();
    let round_tripped: TaskConfig = toml::from_str(&serialized).unwrap();
    assert_eq!(round_tripped, loaded);

    fs::remove_dir_all(directory).unwrap();
}

fn valid_source() -> &'static str {
    "oracle_connect_string = \"//oracle:1521/XE\"\n\
     oracle_username = \"source\"\n\
     oracle_password = \"secret\"\n\
     oracle_client_lib_dir = \"/opt/oracle\"\n\
     sink_base_url = \"http://sink:8080\"\n\
     listen = \"127.0.0.1:8088\"\n\
     data_dir = \"/var/lib/db-qbs-source\"\n"
}

fn valid_task() -> &'static str {
    "[spec]\n\
     owner = \"APP\"\n\
     table = \"ORDERS\"\n\
     target_table = \"ORDERS\"\n\
     columns = [\n\
     { source = \"ID\", target = \"ID\" },\n\
     { source = \"D_BIZ\", target = \"D_BIZ\" },\n\
     ]\n\
     write_mode = \"APPEND\"\n\
     primary_key = [\"ID\"]\n\
     where_clause = \"D_BIZ = DATE '2026-08-14'\"\n\
     \n\
     [oracle]\n\
     connect_string = \"//oracle:1521/XE\"\n\
     username = \"source\"\n\
     password = \"secret\"\n\
     client_lib_dir = \"/opt/oracle\"\n\
     \n\
     [target]\n\
     host = \"127.0.0.1\"\n\
     port = 3306\n\
     username = \"sink\"\n\
     password = \"change-me\"\n\
     database = \"qbs\"\n\
     \n\
     [agent]\n\
     agent_id = \"a1\"\n\
     name = \"目标端 A\"\n\
     base_url = \"http://127.0.0.1:8080\"\n\
     instance_id = \"6f1a9c2d\"\n"
}

fn write(directory: &Path, name: &str, contents: &str) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, contents).unwrap();
    path
}

fn temp_directory() -> PathBuf {
    let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "db-qbs-source-config-test-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}
