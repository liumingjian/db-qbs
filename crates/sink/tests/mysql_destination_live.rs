//! The three [`MysqlDestination`] behaviours that only a real MySQL can answer (#196).
//!
//! Everywhere else a test double stands in for this destination, and what the
//! doubles substitute away is exactly the part that talks to MySQL. Three facts
//! were therefore believed only by reading the implementation:
//!
//! 1. `CLIENT_FOUND_ROWS` makes an existing row whose values did not change count
//!    as 1 rather than 0 (#138) — the whole reason `swap_rows_consistent` may put its
//!    lower bound at `staged_rows` instead of 0;
//! 2. the swap's `affected_rows` lands in `[staged, 2×staged]` (ADR-0035 §4), and
//!    **both ends are reachable**, which is why the judgement is an interval;
//! 3. the Connection Ritual hangs off the pool's connection-creation hook, so the
//!    **second** connection the pool opens comes up configured rather than bare.
//!
//! **Why these three and not the rest of that file** is argued once, in the module
//! note at the top of `src/mysql_destination.rs`; this file does not repeat it.
//!
//! **`#[ignore]` rather than a runtime skip**: a test that decides for itself to do
//! nothing still reports `ok`, and "3 passed" for three tests that never opened a
//! socket is the one outcome worth avoiding here. Ignored tests are counted as
//! ignored, so a plain `cargo test` tells the truth on a machine with no docker.
//!
//! ## Two versions, one suite (#262)
//!
//! MySQL 5.7 joined the support matrix as an **addition, not a replacement**, and
//! the two versions disagree on exactly the things this file is here to observe:
//! 5.7 has no `utf8mb4_0900_ai_ci`, its `information_schema.COLUMNS.EXTRA` never
//! says `DEFAULT_GENERATED`, and its stock `max_allowed_packet` is 4 MiB against
//! the ritual's 64 MiB gate. **Nothing here branches on the version**: every test
//! below states the behaviour that must hold on both, and the way to believe both
//! is to run the same suite twice — `run-mysql-destination-live.sh both`.
//!
//! ## The environment it is pointed at
//!
//! All six of `DB_QBS_TEST_MYSQL_HOST` / `_PORT` / `_USER` / `_PASSWORD` /
//! `_DATABASE` / `_ROOT_PASSWORD` are **required**: these tests only run when
//! someone asked for them by name, so a missing one is a broken environment and
//! says so, and defaulting any of them risks running against the wrong database.
//! [`RIG_SCRIPT`] sets all six and is the intended way in.
//!
//! The root password buys exactly one thing — the right to move
//! `max_allowed_packet` *down* for the length of one test and put it back. The
//! migration account has no such privilege, and giving it one to make a test
//! simpler would be the wrong trade.
//!
//! Every table this file creates is prefixed with a per-test unique name and
//! dropped on the way out, so it is safe to point at a database that has other
//! things in it.

use std::env;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use db_qbs_shared::{swap_rows_consistent, WriteMode, WriteStatement};
use db_qbs_sink::{
    build_staging_ddl, check_connection_settings, precheck_with_primary_key, AtomicSwapRequest,
    AtomicSwapResult, Destination, MysqlDestination, SourceColumn, TargetConnection, MIN_PACKET,
};
use mysql::prelude::Queryable;
use mysql::{Conn, Opts, OptsBuilder};

/// What the rig script runs, quoted in the message a missing variable produces.
const RIG_SCRIPT: &str = "docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh";

/// The `CLIENT_FOUND_ROWS` half of #138.
///
/// A re-run that stages exactly what is already in the target table is the case
/// the flag exists for: MySQL's default counts such a row as 0, and a swap that
/// reports 0 against 3 staged rows falls out of `[staged, 2×staged]` and is
/// rejected as `SWAP_FAILED` — a correct run refused. The control assertion at
/// the end is the same upsert over a connection **without** the flag, so the test
/// says which of the two connections is responsible rather than merely that the
/// number came out right.
#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn a_rerun_that_changes_nothing_is_still_counted_row_for_row() {
    let mut rig = Rig::open("found_rows");
    let rows = [("1", "a"), ("2", "b"), ("3", "c")];

    let first = rig.stage_and_swap("r1", &rows).expect("first swap");
    assert_eq!(first.staged_rows, 3);
    assert_eq!(first.swapped_rows, 3, "three fresh rows are three inserts");

    let rerun = rig
        .stage_and_swap("r2", &rows)
        .expect("a re-run that changes nothing must not be refused");
    assert_eq!(rerun.staged_rows, 3);
    assert_eq!(
        rerun.swapped_rows, 3,
        "with CLIENT_FOUND_ROWS an unchanged existing row counts as matched, not as 0"
    );

    let without_the_flag = rig.upsert_over_a_plain_connection(&rows);
    assert_eq!(
        without_the_flag, 0,
        "the control: without CLIENT_FOUND_ROWS MySQL counts an unchanged matched row as 0"
    );
    assert!(
        !swap_rows_consistent(WriteStatement::Upsert, 3, without_the_flag),
        "and 0 against 3 staged rows is outside the interval, which is the spelling of \
         \"remove the flag and it starts refusing correct runs\""
    );
}

