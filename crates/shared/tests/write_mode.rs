//! The write-mode vocabulary, pinned.
//!
//! These strings cross the process line *and* get written into the per-run TOML
//! task file, so they are pinned here for the same reason `run_stage.rs` pins
//! its five: never change one of them. Adding a value is fine — the clear-then-
//! import mode arrived that way (#264) — but a rename silently splits an old
//! source from a new sink.

use db_qbs_shared::{WriteMode, WriteStatement};

#[test]
fn the_write_mode_spellings_are_frozen() {
    assert_eq!(
        WriteMode::ALL,
        [WriteMode::Append, WriteMode::ClearThenImport]
    );
    assert_eq!(WriteMode::Append.as_str(), "APPEND");
    assert_eq!(WriteMode::ClearThenImport.as_str(), "CLEAR_THEN_IMPORT");
    // 每一档的拼写都要能原样回来。**解析只有 serde 一家**：产品里没有第二个
    // 写入模式解析器，也不该有——两份解析迟早在某个拼写上说出两个答案。
    for mode in WriteMode::ALL {
        let decoded: WriteMode = serde_json::from_value(serde_json::json!(mode.as_str()))
            .expect("wire spelling must parse");
        assert_eq!(decoded, mode);
    }
    for wrong in ["REPLACE", "REPLACED", "append"] {
        assert!(
            serde_json::from_value::<WriteMode>(serde_json::json!(wrong)).is_err(),
            "{wrong} is not a write mode"
        );
    }
}

/// 清空**不改变写入语句的选择**（#264）——这是清空模式最容易被写错的一条。
///
/// 派生只看任务定义记下的主键，模式一个字都不参与：有主键仍 upsert，是为了容忍
/// 同一次运行内出现重复主键；无主键仍纯 INSERT。
#[test]
fn clearing_the_target_does_not_change_which_statement_runs() {
    // 派生根本不接受模式做参数——这一行就是那件事本身：两档模式下同一份主键给出
    // 同一条语句，因为语句压根问不到模式。
    assert_eq!(
        WriteStatement::for_primary_key(&["ID".to_owned()]),
        WriteStatement::Upsert
    );
    assert_eq!(WriteStatement::for_primary_key(&[]), WriteStatement::Insert);
    assert!(!WriteMode::Append.clears_target());
    assert!(WriteMode::ClearThenImport.clears_target());
}

#[test]
fn append_is_the_default_because_it_is_what_every_task_does_today() {
    assert_eq!(WriteMode::default(), WriteMode::Append);
}

#[test]
fn the_write_mode_serialises_as_its_wire_spelling() {
    let encoded = serde_json::to_value(WriteMode::Append).expect("write mode must serialise");
    assert_eq!(encoded, serde_json::json!("APPEND"));
    let decoded: WriteMode =
        serde_json::from_value(serde_json::json!("APPEND")).expect("wire spelling must parse");
    assert_eq!(decoded, WriteMode::Append);

    let encoded =
        serde_json::to_value(WriteMode::ClearThenImport).expect("write mode must serialise");
    assert_eq!(encoded, serde_json::json!("CLEAR_THEN_IMPORT"));
    let decoded: WriteMode = serde_json::from_value(serde_json::json!("CLEAR_THEN_IMPORT"))
        .expect("wire spelling must parse");
    assert_eq!(decoded, WriteMode::ClearThenImport);
}

#[test]
fn the_statement_spellings_are_frozen_too() {
    assert_eq!(WriteStatement::Upsert.as_str(), "UPSERT");
    assert_eq!(WriteStatement::Insert.as_str(), "INSERT");
}
