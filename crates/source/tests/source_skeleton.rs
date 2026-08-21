use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// 任务定义在线上恰好三样：名字、结构化规格、身份。SQL 不在里面（ADR-0036 §2）。
const TASK_FIELDS: [&str; 5] = [
    "name",
    "source_datasource_id",
    "spec",
    "target_datasource_id",
    "task_id",
];

/// 建任务前先备好两端数据源（ADR-0037 §8）。Oracle 那条由 `source.toml` 的退役字段
/// 首启迁移出来（§10）；MySQL 那条这里现建——它不必连得上，本文件不跑真 MySQL。
fn seed_datasources(port: u16) -> (String, String) {
    let listed = json_body(&get(port, "/api/datasources").unwrap());
    let source_id = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|datasource| datasource["kind"] == "oracle")
        .expect("首启迁移必须留下一条 Oracle 数据源")["datasource_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let created = post(
        port,
        "/api/datasources",
        r#"{"name":"目标库","kind":"mysql","host":"127.0.0.1","port":3306,"username":"sink","password":"change-me","database":"qbs"}"#,
    )
    .unwrap();
    assert_eq!(created.status, 201, "{}", created.body);
    let target_id = json_body(&created)["datasource_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (source_id, target_id)
}

/// 一份任务定义的请求体。规格是唯一真相源，条件带一个「运行时填」的日期参数 `d_biz`。
fn task_json(name: &str, target_table: &str, datasources: &(String, String)) -> String {
    let (source_datasource_id, target_datasource_id) = datasources;
    format!(
        r#"{{"name":"{name}","source_datasource_id":"{source_datasource_id}","target_datasource_id":"{target_datasource_id}","spec":{{"owner":"APP","table":"HOLDINGS","target_table":"{target_table}","columns":[{{"source":"ID","target":"ID"}},{{"source":"D_BIZ","target":"D_BIZ"}}],"primary_key":["ID"],"conditions":[{{"column":"D_BIZ","operator":"eq","value_type":"date","parameter":"d_biz","value_source":"runtime","constant":""}}],"order_by":[]}}}}"#
    )
}

/// 上面那份规格现算出来的源端 SQL。父子两端算的是同一份，历史里钉的也是它。
const EXPECTED_SOURCE_SQL: &str = "SELECT a.ID AS ID,\n       a.D_BIZ AS D_BIZ\n  FROM APP.HOLDINGS a\n WHERE a.D_BIZ = TO_DATE(:d_biz,'YYYY-MM-DD')";