/// The interval of ADR-0035 §4, at both of its ends and once in between.
///
/// An equality assertion would be wrong at either end, and the swap only ever
/// reports one number, so the three runs below are the evidence that the interval
/// is the right shape: insert-only sits on the lower bound, an all-values-changed
/// re-run sits on the upper one, and a realistic mixture sits strictly inside.
#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn the_swap_interval_reaches_both_of_its_ends() {
    let mut rig = Rig::open("interval");

    let inserts = rig
        .stage_and_swap("r1", &[("1", "a"), ("2", "b"), ("3", "c")])
        .expect("insert-only swap");
    assert_eq!((inserts.staged_rows, inserts.swapped_rows), (3, 3));

    let updates = rig
        .stage_and_swap("r2", &[("1", "a2"), ("2", "b2"), ("3", "c2")])
        .expect("all-values-changed swap");
    assert_eq!(
        (updates.staged_rows, updates.swapped_rows),
        (3, 6),
        "ON DUPLICATE KEY UPDATE counts a row that really changed as 2"
    );

    let mixed = rig
        .stage_and_swap("r3", &[("1", "a3"), ("2", "b3"), ("4", "d")])
        .expect("mixed swap");
    assert_eq!(
        (mixed.staged_rows, mixed.swapped_rows),
        (3, 5),
        "two changed rows and one new one: 2 + 2 + 1"
    );

    for result in [inserts, updates, mixed] {
        assert!(
            swap_rows_consistent(WriteStatement::Upsert, result.staged_rows, result.swapped_rows),
            "{result:?} must satisfy the judgement both ends share"
        );
    }
}

/// #261：无主键目标表的一次真实往返，以及连跑两次数据翻倍这件已知的事。
///
/// 这条测试是本票唯一能证明「纯 `INSERT ... SELECT` 真的跑得通」的东西——替身能证明
/// 编排，证不了 MySQL 在一张没有唯一约束的表上如何计数。三件事一起钉：
///
/// 1. 语句确实写进去了，行数**严格相等**（不是区间——纯 INSERT 没有更新那条腿）；
/// 2. 再跑一次，目标表变成两份。这不是缺陷，是这条路的定义，所以它被断言，不是被回避；
/// 3. 严格判据在两次运行上都成立，也就是说「翻倍」不会被判成一次失败的切换。
#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn a_keyless_target_appends_and_a_second_run_doubles_it() {
    let mut rig = Rig::open_without_a_key("keyless");
    let rows = [("1", "a"), ("2", "b"), ("3", "c")];

    let first = rig.stage_and_swap("r1", &rows).expect("the first append");
    assert_eq!(
        (first.staged_rows, first.swapped_rows),
        (3, 3),
        "a plain INSERT ... SELECT inserts exactly what was staged"
    );
    assert_eq!(rig.target_row_count(), 3);
    assert!(swap_rows_consistent(
        WriteStatement::Insert,
        first.staged_rows,
        first.swapped_rows
    ));

    // 一模一样的三行再跑一遍。有主键时这是幂等的一次重跑；没有主键时它是又一次追加。
    let second = rig.stage_and_swap("r2", &rows).expect("the second append");
    assert_eq!((second.staged_rows, second.swapped_rows), (3, 3));
    assert_eq!(
        rig.target_row_count(),
        6,
        "重跑不再幂等：同一份任务定义跑两次，目标表状态不同（#261）"
    );
    assert!(swap_rows_consistent(
        WriteStatement::Insert,
        second.staged_rows,
        second.swapped_rows
    ));

    // 对照：upsert 那一档的区间判据会放行 6，等值判据不会——这正是收紧的意义。
    assert!(swap_rows_consistent(WriteStatement::Upsert, 3, 6));
    assert!(!swap_rows_consistent(WriteStatement::Insert, 3, 6));
}

#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn append_pre_sql_on_a_keyless_target_cleans_then_uses_plain_insert() {
    let mut rig = Rig::open_without_a_key("pre_sql_keyless");
    rig.stage_and_swap(
        "r1",
        &[("1", "keep"), ("2", "dirty"), ("3", "dirty")],
    )
    .expect("laying down old keyless target rows");
    let pre_sql = format!(
        "DELETE FROM `{}`.`{}` WHERE K IN ('2', '3')",
        rig.database, rig.target_table
    );

    let cleaned = rig
        .append_with_pre_sql("r2", &[("2", "fresh"), ("4", "new")], &pre_sql)
        .expect("cleanup and plain insert must commit together");

    assert_eq!(cleaned.purged_rows, 2);
    assert_eq!((cleaned.staged_rows, cleaned.swapped_rows), (2, 2));
    assert!(swap_rows_consistent(
        WriteStatement::Insert,
        cleaned.staged_rows,
        cleaned.swapped_rows
    ));
    assert_eq!(
        rig.target_rows(),
        vec![
            ("1".to_owned(), "keep".to_owned()),
            ("2".to_owned(), "fresh".to_owned()),
            ("4".to_owned(), "new".to_owned()),
        ]
    );
}

