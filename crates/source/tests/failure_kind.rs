//! 失败分类闭集的推导规则（ADR-0029）。
//!
//! 这里断言的是**分类的可判定性**，不是措辞：V1 成功标准第 4 条要求排障时不必读完整句人话
//! 才知道是哪一类坏了，所以每条推导都必须由结构化信号唯一定出，不能靠匹配文字。

use db_qbs_source::{oracle_kind, FailureKind};

#[test]
fn the_six_categories_v1_asks_for_are_each_separately_decidable() {
    // STRATEGY-V1 M4：Oracle 连接失败 / dblink 不可用 / 类型映射错 / 网络中断 /
    // MySQL 写入失败 / 校验不通过。六类两两不同码。
    let six = [
        oracle_kind(Some(12541), true),
        oracle_kind(Some(2019), false),
        FailureKind::from_sink_code("PRECHECK_FAILED"),
        FailureKind::Network,
        FailureKind::from_sink_code("BATCH_WRITE_FAILED"),
        FailureKind::from_sink_code("VERIFY_FAILED"),
    ];

    assert_eq!(
        six.map(FailureKind::as_str),
        [
            "SOURCE_CONNECT",
            "SOURCE_DBLINK",
            "MAPPING_PRECHECK",
            "NETWORK",
            "SINK_WRITE",
            "VERIFY_FAILED",
        ]
    );
}

#[test]
fn the_same_oracle_code_means_the_local_database_while_connecting_and_the_link_after() {
    // ORA-12541（监听器没起）在建连接那一步说的是本地库；会话已经建起来之后再撞上它，
    // 撞的只能是 dblink 那一头——分类看的是撞在哪一步，不是码本身。
    assert_eq!(oracle_kind(Some(12541), true), FailureKind::SourceConnect);
    assert_eq!(oracle_kind(Some(12541), false), FailureKind::SourceDblink);
}

#[test]
fn ora_01555_stays_a_source_query_failure_rather_than_a_link_failure() {
    // 快照过旧是本地游标寿命问题，不是远端库不可用；混进 dblink 会让排障去查网络。
    assert_eq!(oracle_kind(Some(1555), false), FailureKind::SourceQuery);
}

#[test]
fn batch_write_and_swap_no_longer_share_one_category() {
    // 拆码之前两者都报 SWAP_FAILED，从码上分不清是写批次坏了还是切换坏了。
    assert_eq!(
        FailureKind::from_sink_code("BATCH_WRITE_FAILED"),
        FailureKind::SinkWrite
    );
    assert_eq!(
        FailureKind::from_sink_code("SINK_ENVIRONMENT"),
        FailureKind::SinkEnvironment
    );
    assert_eq!(
        FailureKind::from_sink_code("DATA_REJECTED"),
        FailureKind::DataRejected
    );
}

#[test]
fn defects_do_not_get_dressed_up_as_run_failures() {
    for code in [
        "INTERNAL_PRECHECK_ESCAPE",
        "INTERNAL_ASSERTION_FAILED",
        "SEQ_MISMATCH",
        "RUN_SEALED",
        "RUN_UNKNOWN",
        "PAYLOAD_TOO_LARGE",
        "BAD_REQUEST",
    ] {
        assert_eq!(
            FailureKind::from_sink_code(code),
            FailureKind::Defect,
            "{code}"
        );
    }
}

#[test]
fn a_code_outside_the_closed_set_is_a_defect_not_a_silent_pass() {
    assert_eq!(
        FailureKind::from_sink_code("SOMETHING_THE_SINK_INVENTED"),
        FailureKind::Defect
    );
}