#[test]
fn task_crud_persists_stable_identity_without_exposing_credentials() {
    let directory = temp_directory();
    let (port, config, child, _ready) =
        start_source_ready(|port| write_config(&directory, &format!("127.0.0.1:{port}")));
    let datasources = seed_datasources(port);

    let created = request(
        port,
        "POST",
        "/api/tasks",
        Some(&task_json("持仓明细", "HOLDINGS", &datasources)),
    )
    .unwrap();
    assert_eq!(created.status, 201, "{}", created.body);
    let created = json_body(&created);
    assert_task_fields(&created);
    let task_id = created["task_id"].as_str().unwrap().to_owned();
    assert!(!task_id.is_empty());

    let listed = get(port, "/api/tasks").unwrap();
    assert_eq!(listed.status, 200);
    assert_eq!(json_body(&listed), serde_json::json!([created]));

    let detail = get(port, &format!("/api/tasks/{task_id}")).unwrap();
    assert_eq!(detail.status, 200);
    assert_eq!(json_body(&detail), created);

    let updated = request(
        port,
        "PUT",
        &format!("/api/tasks/{task_id}"),
        Some(&task_json("持仓日明细", "HOLDINGS_DAILY", &datasources)),
    )
    .unwrap();
    assert_eq!(updated.status, 200, "{}", updated.body);
    let updated = json_body(&updated);
    assert_task_fields(&updated);
    assert_eq!(updated["task_id"], task_id);
    assert_eq!(updated["name"], "持仓日明细");
    assert_eq!(updated["spec"]["target_table"], "HOLDINGS_DAILY");

    let first_output = terminate(child);
    assert_success(&first_output);
    let database = directory.join("db-qbs.sqlite3");
    assert_eq!(
        fs::metadata(&database).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let mut restarted = start_source(&config);
    let listed = wait_for_tasks(port, &mut restarted);
    assert_eq!(listed.status, 200);
    assert_eq!(json_body(&listed), serde_json::json!([updated]));

    let no_config_endpoint = get(port, "/api/config").unwrap();
    assert_eq!(no_config_endpoint.status, 404);
    for response in [&listed, &no_config_endpoint] {
        assert!(!response.body.contains("secret"));
        assert!(!response.body.contains("oracle_password"));
    }

    let deleted = request(port, "DELETE", &format!("/api/tasks/{task_id}"), None).unwrap();
    assert_eq!(deleted.status, 200, "{}", deleted.body);
    assert_eq!(json_body(&deleted), updated);
    assert_eq!(
        get(port, &format!("/api/tasks/{task_id}")).unwrap().status,
        404
    );
    assert_eq!(
        json_body(&get(port, "/api/tasks").unwrap()),
        serde_json::json!([])
    );

    assert_success(&terminate(restarted));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn task_writes_reject_client_identity_and_incomplete_definitions() {
    let directory = temp_directory();
    let (port, _config, child, _ready) =
        start_source_ready(|port| write_config(&directory, &format!("127.0.0.1:{port}")));
    let datasources = seed_datasources(port);

    let client_identity = request(
        port,
        "POST",
        "/api/tasks",
        Some(&format!(
            r#"{{"task_id":"chosen-by-client",{}"#,
            &task_json("持仓明细", "HOLDINGS", &datasources)[1..]
        )),
    )
    .unwrap();
    assert_eq!(client_identity.status, 400, "{}", client_identity.body);

    let missing_name = request(
        port,
        "POST",
        "/api/tasks",
        Some(
            r#"{"spec":{"owner":"APP","table":"HOLDINGS","target_table":"HOLDINGS","columns":[{"source":"ID","target":"ID"}],"primary_key":["ID"],"conditions":[],"order_by":[]}}"#,
        ),
    )
    .unwrap();
    assert_eq!(missing_name.status, 400, "{}", missing_name.body);
    assert_eq!(
        json_body(&get(port, "/api/tasks").unwrap()),
        serde_json::json!([])
    );

    assert_success(&terminate(child));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn run_launch_materializes_task_and_aggregates_child_output_until_exit() {
    let directory = temp_directory();
    let release = directory.join("release-child");
    let invocation = directory.join("child-args");
    let fake_child = write_fake_child(
        &directory,
        &format!(
            r#"printf '%s\n' "$@" > '{}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:00.000Z","level":"info","event":"source_started","run_id":null,"task":null,"message":"started"}}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:01.000Z","level":"info","event":"stage_changed","run_id":"run-7","task":null,"stage":"PREPARING","message":"preparing"}}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:02.000Z","level":"info","event":"run_opened","run_id":"run-7","task":null,"staging_table":"STG_7","columns_checked":2,"message":"opened"}}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:03.000Z","level":"info","event":"stage_changed","run_id":"run-7","task":null,"stage":"STREAMING","message":"streaming"}}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:04.000Z","level":"info","event":"batch_pushed","run_id":"run-7","task":null,"seq":1,"rows":3,"source_rows":3,"bytes":100,"written":3,"ms":10}}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:05.000Z","level":"info","event":"batch_pushed","run_id":"run-7","task":null,"seq":2,"rows":4,"source_rows":7,"bytes":120,"written":4,"ms":12}}'
while [ ! -f '{}' ]; do sleep 0.02; done
printf '%s\n' '{{"ts":"2026-08-15T10:00:06.000Z","level":"info","event":"stage_changed","run_id":"run-7","task":null,"stage":"COMMITTING","message":"committing"}}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:07.000Z","level":"info","event":"run_finished","run_id":"run-7","task":null,"terminal":"SUCCEEDED","stage":"SUCCEEDED","message":"done","source_code":null,"sink_code":null,"column":null,"value":null,"source_rows":7,"source_batches":2,"staged_rows":7,"received_batches":2,"sink_reported_rows":7,"purged_rows":1,"fetch_ms":4,"push_ms":22,"commit_ms":6,"count_ms":2,"cursor_ms":1}}'
"#,
            invocation.display(),
            release.display(),
        ),
    );
    let (port, config, source, _ready) =
        start_source_ready(|port| write_run_config(&directory, port, &fake_child));
    let datasources = seed_datasources(port);
    let created = post(
        port,
        "/api/tasks",
        &task_json("holdings", "HOLDINGS", &datasources),
    )
    .unwrap();
    assert_eq!(created.status, 201, "{}", created.body);
    let task_id = json_body(&created)["task_id"].as_str().unwrap().to_owned();
    let audit = rusqlite::Connection::open(directory.join("db-qbs.sqlite3")).unwrap();
    audit
        .execute_batch(
            "CREATE TABLE history_write_audit (operation TEXT NOT NULL);
             CREATE TRIGGER audit_history_insert AFTER INSERT ON run_history
             BEGIN INSERT INTO history_write_audit VALUES ('INSERT'); END;
             CREATE TRIGGER audit_history_update AFTER UPDATE ON run_history
             BEGIN INSERT INTO history_write_audit VALUES ('UPDATE'); END;",
        )
        .unwrap();
    drop(audit);

    let started = post(
        port,
        "/api/runs",
        &format!(r#"{{"task_id":"{task_id}","run_params":{{"d_biz":"2026-08-14"}}}}"#),
    )
    .unwrap();
    assert_eq!(started.status, 202, "{}", started.body);
    let run_record_id = json_body(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let detail = wait_for_json(port, &format!("/api/runs/{run_record_id}"), |body| {
        body["rows_pushed"] == 7
    });
    assert_eq!(
        detail,
        serde_json::json!({
            "run_record_id": run_record_id,
            "run_id": "run-7",
            "run_params": { "d_biz": "2026-08-14" },
            "source_sql": EXPECTED_SOURCE_SQL,
            "staging_table": "STG_7",
            "stage": "STREAMING",
            "total_rows": null,
            "precount_ms": null,
            "seq": 2,
            "rows_pushed": 7,
            "bytes": 220,
            "ms": 22,
            "last_ts": "2026-08-15T10:00:05.000Z",
            "live": true,
        })
    );
    let live_list = json_body(&get(port, &format!("/api/runs?task_id={task_id}")).unwrap());
    assert_eq!(live_list.as_array().unwrap().len(), 1);
    assert_eq!(live_list[0]["run_record_id"], run_record_id);
    assert_eq!(live_list[0]["rows_pushed"], 7);
    assert_eq!(live_list[0]["finished_at"], Value::Null);

    let task_files = directory_entries(&directory.join("run-tasks"));
    assert_eq!(task_files.len(), 1);
    assert!(task_files[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains(&run_record_id));
    assert_eq!(
        fs::metadata(&task_files[0]).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let task_toml = fs::read_to_string(&task_files[0]).unwrap();
    for field in [
        "owner",
        "table",
        "columns",
        "primary_key",
        "conditions",
        "run_params",
    ] {
        assert!(task_toml.contains(field), "{task_toml}");
    }
    // SQL 不落进任务文件（ADR-0036 §2）：子进程从同一份规格现算。
    assert!(!task_toml.contains("SELECT"), "{task_toml}");
    // 任务身份仍不落进去——子进程按规格干活，不需要知道自己是哪条任务。
    for absent in ["holdings", "task_id"] {
        assert!(!task_toml.contains(absent), "{task_toml}");
    }
    // **两端凭据现在落进去了**（ADR-0037 §1/§8，推翻了原来那条「任务文件不含凭据」的断言）：
    // 编排进程解一次，子进程不碰数据源库、也不碰密钥文件。兜底是上面那条 0600
    // 与「启动 / 退出各扫一次 run-tasks」的清扫。
    for present in ["[oracle]", "[target]", "client_lib_dir"] {
        assert!(task_toml.contains(present), "{task_toml}");
    }

    let args = fs::read_to_string(invocation).unwrap();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        [
            "--config",
            config.to_str().unwrap(),
            "--task",
            task_files[0].to_str().unwrap(),
        ]
    );

    fs::write(release, "").unwrap();
    let history = wait_for_json(port, &format!("/api/runs/{run_record_id}"), |body| {
        body["live"] == false
    });
    assert_eq!(history["run_record_id"], run_record_id);
    assert_eq!(history["run_id"], "run-7");
    assert_eq!(history["task_id"], task_id);
    assert_eq!(
        history["run_params"],
        serde_json::json!({ "d_biz": "2026-08-14" })
    );
    // 当次执行的 SQL 快照（ADR-0036 §2）：存的是未绑定的语句文本，参数值不内联。
    assert_eq!(history["source_sql"], EXPECTED_SOURCE_SQL);
    assert_eq!(history["outcome"], "SUCCEEDED");
    assert_eq!(history["target_table_effect"], "SWAPPED");
    assert_eq!(history["source_rows"], 7);
    assert_eq!(history["source_batches"], 2);
    assert_eq!(history["rows_pushed"], 7);
    assert_eq!(history["seq"], 2);
    assert_eq!(history["bytes"], 220);
    assert_eq!(history["ms"], 22);
    assert_eq!(history["source_code"], Value::Null);
    assert_eq!(history["sink_code"], Value::Null);

    let by_task = json_body(&get(port, &format!("/api/runs?task_id={task_id}")).unwrap());
    assert_eq!(by_task.as_array().unwrap().len(), 1);
    assert_eq!(by_task[0]["run_record_id"], run_record_id);
    // 筛选只剩任务这一维：运行参数是任务自定义的名字，筛不出一个跨任务的通用维度。
    let no_match = json_body(&get(port, "/api/runs?task_id=nonexistent").unwrap());
    assert_eq!(no_match, serde_json::json!([]));
    let audit = rusqlite::Connection::open(directory.join("db-qbs.sqlite3")).unwrap();
    let history_writes: u64 = audit
        .query_row("SELECT COUNT(*) FROM history_write_audit", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(history_writes, 5);
    wait_for_empty_directory(&directory.join("run-tasks"));

    assert_success(&terminate(source));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn silent_child_disappearance_evicts_projection_and_removes_task_file() {
    let directory = temp_directory();
    let fake_child = write_fake_child(&directory, "exit 1\n");
    let (port, _config, source, _ready) =
        start_source_ready(|port| write_run_config(&directory, port, &fake_child));
    let task_id = create_task(port);

    let run_record_id = start_run(port, &task_id);
    let history = wait_for_json(port, &format!("/api/runs/{run_record_id}"), |body| {
        body["live"] == false
    });
    assert_eq!(history["outcome"], "FAILED");
    assert_eq!(history["unknown_reason"], "PROCESS_DISAPPEARED");
    assert_eq!(history["message"], "进程消失，无终态日志");
    assert_eq!(history["source_code"], Value::Null);
    assert_eq!(history["sink_code"], Value::Null);
    assert_eq!(history["target_table_effect"], Value::Null);
    wait_for_empty_directory(&directory.join("run-tasks"));

    assert_success(&terminate(source));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn restart_cleanup_distinguishes_graceful_restart_from_process_disappearance() {
    let directory = temp_directory();
    let fake_child = write_fake_child(
        &directory,
        r#"printf '%s\n' '{"ts":"2026-08-15T12:00:00.000Z","level":"info","event":"source_started","run_id":null,"task":null}'
printf '%s\n' '{"ts":"2026-08-15T12:00:01.000Z","level":"info","event":"stage_changed","run_id":"run-restart","task":null,"stage":"PREPARING"}'
sleep 2
"#,
    );
    let (port, config, source, _ready) =
        start_source_ready(|port| write_run_config(&directory, port, &fake_child));
    let task_id = create_task(port);

    let graceful_record_id = start_run(port, &task_id);
    wait_for_json(port, &format!("/api/runs/{graceful_record_id}"), |body| {
        body["stage"] == "PREPARING"
    });
    assert_success(&terminate(source));

    let restarted = restart_source(&config, port);
    let graceful = json_body(&get(port, &format!("/api/runs/{graceful_record_id}")).unwrap());
    assert_eq!(graceful["live"], false);
    assert_eq!(graceful["unknown_reason"], "SERVICE_RESTARTED");
    assert_eq!(graceful["message"], "服务重启，结局未知");
    assert_eq!(graceful["source_code"], Value::Null);
    assert_eq!(graceful["target_table_effect"], Value::Null);

    let disappeared_record_id = start_run(port, &task_id);
    wait_for_json(
        port,
        &format!("/api/runs/{disappeared_record_id}"),
        |body| body["stage"] == "PREPARING",
    );
    let killed = kill_source(restarted, "-KILL");
    assert!(!killed.status.success(), "{}", output_text(&killed));

    let after_kill = restart_source(&config, port);
    let disappeared = json_body(&get(port, &format!("/api/runs/{disappeared_record_id}")).unwrap());
    assert_eq!(disappeared["live"], false);
    assert_eq!(disappeared["unknown_reason"], "PROCESS_DISAPPEARED");
    assert_eq!(disappeared["message"], "进程消失，无终态日志");
    assert_eq!(disappeared["sink_code"], Value::Null);
    assert_eq!(disappeared["target_table_effect"], Value::Null);
    wait_for_empty_directory(&directory.join("run-tasks"));

    assert_success(&terminate(after_kill));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn child_hanging_mid_run_remains_live_and_accepted_is_not_preparing() {
    let directory = temp_directory();
    let emit = directory.join("emit-lines");
    let release = directory.join("release-child");
    let fake_child = write_fake_child(
        &directory,
        &format!(
            r#"while [ ! -f '{}' ]; do sleep 0.02; done
printf '%s\n' '{{"ts":"2026-08-15T11:00:00.000Z","level":"info","event":"source_started","run_id":null,"task":null,"message":"started"}}'
printf '%s\n' '{{"ts":"2026-08-15T11:00:01.000Z","level":"info","event":"stage_changed","run_id":"run-hanging","task":null,"stage":"PREPARING","message":"preparing"}}'
printf '%s\n' '{{"ts":"2026-08-15T11:00:02.000Z","level":"info","event":"batch_pushed","run_id":"run-hanging","task":null,"seq":1,"rows":5,"source_rows":5,"bytes":64,"written":5,"ms":9}}'
while [ ! -f '{}' ]; do sleep 0.02; done
"#,
            emit.display(),
            release.display(),
        ),
    );
    let (port, _config, source, _ready) =
        start_source_ready(|port| write_run_config(&directory, port, &fake_child));
    let task_id = create_task(port);

    let run_record_id = start_run(port, &task_id);
    let accepted = json_body(&get(port, &format!("/api/runs/{run_record_id}")).unwrap());
    assert_eq!(accepted["stage"], Value::Null);
    assert_eq!(accepted["run_id"], Value::Null);
    assert_eq!(
        accepted["run_params"],
        serde_json::json!({ "d_biz": "2026-08-14" })
    );
    assert_eq!(accepted["source_sql"], EXPECTED_SOURCE_SQL);
    assert_eq!(accepted["seq"], 0);
    assert_eq!(accepted["rows_pushed"], 0);
    assert_eq!(accepted["bytes"], 0);
    assert_eq!(accepted["ms"], 0);
    assert_eq!(accepted["last_ts"], Value::Null);
    assert_eq!(accepted["live"], true);

    fs::write(emit, "").unwrap();
    let partial = wait_for_json(port, &format!("/api/runs/{run_record_id}"), |body| {
        body["rows_pushed"] == 5
    });
    assert_eq!(partial["stage"], "PREPARING");
    assert_eq!(partial["run_id"], "run-hanging");
    assert_eq!(
        partial["run_params"],
        serde_json::json!({ "d_biz": "2026-08-14" })
    );
    assert_eq!(partial["seq"], 1);
    assert_eq!(partial["bytes"], 64);
    assert_eq!(partial["ms"], 9);
    assert_eq!(partial["live"], true);

    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        get(port, &format!("/api/runs/{run_record_id}"))
            .unwrap()
            .status,
        200
    );
    fs::write(release, "").unwrap();
    let history = wait_for_json(port, &format!("/api/runs/{run_record_id}"), |body| {
        body["live"] == false
    });
    assert_eq!(history["unknown_reason"], "PROCESS_DISAPPEARED");

    assert_success(&terminate(source));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn run_launch_rejects_only_the_same_task_and_run_parameters_until_child_reap() {
    let directory = temp_directory();
    let release = directory.join("release-children");
    let fake_child = write_fake_child(
        &directory,
        &format!(
            "while [ ! -f '{}' ]; do sleep 0.02; done\nexit 1\n",
            release.display()
        ),
    );
    let (port, _config, source, _ready) =
        start_source_ready(|port| write_run_config(&directory, port, &fake_child));
    let first_task_id = create_task(port);
    let second_task_id = create_task(port);

    let first = start_run_for_date(port, &first_task_id, "2026-08-14");
    let duplicate = post(
        port,
        "/api/runs",
        &format!(r#"{{"task_id":"{first_task_id}","run_params":{{"d_biz":"2026-08-14"}}}}"#),
    )
    .unwrap();
    assert_eq!(duplicate.status, 409, "{}", duplicate.body);
    assert!(json_body(&duplicate)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("已有 run 进行中"));

    let other_date = start_run_for_date(port, &first_task_id, "2026-08-15");
    let other_task = start_run_for_date(port, &second_task_id, "2026-08-14");

    fs::write(release, "").unwrap();
    for run_record_id in [&first, &other_date, &other_task] {
        wait_for_json(port, &format!("/api/runs/{run_record_id}"), |body| {
            body["live"] == false
        });
    }
    wait_for_empty_directory(&directory.join("run-tasks"));

    let relaunched = start_run_for_date(port, &first_task_id, "2026-08-14");
    wait_for_json(port, &format!("/api/runs/{relaunched}"), |body| {
        body["live"] == false
    });

    assert_success(&terminate(source));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn cancel_signals_preparing_and_streaming_but_rejects_committing() {
    let directory = temp_directory();
    let canceled = directory.join("canceled-dates");
    let release_committing = directory.join("release-committing");
    let fake_child = write_fake_child(
        &directory,
        &format!(
            r#"biz_date=$(sed -n 's/^d_biz = "\(.*\)"$/\1/p' "$4")
case "$biz_date" in
  2026-08-14) stage=PREPARING ;;
  2026-08-15) stage=STREAMING ;;
  2026-08-16) stage=COMMITTING ;;
esac
trap 'printf "%s\n" "$biz_date" >> "{}"; exit 0' TERM
printf '%s\n' "{{\"ts\":\"2026-08-15T13:00:00.000Z\",\"level\":\"info\",\"event\":\"stage_changed\",\"run_id\":\"run-$biz_date\",\"task\":null,\"stage\":\"$stage\"}}"
if [ "$stage" = COMMITTING ]; then
  while [ ! -f '{}' ]; do sleep 0.02; done
else
  while :; do sleep 0.02; done
fi
"#,
            canceled.display(),
            release_committing.display(),
        ),
    );
    let (port, _config, source, _ready) =
        start_source_ready(|port| write_run_config(&directory, port, &fake_child));
    let task_id = create_task(port);

    for (biz_date, stage) in [("2026-08-14", "PREPARING"), ("2026-08-15", "STREAMING")] {
        let run_record_id = start_run_for_date(port, &task_id, biz_date);
        wait_for_json(port, &format!("/api/runs/{run_record_id}"), |body| {
            body["stage"] == stage
        });
        let canceled_response =
            post(port, &format!("/api/runs/{run_record_id}/cancel"), "").unwrap();
        assert_eq!(canceled_response.status, 202, "{}", canceled_response.body);
        wait_for_file_text(&canceled, |text| text.lines().any(|line| line == biz_date));
        wait_for_json(port, &format!("/api/runs/{run_record_id}"), |body| {
            body["live"] == false
        });
    }

    let committing = start_run_for_date(port, &task_id, "2026-08-16");
    wait_for_json(port, &format!("/api/runs/{committing}"), |body| {
        body["stage"] == "COMMITTING"
    });
    let rejected_response = post(port, &format!("/api/runs/{committing}/cancel"), "").unwrap();
    assert_eq!(rejected_response.status, 409, "{}", rejected_response.body);
    let rejected_body = json_body(&rejected_response);
    assert_eq!(rejected_body["error"]["message"], "已过封口点，停不了");
    assert!(rejected_body.get("code").is_none());
    assert!(rejected_body["error"].get("code").is_none());
    assert!(!fs::read_to_string(&canceled)
        .unwrap()
        .lines()
        .any(|line| line == "2026-08-16"));

    fs::write(release_committing, "").unwrap();
    wait_for_json(port, &format!("/api/runs/{committing}"), |body| {
        body["live"] == false
    });

    assert_success(&terminate(source));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn tasks_endpoint_is_ready_and_sigterm_allows_same_port_restart() {
    let directory = temp_directory();
    let (port, config, first, response) =
        start_source_ready(|port| write_config(&directory, &format!("127.0.0.1:{port}")));
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "[]");
    assert_eq!(get(port, "/health").unwrap().status, 404);
    let first_output = terminate(first);
    assert_success(&first_output);
    let first_lines = json_lines(&first_output.stdout);
    assert!(first_lines.iter().any(|line| {
        line["level"] == "info"
            && line["event"] == "source_started"
            && line["listen"] == format!("127.0.0.1:{port}")
    }));

    let mut second = start_source(&config);
    let response = wait_for_tasks(port, &mut second);
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "[]");
    let second_output = terminate(second);
    assert_success(&second_output);

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn empty_listen_fails_with_a_readable_reason() {
    let directory = temp_directory();
    let config = write_config(&directory, "");

    let output = start_source(&config).wait_with_output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    let output_text = output_text(&output);
    assert!(output_text.contains("listen"), "{output_text}");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn non_loopback_listen_emits_the_required_warning() {
    let directory = temp_directory();
    let (port, _config, child, response) =
        start_source_ready(|port| write_config(&directory, &format!("0.0.0.0:{port}")));
    assert_eq!(response.status, 200);
    let output = terminate(child);
    let lines = json_lines(&output.stdout);
    let address = format!("0.0.0.0:{port}");
    // 按 `listen` 字段挑，不能按「第一条 warn」挑：退役字段的首启迁移告警（ADR-0037 §10）
    // 排在这条之前，且它不带 `listen`。用告警自己的措辞去挑则是循环论证。
    let warning = lines
        .iter()
        .find(|line| line["level"] == "warn" && line["listen"] == address)
        .unwrap_or_else(|| panic!("没有带 listen={address} 的 warn 行：{lines:?}"));
    let message = warning["message"].as_str().unwrap();
    // 措辞按 ADR-0037 §5 ③ 加码：暴露面从「该 source 的」放大到「全部已配置数据源」，
    // 这句话本身是那条裁定的唯一可观测物，必须钉住，连带两个「任一」的量词。
    for phrase in [
        "无鉴权",
        "全部已配置数据源",
        "任一源库跑任意 SQL",
        "清空重写任一目标表",
        "运行历史含源库真实业务值",
        address.as_str(),
    ] {
        assert!(
            message.contains(phrase),
            "missing {phrase:?} in {message:?}"
        );
    }

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn column_fetch_rejects_an_invalid_spec_before_reaching_oracle() {
    // SQL 形状预检整段取消（ADR-0036 §5）后，取列前的本地闸只剩规格自身的合法性：
    // 标识符白名单、主键落在选中列里这一类。它仍必须在**连 Oracle 之前**判完。
    let directory = temp_directory();
    let (port, _config, child, _ready) =
        start_source_ready(|port| write_config(&directory, &format!("127.0.0.1:{port}")));

    let response = post(
        port,
        "/api/columns",
        r#"{
          "datasource_id":"unused-the-spec-gate-runs-first",
          "spec":{
            "owner":"APP","table":"ORDERS","target_table":"ORDERS",
            "columns":[{"source":"ID","target":"ID"}],"primary_key":["MISSING"],
            "conditions":[],"order_by":[]
          }
        }"#,
    )
    .unwrap();

    assert_eq!(response.status, 400, "{}", response.body);
    let body: Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(body["kind"], "request");
    assert!(body.get("code").is_none());
    assert!(body.get("run_id").is_none());
    assert!(body["message"].as_str().unwrap().contains("MISSING"));

    assert_success(&terminate(child));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn builder_sql_is_derived_from_the_spec_and_never_travels_back() {
    let directory = temp_directory();
    let (port, _config, child, _ready) =
        start_source_ready(|port| write_config(&directory, &format!("127.0.0.1:{port}")));

    let generated = post(
        port,
        "/api/builder/sql",
        r#"{
          "dblink":"FA",
          "owner":"HTBR45",
          "table":"T_R_FR_ASTSTAT",
          "target_table":"T_POSITION",
          "columns":[{"source":"N_VA_PRICE","target":"N_VA_PRICE"},{"source":"D_BIZ","target":"D_BIZ"}],
          "primary_key":["D_BIZ"],
          "conditions":[{"column":"D_BIZ","operator":"eq","value_type":"date","parameter":"d_biz","value_source":"runtime","constant":""}],
          "order_by":[{"column":"D_BIZ","direction":"desc"}]
        }"#,
    )
    .unwrap();

    assert_eq!(generated.status, 200, "{}", generated.body);
    let derived: Value = serde_json::from_str(&generated.body).unwrap();
    // 派生面恰好两样（ADR-0036 §6 的前两样；第三样在报文里）。
    assert_eq!(derived.as_object().unwrap().len(), 2);
    let sql = derived["source_sql"].as_str().unwrap();
    assert!(sql.contains("T_R_FR_ASTSTAT@FA"), "{sql}");
    assert!(sql.contains("TO_DATE(:d_biz,'YYYY-MM-DD')"), "{sql}");
    assert!(sql.ends_with(" ORDER BY a.D_BIZ DESC"), "{sql}");
    assert_eq!(
        derived["run_parameters"],
        serde_json::json!([{ "parameter": "d_biz", "column": "D_BIZ", "value_type": "date" }])
    );

    // 形状预检那个端点整段没了（ADR-0036 §5），不是改了语义。
    let retired = post(port, "/api/sql-shape", &generated.body).unwrap();
    assert_eq!(retired.status, 404);

    let tasks = get(port, "/api/tasks").unwrap();
    assert_eq!(tasks.status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&tasks.body).unwrap(),
        Value::Array(Vec::new())
    );

    assert_success(&terminate(child));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn builder_rejects_an_invalid_dblink_before_connecting_to_oracle() {
    let directory = temp_directory();
    let (port, _config, child, _ready) =
        start_source_ready(|port| write_config(&directory, &format!("127.0.0.1:{port}")));

    let response = post(
        port,
        "/api/builder/tables",
        r#"{"datasource_id":"unused-the-dblink-gate-runs-first","dblink":"FA WHERE 1=1"}"#,
    )
    .unwrap();

    assert_eq!(response.status, 400);
    let body: Value = serde_json::from_str(&response.body).unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("dblink"));

    assert_success(&terminate(child));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn column_fetch_oracle_failure_does_not_create_a_run_touch_sink_or_write_storage() {
    let directory = temp_directory();
    let sink = TcpListener::bind("127.0.0.1:0").unwrap();
    sink.set_nonblocking(true).unwrap();
    let sink_url = format!("http://{}", sink.local_addr().unwrap());
    let (port, _config, child, _ready) = start_source_ready(|port| {
        write_config_with_oracle(
            &directory,
            &format!("127.0.0.1:{port}"),
            &sink_url,
            "/db-qbs-missing-oracle-client",
        )
    });
    // 数据源要真存在：本用例买的是「Oracle 连不上时不留痕」，不是「数据源解不出来」。
    let (source_datasource_id, _) = seed_datasources(port);
    let files_before = directory_entries(&directory);

    let response = post(
        port,
        "/api/columns",
        &format!(
            r#"{{
          "datasource_id":"{source_datasource_id}",
          "spec":{{
            "owner":"APP","table":"MISSING_ORDERS","target_table":"ORDERS",
            "columns":[{{"source":"ID","target":"ID"}},{{"source":"BIZ_DAY","target":"BIZ_DAY"}}],"primary_key":["ID"],
            "conditions":[],"order_by":[]
          }}
        }}"#
        ),
    )
    .unwrap();

    assert_eq!(response.status, 502);
    let body: Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(body["kind"], "oracle");
    assert!(body.get("run_id").is_none());
    assert_eq!(directory_entries(&directory), files_before);
    let sink_error = sink.accept().unwrap_err();
    assert_eq!(sink_error.kind(), std::io::ErrorKind::WouldBlock);

    assert_success(&terminate(child));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn the_target_metadata_proxy_resolves_credentials_and_writes_nothing() {
    // 目标端元数据面（ADR-0038 §3）：source 仍不建 MySQL 连接，只把解出来的凭据转给 sink。
    // 本用例买三件事——凭据由 datasource_id 解、请求确实过线到 sink、**一个字节都不落盘**
    // （结果纯瞬态，ADR-0038 §8）。真查到什么列归台架的 C 系列去证，那要一个活的 MySQL。
    let directory = temp_directory();
    let sink = TcpListener::bind("127.0.0.1:0").unwrap();
    let sink_url = format!("http://{}", sink.local_addr().unwrap());
    drop(sink); // 端口留空：请求发得出去、连不上，回话必须是 502 kind=sink。
    let (port, _config, child, _ready) = start_source_ready(|port| {
        write_config_with_oracle(
            &directory,
            &format!("127.0.0.1:{port}"),
            &sink_url,
            "/db-qbs-missing-oracle-client",
        )
    });
    let (source_datasource_id, target_datasource_id) = seed_datasources(port);
    let files_before = directory_entries(&directory);

    // Oracle 数据源上没有目标端连接——按名字拒，不编一份出来。
    let wrong_kind = post(
        port,
        "/api/target/tables",
        &format!(r#"{{"datasource_id":"{source_datasource_id}"}}"#),
    )
    .unwrap();
    assert_eq!(wrong_kind.status, 400, "{}", wrong_kind.body);

    // 取列面必须点名一张表：不给库清单端点、也不替用户猜表（ADR-0038 §3）。
    let missing_table = post(
        port,
        "/api/target/columns",
        &format!(r#"{{"datasource_id":"{target_datasource_id}"}}"#),
    )
    .unwrap();
    assert_eq!(missing_table.status, 400, "{}", missing_table.body);
    assert!(missing_table.body.contains("target_table"), "{}", missing_table.body);

    for (path, body) in [
        (
            "/api/target/tables",
            format!(r#"{{"datasource_id":"{target_datasource_id}"}}"#),
        ),
        (
            "/api/target/columns",
            format!(r#"{{"datasource_id":"{target_datasource_id}","target_table":"T_POSITION"}}"#),
        ),
    ] {
        let response = post(port, path, &body).unwrap();
        assert_eq!(response.status, 502, "{}", response.body);
        let parsed: Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(parsed["kind"], "sink");
        // 不属于任何 run：回话里没有 run_id（ADR-0038 §3）。
        assert!(parsed.get("run_id").is_none(), "{}", response.body);
    }

    // 不进任务定义、不进 SQLite、不留临时文件——目录里一个新条目都没有。
    assert_eq!(directory_entries(&directory), files_before);

    assert_success(&terminate(child));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn the_draft_test_connection_reads_the_form_values_and_writes_nothing() {
    // 「测通才让存」（ADR-0039 §3）要测的是**还没存进去的那组值**——新建态库里根本没有这条，
    // 按 id 测无从谈起。本用例买三件事：路由没被按 id 那条吃掉、草稿走的是表单里的值、
    // **一个字节都不落盘**（测连不产生数据源、不留 run）。
    let directory = temp_directory();
    let sink = TcpListener::bind("127.0.0.1:0").unwrap();
    let sink_url = format!("http://{}", sink.local_addr().unwrap());
    drop(sink); // 端口留空：请求发得出去、连不上，回话必须是 502 kind=sink。
    let (port, _config, child, _ready) = start_source_ready(|port| {
        write_config_with_oracle(
            &directory,
            &format!("127.0.0.1:{port}"),
            &sink_url,
            "/db-qbs-missing-oracle-client",
        )
    });
    let datasources_before = json_body(&get(port, "/api/datasources").unwrap());
    let files_before = directory_entries(&directory);

    // 路由：`test-connection` 那一截不许被 `resource_id_from_path` 当成数据源 id 吃掉。
    // 若被吃掉，这里回的是 404「数据源不存在」而不是 400「请求体读不出来」。
    let malformed = post(port, "/api/datasources/test-connection", "{}").unwrap();
    assert_eq!(malformed.status, 400, "{}", malformed.body);

    // 字段不全按字段判，不按 id 判——库里有没有这条数据源与它无关。
    let empty_host = post(
        port,
        "/api/datasources/test-connection",
        r#"{"name":"草稿","kind":"mysql","host":"","port":3306,"username":"u","password":"p","database":"dw"}"#,
    )
    .unwrap();
    assert_eq!(empty_host.status, 400, "{}", empty_host.body);
    assert!(empty_host.body.contains("host"), "{}", empty_host.body);

    // 目标端草稿：source 不建 MySQL 连接，测连也走 sink（ADR-0037 §9）。sink 不在 → 502。
    let mysql_draft = post(
        port,
        "/api/datasources/test-connection",
        r#"{"name":"草稿","kind":"mysql","host":"127.0.0.1","port":3306,"username":"u","password":"p","database":"dw_stage"}"#,
    )
    .unwrap();
    assert_eq!(mysql_draft.status, 502, "{}", mysql_draft.body);
    let parsed: Value = serde_json::from_str(&mysql_draft.body).unwrap();
    assert_eq!(parsed["kind"], "sink");
    // 不属于任何 run：回话里没有 run_id，也没有错误码标签（ADR-0039 §3）。
    assert!(parsed.get("run_id").is_none(), "{}", mysql_draft.body);

    // Oracle 草稿：客户端库路径是假的，连不上——但它同样不该落盘。
    let oracle_draft = post(
        port,
        "/api/datasources/test-connection",
        r#"{"name":"草稿","kind":"oracle","connect_string":"//127.0.0.1:1521/NOPE","username":"u","password":"p"}"#,
    )
    .unwrap();
    assert_ne!(oracle_draft.status, 200, "{}", oracle_draft.body);

    // 三次测连之后：数据源清单逐字未变、目录里一个新条目都没有。
    assert_eq!(
        json_body(&get(port, "/api/datasources").unwrap()),
        datasources_before
    );
    assert_eq!(directory_entries(&directory), files_before);

    assert_success(&terminate(child));
    fs::remove_dir_all(directory).unwrap();
}

fn start_source(config: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_db-qbs-source"))
        .args(["--config"])
        .arg(config)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn terminate(child: Child) -> Output {
    kill_source(child, "-TERM")
}

fn kill_source(child: Child, signal: &str) -> Output {
    let kill_status = Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .unwrap();
    assert!(kill_status.success());
    child.wait_with_output().unwrap()
}

/// 起一个 source 并等它就绪；端口被别人占走就换个号从头再来。
///
/// 端口没法在测试里真正预定：`unused_port()` 把探测用的 listener 交还，到 source 走完启动前的
/// 磁盘活（开两个 SQLite store、seal_incomplete、clean_run_tasks）真正 bind，中间是一段空窗。
/// 空窗里抢号的有三路：同一个测试二进制里并行跑的其它测试发出的 HTTP 请求（`TcpStream::connect`
/// 的本地端口同样从临时端口池里取）、上一轮跑残下来还在监听的 source 进程、以及被 SIGKILL 的
/// source 留下的、还没释放干净的监听套接字。三路的症状是同一句
/// 「监听 … 失败：Address already in use」、退出码 1，不是「5 秒没等到」——所以修法是换号重来，
/// 拉长等待没有用。
fn start_source_ready(
    build_config: impl Fn(u16) -> PathBuf,
) -> (u16, PathBuf, Child, HttpResponse) {
    for attempt in 0..8 {
        let port = unused_port();
        let config = build_config(port);
        let mut child = start_source(&config);
        match try_wait_for_tasks(port, &mut child) {
            Ok(response) => return (port, config, child, response),
            Err(error) if error.contains("Address already in use") => assert!(
                attempt < 7,
                "连续 8 个端口都在 source bind 之前被占走：{error}"
            ),
            Err(error) => panic!("{error}"),
        }
    }
    unreachable!()
}

/// 同端口重启 source。被 SIGKILL 的上一个 source 偶尔会把监听套接字多留一会儿，
/// 这里只等它释放，不换端口——换了端口就测不到「重启后接着读同一份历史」这件事。
fn restart_source(config: &Path, port: u16) -> Child {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut child = start_source(config);
        match try_wait_for_tasks(port, &mut child) {
            Ok(_) => return child,
            Err(error) if error.contains("Address already in use") => assert!(
                Instant::now() < deadline,
                "端口迟迟没释放，同端口重启失败：{error}"
            ),
            Err(error) => panic!("{error}"),
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_tasks(port: u16, child: &mut Child) -> HttpResponse {
    match try_wait_for_tasks(port, child) {
        Ok(response) => response,
        Err(error) => panic!("{error}"),
    }
}

/// source 起来没有；起不来时把它的诊断信息带回去，好让调用方决定换号重试还是直接判失败。
fn try_wait_for_tasks(port: u16, child: &mut Child) -> Result<HttpResponse, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            // source 的失败原因走 emit()，落在 stdout 上，stderr 通常是空的。
            let mut stdout = String::new();
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stdout.take() {
                let _ = pipe.read_to_string(&mut stdout);
            }
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            return Err(format!(
                "source exited before readiness with {status}; stdout={stdout} stderr={stderr}"
            ));
        }
        if let Ok(response) = get(port, "/api/tasks") {
            return Ok(response);
        }
        if Instant::now() >= deadline {
            return Err(format!("source did not become ready on port {port}"));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn get(port: u16, path: &str) -> std::io::Result<HttpResponse> {
    request(port, "GET", path, None)
}

/// 一次请求的三段（连、写、读）各自的上限。
///
/// 取 15 秒的依据：本文件里最慢的一档是 `/api/target/*` 与 `/api/columns`——它们要往 sink 或
/// Oracle 发连接，而这两处在测试里都是打不通的（sink 端口空着、Oracle 客户端库路径是假的），
/// 失败在秒级内就回；其余端点是纯本地 SQLite，亚秒级。15 秒明显大于正常值，又明显小于 CI 的
/// 外层耐心（分钟级），所以卡住时是本进程自己报出「哪条请求超时」，而不是被外层砍掉。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

fn request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> std::io::Result<HttpResponse> {
    let body = body.unwrap_or("");
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    // 三段都要设：被测的 source 在某条路径上不回响应、也不关连接时，无超时的 `read_to_string`
    // 会永久阻塞——挂死的不是这一条用例，是整个测试二进制。
    let mut stream = TcpStream::connect_timeout(&address, REQUEST_TIMEOUT)
        .map_err(|error| annotate(method, path, "连接", error))?;
    stream
        .set_write_timeout(Some(REQUEST_TIMEOUT))
        .and_then(|()| stream.set_read_timeout(Some(REQUEST_TIMEOUT)))
        .map_err(|error| annotate(method, path, "设超时", error))?;
    stream
        .write_all(
            format!(
                "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .map_err(|error| annotate(method, path, "发请求", error))?;
    read_response(&mut stream).map_err(|error| annotate(method, path, "读回话", error))
}

/// 超时（以及任何 I/O 失败）的信息里必须点名是哪条 `method path` 的哪一段——
/// 否则挂死只是换成了一句「某条请求超时」，排障省不下多少。
fn annotate(
    method: &str,
    path: &str,
    stage: &str,
    error: std::io::Error,
) -> std::io::Error {
    std::io::Error::new(
        error.kind(),
        format!("{method} {path} 的{stage}阶段失败（上限 {REQUEST_TIMEOUT:?}）：{error}"),
    )
}

fn post(port: u16, path: &str, body: &str) -> std::io::Result<HttpResponse> {
    request(port, "POST", path, Some(body))
}

fn read_response(stream: &mut TcpStream) -> std::io::Result<HttpResponse> {
    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let status = head
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u16>()
        .unwrap();
    Ok(HttpResponse {
        status,
        body: body.to_owned(),
    })
}

fn json_body(response: &HttpResponse) -> Value {
    serde_json::from_str(&response.body).unwrap()
}

fn assert_task_fields(task: &Value) {
    let mut fields: Vec<_> = task
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(fields, TASK_FIELDS);
}

fn create_task(port: u16) -> String {
    let datasources = seed_datasources(port);
    let response = post(
        port,
        "/api/tasks",
        &task_json("holdings", "HOLDINGS", &datasources),
    )
    .unwrap();
    assert_eq!(response.status, 201, "{}", response.body);
    json_body(&response)["task_id"].as_str().unwrap().to_owned()
}

fn start_run(port: u16, task_id: &str) -> String {
    start_run_for_date(port, task_id, "2026-08-14")
}

/// 发起一次运行。运行参数只有一个 `d_biz`——它是任务自己声明的参数名，不是产品概念。
fn start_run_for_date(port: u16, task_id: &str, biz_date: &str) -> String {
    let response = post(
        port,
        "/api/runs",
        &format!(r#"{{"task_id":"{task_id}","run_params":{{"d_biz":"{biz_date}"}}}}"#),
    )
    .unwrap();
    assert_eq!(response.status, 202, "{}", response.body);
    json_body(&response)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn write_fake_child(directory: &Path, body: &str) -> PathBuf {
    let path = directory.join("fake-source-run.sh");
    fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}")).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

fn write_run_config(directory: &Path, port: u16, run_executable: &Path) -> PathBuf {
    let path = directory.join("source.toml");
    fs::write(
        &path,
        format!(
            "oracle_connect_string = \"//oracle:1521/XE\"\n\
             oracle_username = \"source\"\n\
             oracle_password = \"secret\"\n\
             oracle_client_lib_dir = \"/opt/oracle\"\n\
             sink_base_url = \"http://127.0.0.1:18080\"\n\
             listen = \"127.0.0.1:{port}\"\n\
             data_dir = \"{}\"\n\
             run_executable = \"{}\"\n",
            directory.display(),
            run_executable.display(),
        ),
    )
    .unwrap();
    path
}

fn wait_for_json(port: u16, path: &str, predicate: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = get(port, path).unwrap();
        if response.status == 200 {
            let body = json_body(&response);
            if predicate(&body) {
                return body;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for {path}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_empty_directory(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fs::read_dir(path).unwrap().next().is_none() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for cleanup");
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_file_text(path: &Path, predicate: impl Fn(&str) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(text) = fs::read_to_string(path) {
            if predicate(&text) {
                return;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for {path:?}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn write_config(directory: &Path, listen: &str) -> PathBuf {
    write_config_with_oracle(directory, listen, "http://127.0.0.1:18080", "/opt/oracle")
}

fn write_config_with_oracle(
    directory: &Path,
    listen: &str,
    sink_base_url: &str,
    oracle_client_lib_dir: &str,
) -> PathBuf {
    let path = directory.join("source.toml");
    fs::write(
        &path,
        format!(
            "oracle_connect_string = \"//oracle:1521/XE\"\n\
             oracle_username = \"source\"\n\
             oracle_password = \"secret\"\n\
             oracle_client_lib_dir = \"{oracle_client_lib_dir}\"\n\
             sink_base_url = \"{sink_base_url}\"\n\
             listen = \"{listen}\"\n\
             data_dir = \"{}\"\n",
            directory.display(),
        ),
    )
    .unwrap();
    path
}

fn directory_entries(directory: &Path) -> Vec<PathBuf> {
    let mut entries = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

fn unused_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", output_text(output));
}

fn json_lines(stdout: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn temp_directory() -> PathBuf {
    let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "db-qbs-source-skeleton-test-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}

struct HttpResponse {
    status: u16,
    body: String,
}