/// #264：清空后导入的一次真实往返——**跑完之后目标表精确等于本次查询的结果**。
///
/// 这是本票的核心承诺，而它只有真 MySQL 能回答：替身证得了编排顺序，证不了
/// 一条事务内整表 `DELETE` 加一条 `INSERT ... SELECT` 在 InnoDB 上到底留下什么。
///
/// 先用追加写铺一份底（1、2、3），再用清空后导入跑一份**不含 1、却多出 4** 的查询结果。
/// 追加写模型下 1 会永远留在目标表里，那正是这个模式来解决的债。断言比的是逐行值，
/// 不只是行数：行数相等而内容不同，恰恰是最难发现的那种错。
#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn a_clear_then_import_run_leaves_the_target_exactly_equal_to_this_run() {
    let mut rig = Rig::open("replace");

    rig.stage_and_swap("r1", &[("1", "a"), ("2", "b"), ("3", "c")])
        .expect("the append that lays down the old contents");
    assert_eq!(rig.target_row_count(), 3);

    // 源端删掉了 1，改了 2，新增了 4。
    let replaced = rig
        .clear_and_import("r2", &[("2", "b2"), ("3", "c"), ("4", "d")])
        .expect("the clear-then-import run");
    assert_eq!(
        replaced.purged_rows, 3,
        "事务内那条整表 DELETE 报告删掉了原有的三行"
    );
    assert_eq!(
        rig.target_rows(),
        vec![
            ("2".to_owned(), "b2".to_owned()),
            ("3".to_owned(), "c".to_owned()),
            ("4".to_owned(), "d".to_owned()),
        ],
        "目标表精确等于本次查询的结果：源端删掉的 1 不再残留（#264）"
    );

    // 语句的选择**没有被清空改变**：目标表有主键，走的仍是 upsert，判据仍是区间。
    assert!(swap_rows_consistent(
        WriteStatement::Upsert,
        replaced.staged_rows,
        replaced.swapped_rows
    ));
}

#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn append_pre_sql_cleans_only_its_range_then_upserts_the_staged_rows() {
    let mut rig = Rig::open("pre_sql_append");
    rig.stage_and_swap("r1", &[("1", "keep"), ("2", "dirty"), ("4", "dirty")])
        .expect("laying down old target rows");
    let pre_sql = format!(
        "DELETE FROM `{}`.`{}` WHERE K IN ('2', '4')",
        rig.database, rig.target_table
    );

    let cleaned = rig
        .append_with_pre_sql("r2", &[("2", "fresh"), ("4", "new")], &pre_sql)
        .expect("cleanup and upsert must commit together");

    assert_eq!(cleaned.purged_rows, 2);
    assert_eq!(cleaned.staged_rows, 2);
    assert_eq!(
        rig.target_rows(),
        vec![
            ("1".to_owned(), "keep".to_owned()),
            ("2".to_owned(), "fresh".to_owned()),
            ("4".to_owned(), "new".to_owned()),
        ]
    );
}

/// #264：导入中途失败时，目标表**原样不动**——包括那条已经执行过的整表 DELETE。
///
/// 这一条是不用 `TRUNCATE` 的全部理由。`TRUNCATE` 是 DDL、隐式提交，同样的失败会
/// 留下一张空表；`DELETE` 在同一个事务里，回滚之后一行都没少。
///
/// 失败是造出来的，而且是**真的失败**：暂存表每一列都可空，目标表的 `V` 是
/// `NOT NULL`，于是导入那条 `INSERT ... SELECT` 在事务中途撞 `ERROR 1048`。
#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn a_clear_whose_import_fails_leaves_the_target_untouched() {
    let mut rig = Rig::open("replace_fail");

    rig.stage_and_swap("r1", &[("1", "a"), ("2", "b"), ("3", "c")])
        .expect("the append that lays down the contents that must survive");
    let before = rig.target_rows();
    assert_eq!(before.len(), 3);

    let failure = rig
        .stage_and_swap_nullable(
            "r2",
            &[("7", Some("g")), ("8", None)],
            WriteMode::ClearThenImport,
        )
        .expect_err("a NULL into a NOT NULL column must fail the import");
    assert!(
        failure.contains("1048") || failure.to_ascii_uppercase().contains("NULL"),
        "失败得是导入那条语句被拒，不是别的：{failure}"
    );

    assert_eq!(
        rig.target_rows(),
        before,
        "导入中途失败，事务回滚，那条整表 DELETE 跟着一起没了：目标表原样不动（#264）"
    );
}

/// #264：清空模式下，同一次运行内出现重复主键**不导致失败**。
///
/// 这是「清空不改变写入语句选择」那条规则的用处所在。对着刚清空的表打纯 `INSERT`，
/// 两行同键会撞 `ERROR 1062`、整次运行半途而废；保留 upsert 之后，后一行赢，
/// 运行照常跑完。行数判据也照旧按 upsert 那一档的区间走。
#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn a_clear_mode_run_tolerates_a_duplicate_key_within_itself() {
    let mut rig = Rig::open("replace_dup");

    let result = rig
        .clear_and_import("r1", &[("1", "a"), ("1", "b"), ("2", "c")])
        .expect("重复主键不该让清空模式的这次运行失败");
    assert_eq!(result.staged_rows, 3);
    assert!(
        swap_rows_consistent(
            WriteStatement::Upsert,
            result.staged_rows,
            result.swapped_rows
        ),
        "{result:?} 仍按 upsert 那一档的区间判"
    );
    assert_eq!(
        rig.target_rows(),
        vec![
            ("1".to_owned(), "b".to_owned()),
            ("2".to_owned(), "c".to_owned())
        ],
        "同键的后一行赢，运行不失败"
    );
}

