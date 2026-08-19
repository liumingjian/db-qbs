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
    // 映射逐条读进来：目标字段是规格里的一等字段，不是从源列名推出来的（ADR-0038 §2）。
    assert_eq!(
        loaded
            .spec
            .columns
            .iter()
            .map(|mapping| (mapping.source.as_str(), mapping.target.as_str()))
            .collect::<Vec<_>>(),
        vec![("ID", "ID"), ("D_BIZ", "D_BIZ")]
    );
    assert_eq!(loaded.run_params["d_biz"], "2026-08-14");
    // 两端连接由编排进程解好写进来（ADR-0037 §1/§8）——子进程不碰数据源库、也不碰密钥。
    assert_eq!(loaded.oracle.username, "source");
    assert_eq!(loaded.target.database, "qbs");
    // SQL 由规格现算（ADR-0036 §2）——两端算的是同一份，不存在「存下来的那份对不上」。
    assert!(loaded.source_sql().contains("TO_DATE(:d_biz,'YYYY-MM-DD')"));
    assert_eq!(
        loaded.bindings().unwrap(),
        vec![("d_biz".to_owned(), "2026-08-14".to_owned())]
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn column_precision_is_not_a_task_definition_field_any_more() {
    // 它随取列请求走、用完即弃（ADR-0036 §6）。任务文件里再出现就是配置错，要点名。
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
    // 临时任务定义是父进程写、子进程读的。`columns` / `conditions` / `order_by` 是
    // array-of-tables，必须排在所有标量之后，否则 `toml::to_string` 直接失败——这条用例就是那道闸。
    // `columns` 自 ADR-0038 §2 换成 `ColumnMapping` 之后也归这一类，所以它在结构体里排到了
    // `primary_key` 之后；把它挪回去这条用例就会红。
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
     primary_key = [\"ID\"]\n\
     \n\
     [[spec.conditions]]\n\
     column = \"D_BIZ\"\n\
     operator = \"eq\"\n\
     value_type = \"date\"\n\
     parameter = \"d_biz\"\n\
     value_source = \"runtime\"\n\
     constant = \"\"\n\
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
     [run_params]\n\
     d_biz = \"2026-08-14\"\n"
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
