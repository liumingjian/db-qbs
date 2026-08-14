use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use db_qbs_source::{load_source_config, load_task_config, parse_biz_date};

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
        &format!("{}granularity = \"DAY\"\n", valid_task()),
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
    let malformed_task = write(&directory, "broken-task.toml", "source_sql = [\n");

    let source_error = load_source_config(&missing_source).unwrap_err().to_string();
    let task_error = load_task_config(&malformed_task).unwrap_err().to_string();

    assert!(source_error.contains("source config file"));
    assert!(source_error.contains("missing-source.toml"));
    assert!(task_error.contains("task file"));
    assert!(task_error.contains("broken-task.toml"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn business_date_is_an_exact_timezone_free_calendar_day() {
    assert!(parse_biz_date("2024-02-29").is_ok());
    for invalid in [
        "2023-02-29",
        "0000-01-01",
        "2026-8-14",
        "2026-08-14Z",
        "2026-08-14T00:00:00+08:00",
    ] {
        assert!(parse_biz_date(invalid).is_err(), "accepted {invalid}");
    }
}

fn valid_source() -> &'static str {
    "oracle_connect_string = \"//oracle:1521/XE\"\n\
     oracle_username = \"source\"\n\
     oracle_password = \"secret\"\n\
     oracle_client_lib_dir = \"/opt/oracle\"\n\
     sink_base_url = \"http://sink:8080\"\n"
}

fn valid_task() -> &'static str {
    "source_sql = \"SELECT id FROM orders\"\n\
     source_date_col = \"BIZ_DAY\"\n\
     target_table = \"ORDERS\"\n\
     target_date_col = \"BIZ_DAY\"\n"
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