/// The Connection Ritual is the pool's, not the first connection's.
///
/// Nothing else asserts this. The ritual runs inside the pool's connection
/// creation, so the way to reach the second connection is to make the first one
/// unavailable: one thread parks it inside a statement that blocks, which leaves
/// the pool no choice but to open a new connection for the probe. That connection's
/// own session state then goes to the same [`check_connection_settings`] the ritual
/// uses — the test states no thresholds of its own.
///
/// **The occupier blocks on a lock this test holds, rather than on a sleep.** A
/// sleep would put a clock on the window: finish early and the probe pops the
/// freed connection instead, and a passing ritual reads as a regression. The lock
/// makes the window close when this test says so and not before.
#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn the_second_connection_the_pool_opens_has_been_through_the_ritual() {
    let rig = Rig::open("ritual");

    let first = rig.probe_the_connection_in_use("p1");

    let second = thread::scope(|scope| {
        rig.take_the_lock();
        let occupier = scope.spawn(|| rig.on_a_pooled_connection(&rig.wait_for_the_lock()));
        rig.wait_until_the_pooled_connection_is_blocked();
        let second = rig.probe_the_connection_in_use("p2");
        rig.release_the_lock();
        occupier.join().expect("occupying thread panicked");
        second
    });

    assert_ne!(
        second.connection_id, first.connection_id,
        "the probe must have landed on a connection the pool opened for it, \
         not on the one the occupier is holding"
    );
    check_connection_settings(
        &second.character_set_client,
        &second.character_set_connection,
        &second.character_set_results,
        &second.sql_mode,
        second.max_allowed_packet,
    )
    .expect("the second connection the pool opened came up bare");
}

/// #262: an auto-increment column is recognised on whichever version answers.
///
/// The judgement is `EXTRA` **contains** `auto_increment`, case-insensitively — not
/// an equality test against `DEFAULT_GENERATED`, which is a value only 8.0 ever
/// produces. Against 5.7 that equality test made every auto-increment column read
/// as an ordinary one, and the precheck then let through the very column it exists
/// to stop: unmapped, `NOT NULL`, no default. The mistake is invisible on 8.0,
/// which is exactly why it has to be asked of a real server on both versions.
///
/// The control at the end is what makes this a test rather than a coincidence: with
/// the column's own `EXTRA` blanked — 5.7's answer under the old comparison — the
/// same precheck rejects, so the pass above is attributable to the recognition and
/// not to some other branch waving the column through.
#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn an_unmapped_auto_increment_column_is_recognised_on_this_server() {
    let rig = Rig::open_with_columns(
        "autoinc",
        "`SEQ_NO` BIGINT NOT NULL AUTO_INCREMENT, \
         `K` VARCHAR(16) NOT NULL, \
         `V` VARCHAR(32) NULL, \
         `CREATE_TIME` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP, \
         PRIMARY KEY (`SEQ_NO`), UNIQUE KEY `uk_k` (`K`)",
        vec!["K".to_owned()],
    );

    let columns = rig
        .destination
        .target_columns(&rig.target_table)
        .expect("reading target columns");
    let keys = rig
        .destination
        .target_keys(&rig.target_table)
        .expect("reading target keys");

    let sequence = columns
        .iter()
        .find(|column| column.name == "SEQ_NO")
        .unwrap_or_else(|| panic!("{columns:?}"));
    assert!(
        sequence
            .extra
            .to_ascii_lowercase()
            .contains("auto_increment"),
        "this server spells EXTRA {:?}, and the judgement must survive its spelling",
        sequence.extra
    );
    assert!(
        !sequence.nullable && sequence.default_value.is_none(),
        "the column has to be the hard case — NOT NULL with no default — or the \
         precheck below would pass for an unrelated reason: {sequence:?}"
    );

    // Only K and V are mapped. SEQ_NO is filled by the database and CREATE_TIME by
    // its default, so neither may be demanded of the source.
    let sources = vec![varchar_source("K", 16), varchar_source("V", 32)];
    let primary_key = vec!["K".to_owned()];
    let issues =
        precheck_with_primary_key(&rig.target_table, &primary_key, &sources, &columns, &keys);
    assert_eq!(
        issues,
        Vec::new(),
        "an unmapped auto-increment column is the database's to fill, not a rejection"
    );

    let blanked: Vec<_> = columns
        .iter()
        .cloned()
        .map(|mut column| {
            if column.name == "SEQ_NO" {
                column.extra = String::new();
            }
            column
        })
        .collect();
    let without_recognition =
        precheck_with_primary_key(&rig.target_table, &primary_key, &sources, &blanked, &keys);
    assert!(
        without_recognition
            .iter()
            .any(|issue| issue.column == "SEQ_NO"),
        "the control: unrecognised, the same column is refused — {without_recognition:?}"
    );
}

/// #262: the 64 MiB gate holds, and says what to type.
///
/// MySQL 5.7 ships `max_allowed_packet` at 4 MiB, so **every untuned 5.7 meets this
/// message before it ever moves a row**. The gate is not relaxed — it is what stops
/// a large batch being truncated at the protocol layer, which surfaces as a syntax
/// error and sends whoever is on call digging through business data that is fine.
/// What changed is that the message now hands over the command and the my.cnf line.
///
/// The server really is lowered and really is put back: asserting on a message
/// composed from a made-up number would prove the formatting, not the gate. The
/// global is restored **before** the assertions so that a failing assertion leaves
/// the rig usable for the next test.
#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn an_untuned_max_allowed_packet_is_refused_with_the_command_that_fixes_it() {
    let target = target_from_env();
    let error = {
        let _untuned = UntunedPacket::lower_to(4 * 1024 * 1024);
        MysqlDestination::connect(&target)
            .err()
            .expect("a 4 MiB packet must not open — this gate is not relaxed")
    };

    assert!(
        error.contains("max_allowed_packet") && error.contains(&MIN_PACKET.to_string()),
        "{error}"
    );
    assert!(
        error.contains("SET GLOBAL max_allowed_packet = 67108864;"),
        "the command has to be copyable as it stands: {error}"
    );
    assert!(
        error.contains("my.cnf")
            && error.contains("[mysqld]")
            && error.contains("max_allowed_packet = 64M"),
        "and it has to survive the next restart: {error}"
    );
    assert!(
        error.contains("不要排查业务数据"),
        "the data is not the problem and the message must say so: {error}"
    );
}

/// #257 on both halves of the matrix (#262): the version is observed, never inferred.
///
/// 5.7 has no `utf8mb4_0900_ai_ci` at all, so a collation guessed from a version
/// number is a `CREATE TABLE` that fails on the customer's server after the mapping
/// was already agreed. This test states only what must be true of any server this
/// suite is legitimately pointed at, so it passes unchanged on either.
#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn the_destination_reports_the_server_it_is_actually_talking_to() {
    let rig = Rig::open("serverinfo");
    let observed = rig
        .destination
        .server_info()
        .expect("a credentialed destination has already read the server's own account of itself");

    assert!(
        observed.version.starts_with("5.7.") || observed.version.starts_with("8.0."),
        "the support matrix is 5.7 and 8.0; this rig is pointed at {:?}",
        observed.version
    );
    assert!(
        observed.utf8mb4_collation.starts_with("utf8mb4_"),
        "{observed:?}"
    );
    if observed.version.starts_with("5.7.") {
        assert_ne!(
            observed.utf8mb4_collation, "utf8mb4_0900_ai_ci",
            "5.7 does not have that collation, so reporting it would produce DDL \
             the server refuses"
        );
    }

    // And the value is the server's, not a constant: it agrees with what the server
    // says when asked directly, over a connection this test opened itself.
    let mut reader = rig.new_plain_connection();
    let (version, collation): (String, String) = reader
        .query_first(
            "SELECT @@version, \
             (SELECT DEFAULT_COLLATE_NAME FROM information_schema.CHARACTER_SETS \
               WHERE CHARACTER_SET_NAME = 'utf8mb4')",
        )
        .expect("asking the server directly")
        .expect("one row");
    assert_eq!(
        (observed.version, observed.utf8mb4_collation),
        (version, collation)
    );
}

/// A source column of the shape the nine-row whitelist calls a character column.
fn varchar_source(name: &str, length: u64) -> SourceColumn {
    SourceColumn {
        name: name.to_owned(),
        data_type: "VARCHAR2".to_owned(),
        precision: None,
        scale: None,
        length: Some(length),
        fsp: None,
        support: None,
    }
}

/// `max_allowed_packet` held below the ritual's gate for the length of one test,
/// and put back on the way out — including while unwinding from a failed assertion,
/// which is the whole reason this is a guard and not two statements.
struct UntunedPacket {
    root: Conn,
    previous: u64,
}

impl UntunedPacket {
    fn lower_to(bytes: u64) -> Self {
        let target = target_from_env();
        let mut root = Conn::new(Opts::from(
            OptsBuilder::new()
                .ip_or_hostname(Some(target.host))
                .tcp_port(target.port)
                .user(Some("root".to_owned()))
                .pass(Some(required("DB_QBS_TEST_MYSQL_ROOT_PASSWORD"))),
        ))
        .expect("moving a global needs the administrative account");
        let previous: u64 = root
            .query_first("SELECT @@GLOBAL.max_allowed_packet")
            .expect("reading the global")
            .expect("one row");
        root.query_drop(format!("SET GLOBAL max_allowed_packet = {bytes}"))
            .expect("lowering the global");
        Self { root, previous }
    }
}

impl Drop for UntunedPacket {
    fn drop(&mut self) {
        let previous = self.previous;
        if let Err(error) = self
            .root
            .query_drop(format!("SET GLOBAL max_allowed_packet = {previous}"))
        {
            // Never panic while unwinding, but never lose it quietly either: every
            // later test on this server would fail its Connection Ritual.
            eprintln!("!! max_allowed_packet left at a lowered value: {error}");
        }
    }
}

/// What one connection reports about itself.
#[derive(Debug)]
struct ConnectionProbe {
    connection_id: u64,
    character_set_client: String,
    character_set_connection: String,
    character_set_results: String,
    sql_mode: String,
    max_allowed_packet: u64,
}

/// One target table, one destination pointed at it, and one connection of the
/// test's own — outside the pool, and on the driver's plain defaults, which is to
/// say without `CLIENT_FOUND_ROWS`.
struct Rig {
    destination: MysqlDestination,
    database: String,
    /// Every table this rig creates starts with these characters, which is how
    /// [`Rig::drop`] finds them again without keeping a list.
    prefix: String,
    target_table: String,
    /// 这张目标表的主键，也就是这台台架跑的是哪一种写法（#261）。
    /// 空 = 目标表上什么唯一约束都没有，切换段走纯 `INSERT ... SELECT`。
    primary_key: Vec<String>,
    plain_opts: Opts,
    plain: Conn,
    /// The connection holding the occupier's lock, if one is held. A MySQL named
    /// lock lives and dies with the connection that took it, so releasing is
    /// `take()`ing this — and the whole rig can hand it out behind `&self`.
    locker: Mutex<Option<Conn>>,
}

/// The table [`Rig::open`] creates when a test does not ask for another shape.
const DEFAULT_TABLE_BODY: &str =
    "`K` VARCHAR(16) NOT NULL, `V` VARCHAR(32) NOT NULL, PRIMARY KEY (`K`)";

/// The same two columns with no unique constraint at all — the pure-append path (#261).
const KEYLESS_TABLE_BODY: &str = "`K` VARCHAR(16) NULL, `V` VARCHAR(32) NULL";

impl Rig {
    fn open(label: &str) -> Self {
        Self::open_with_columns(label, DEFAULT_TABLE_BODY, vec!["K".to_owned()])
    }

    /// 同一台台架，目标表**没有任何唯一约束**（#261）。
    fn open_without_a_key(label: &str) -> Self {
        Self::open_with_columns(label, KEYLESS_TABLE_BODY, Vec::new())
    }

    /// The same rig over a target table of the caller's own shape — for the tests
    /// that are about what `information_schema` says, rather than about the swap.
    fn open_with_columns(label: &str, table_body: &str, primary_key: Vec<String>) -> Self {
        let target = target_from_env();

        let plain_opts = Opts::from(
            OptsBuilder::new()
                .ip_or_hostname(Some(target.host.clone()))
                .tcp_port(target.port)
                .user(Some(target.username.clone()))
                .pass(Some(target.password.clone()))
                .db_name(Some(target.database.clone())),
        );
        let mut plain = Conn::new(plain_opts.clone()).expect("the test's own MySQL connection");

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before the epoch")
            .as_nanos();
        let prefix = format!("qbs_live_{label}_{}", stamp % 1_000_000_007);
        let target_table = format!("{prefix}_t");
        plain
            .query_drop(format!(
                "CREATE TABLE `{}`.`{target_table}` ({table_body}) \
                 ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
                target.database
            ))
            .expect("creating the target table");

        let destination = MysqlDestination::connect(&target).expect("connecting the destination");
        Self {
            destination,
            database: target.database.clone(),
            prefix,
            target_table,
            primary_key,
            plain_opts,
            plain,
            locker: Mutex::new(None),
        }
    }

    /// One whole run against the target table: staging table from the target's own
    /// metadata, one batch, one swap, staging table dropped. The production chain,
    /// minus the HTTP in front of it.
    fn stage_and_swap(
        &mut self,
        run: &str,
        rows: &[(&str, &str)],
    ) -> Result<AtomicSwapResult, String> {
        self.stage_and_swap_as(run, rows, WriteMode::Append)
    }

    /// 同一条链路，清空后导入那一档（#264）：切换段在同一个事务里先整表 DELETE。
    fn clear_and_import(
        &mut self,
        run: &str,
        rows: &[(&str, &str)],
    ) -> Result<AtomicSwapResult, String> {
        self.stage_and_swap_as(run, rows, WriteMode::ClearThenImport)
    }

    fn append_with_pre_sql(
        &mut self,
        run: &str,
        rows: &[(&str, &str)],
        pre_sql: &str,
    ) -> Result<AtomicSwapResult, String> {
        let rows: Vec<(&str, Option<&str>)> = rows
            .iter()
            .map(|(key, value)| (*key, Some(*value)))
            .collect();
        self.stage_and_swap_nullable_with_pre_sql(
            run,
            &rows,
            WriteMode::Append,
            Some(pre_sql.to_owned()),
        )
    }

    fn stage_and_swap_as(
        &mut self,
        run: &str,
        rows: &[(&str, &str)],
        write_mode: WriteMode,
    ) -> Result<AtomicSwapResult, String> {
        let rows: Vec<(&str, Option<&str>)> = rows
            .iter()
            .map(|(key, value)| (*key, Some(*value)))
            .collect();
        self.stage_and_swap_nullable(run, &rows, write_mode)
    }

    /// 同一条链路，但暂存的值可以是 NULL——暂存表每一列都可空（`build_staging_ddl`），
    /// 而目标表的 `V` 是 `NOT NULL`，于是导入那条语句会在事务中途撞 `ERROR 1048`。
    /// 这是本文件唯一能造出「清空已经执行、导入失败」的办法。
    fn stage_and_swap_nullable(
        &mut self,
        run: &str,
        rows: &[(&str, Option<&str>)],
        write_mode: WriteMode,
    ) -> Result<AtomicSwapResult, String> {
        self.stage_and_swap_nullable_with_pre_sql(run, rows, write_mode, None)
    }

    fn stage_and_swap_nullable_with_pre_sql(
        &mut self,
        run: &str,
        rows: &[(&str, Option<&str>)],
        write_mode: WriteMode,
        pre_sql: Option<String>,
    ) -> Result<AtomicSwapResult, String> {
        let staging_table = format!("{}_stg_{run}", self.prefix);
        let columns = self
            .destination
            .target_columns(&self.target_table)
            .expect("reading target columns");
        let ddl = build_staging_ddl(&self.database, &staging_table, &columns);
        self.destination
            .create_staging(&staging_table, &ddl)
            .map_err(|error| format!("{error:?}"))?;

        let names: Vec<String> = columns.iter().map(|column| column.name.clone()).collect();
        let values: Vec<Vec<Option<String>>> = rows
            .iter()
            .map(|(key, value)| vec![Some((*key).to_owned()), value.map(str::to_owned)])
            .collect();
        self.destination
            .write_batch(&staging_table, &names, &values, 100)
            .map_err(|error| format!("{error:?}"))?;

        let result = self
            .destination
            .atomic_swap(&AtomicSwapRequest {
                run_id: format!("{}_{run}", self.prefix),
                staging_table: staging_table.clone(),
                target_table: self.target_table.clone(),
                write_mode,
                pre_sql,
                primary_key: self.primary_key.clone(),
                columns: names,
                source_rows: rows.len() as u64,
                source_batches: 1,
                received_batches: 1,
            })
            .map_err(|error| format!("{error:?}"));
        self.destination
            .drop_staging(&staging_table)
            .expect("dropping the staging table");
        result
    }

    /// The swap's own upsert, over the connection the test owns — which was built
    /// on the driver's defaults, so without `CLIENT_FOUND_ROWS`. Returns MySQL's
    /// `affected_rows`.
    ///
    /// `INSERT … SELECT … FROM` a staging table, not `VALUES`, because that is the
    /// form `build_swap_upsert_statement` produces. The builder is private to the
    /// destination, so the shape is matched by hand here; what differs between the
    /// two connections is the flag, and the flag counts matched rows the same way
    /// whichever form issued the upsert.
    fn upsert_over_a_plain_connection(&mut self, rows: &[(&str, &str)]) -> u64 {
        let staging_table = format!("{}_control", self.prefix);
        let values = rows
            .iter()
            .map(|(key, value)| format!("('{key}','{value}')"))
            .collect::<Vec<_>>()
            .join(",");
        for statement in [
            format!(
                "CREATE TABLE `{}`.`{staging_table}` LIKE `{}`.`{}`",
                self.database, self.database, self.target_table
            ),
            format!(
                "INSERT INTO `{}`.`{staging_table}` (`K`,`V`) VALUES {values}",
                self.database
            ),
        ] {
            self.plain
                .query_drop(statement)
                .expect("staging the control rows");
        }

        self.plain
            .query_drop(format!(
                "INSERT INTO `{}`.`{}` (`K`, `V`) SELECT `K`, `V` FROM `{}`.`{staging_table}` \
                 ON DUPLICATE KEY UPDATE `V` = VALUES(`V`)",
                self.database, self.target_table, self.database
            ))
            .expect("the control upsert");
        self.plain.affected_rows()
    }

    /// How many rows the target table holds right now, over the test's own
    /// connection. On the keyless path this is the whole point: the row count is
    /// the only thing that tells "appended again" apart from "merged".
    /// 目标表现在到底有哪些行，按主键排好——「跑完之后目标表精确等于本次查询结果」
    /// 这句话只有逐行比才算证明，光比行数不算（#264）。
    fn target_rows(&mut self) -> Vec<(String, String)> {
        self.plain
            .query_map(
                format!(
                    "SELECT `K`, `V` FROM `{}`.`{}` ORDER BY `K`, `V`",
                    self.database, self.target_table
                ),
                |(key, value): (String, String)| (key, value),
            )
            .expect("reading the target rows")
    }

    fn target_row_count(&mut self) -> u64 {
        self.plain
            .query_first(format!(
                "SELECT COUNT(*) FROM `{}`.`{}`",
                self.database, self.target_table
            ))
            .expect("counting the target rows")
            .expect("COUNT(*) must return one row")
    }

    /// Runs one statement of the test's own on a connection out of the pool.
    ///
    /// `create_staging` is the only door the `Destination` interface leaves for
    /// this: it hands its second argument straight to the connection, and the
    /// MySQL implementation never looks at the staging-table name it is given, so
    /// nothing here has to be a staging table.
    fn on_a_pooled_connection(&self, statement: &str) {
        self.destination
            .create_staging("not a staging table", statement)
            .expect("running a statement on a pooled connection");
    }

    /// Runs a statement through the pool and reads back the session state of
    /// whichever connection served it.
    fn probe_the_connection_in_use(&self, label: &str) -> ConnectionProbe {
        let table = format!("{}_{label}", self.prefix);
        self.on_a_pooled_connection(&format!(
            "CREATE TABLE `{}`.`{table}` AS SELECT \
             CONNECTION_ID() AS connection_id, \
             @@character_set_client AS character_set_client, \
             @@character_set_connection AS character_set_connection, \
             @@character_set_results AS character_set_results, \
             @@SESSION.sql_mode AS sql_mode, \
             @@max_allowed_packet AS max_allowed_packet",
            self.database
        ));

        let mut reader = self.new_plain_connection();
        let row: (u64, String, String, String, String, u64) = reader
            .query_first(format!("SELECT * FROM `{}`.`{table}`", self.database))
            .expect("reading the probe")
            .expect("the probe table must have one row");
        ConnectionProbe {
            connection_id: row.0,
            character_set_client: row.1,
            character_set_connection: row.2,
            character_set_results: row.3,
            sql_mode: row.4,
            max_allowed_packet: row.5,
        }
    }

    /// The named lock the occupier parks on. Named after the rig, so two runs
    /// against the same MySQL never wait on each other.
    fn lock_name(&self) -> String {
        format!("{}_occupied", self.prefix)
    }

    fn take_the_lock(&self) {
        let mut locker = self.new_plain_connection();
        let held: Option<u64> = locker
            .query_first(format!("SELECT GET_LOCK('{}', 10)", self.lock_name()))
            .expect("taking the lock");
        assert_eq!(held, Some(1), "the rig's own lock must be free");
        *self.locker.lock().expect("lock-holder mutex poisoned") = Some(locker);
    }

    /// The statement the occupier runs: it returns only once [`Rig::release_the_lock`]
    /// has been called, and holds its pooled connection until then.
    fn wait_for_the_lock(&self) -> String {
        format!("DO GET_LOCK('{}', 60)", self.lock_name())
    }

    fn release_the_lock(&self) {
        self.locker
            .lock()
            .expect("lock-holder mutex poisoned")
            .take();
    }

    /// Blocks until the occupying statement is visibly waiting, which is the moment
    /// the pool's only idle connection is provably gone. Polling the process list
    /// rather than sleeping a fixed while: the point is *that* the connection is
    /// taken, not how long it took.
    fn wait_until_the_pooled_connection_is_blocked(&self) {
        let mut watcher = self.new_plain_connection();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let waiting: Option<u64> = watcher
                .query_first(
                    "SELECT COUNT(*) FROM information_schema.PROCESSLIST \
                     WHERE INFO LIKE 'DO GET\\_LOCK%'",
                )
                .expect("reading the process list");
            if waiting.unwrap_or(0) > 0 {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the occupying statement never showed up in the process list"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn new_plain_connection(&self) -> Conn {
        Conn::new(self.plain_opts.clone()).expect("a further MySQL connection")
    }
}

impl Drop for Rig {
    /// Cleanup never panics — a failing test must report its own failure, not be
    /// buried under a double panic while unwinding — but it never fails silently
    /// either: whatever is left behind is named on stderr so the next run's
    /// leftovers have an explanation.
    fn drop(&mut self) {
        let mut left_behind = Vec::new();
        let tables: Vec<String> = match self.plain.query(format!(
            "SELECT TABLE_NAME FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = '{}' AND TABLE_NAME LIKE '{}%'",
            self.database, self.prefix
        )) {
            Ok(tables) => tables,
            Err(error) => {
                eprintln!("!! {} left everything behind: {error}", self.prefix);
                return;
            }
        };
        for table in tables {
            if let Err(error) = self.plain.query_drop(format!(
                "DROP TABLE IF EXISTS `{}`.`{table}`",
                self.database
            )) {
                left_behind.push(format!("table {table}: {error}"));
            }
        }
        if !left_behind.is_empty() {
            eprintln!("!! {} left behind {}", self.prefix, left_behind.join("; "));
        }
    }
}

/// The one target connection every test in this file is pointed at. Which MySQL
/// version answers on the other end is the environment's business, not the tests'.
fn target_from_env() -> TargetConnection {
    TargetConnection {
        host: required("DB_QBS_TEST_MYSQL_HOST"),
        port: required("DB_QBS_TEST_MYSQL_PORT")
            .parse()
            .expect("DB_QBS_TEST_MYSQL_PORT must be a port number"),
        username: required("DB_QBS_TEST_MYSQL_USER"),
        password: required("DB_QBS_TEST_MYSQL_PASSWORD"),
        database: required("DB_QBS_TEST_MYSQL_DATABASE"),
    }
}

/// These tests only run when asked for by name, so a missing variable is a broken
/// environment rather than a reason to do nothing quietly.
fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| {
        panic!("{name} is not set — these tests need a real MySQL; {RIG_SCRIPT} brings one up")
    })
}
