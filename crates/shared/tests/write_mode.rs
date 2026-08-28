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
    assert_eq!(WriteMode::ALL.len(), 2);
    assert_eq!(WriteMode::Append.as_str(), "APPEND");
    assert_eq!(WriteMode::ClearThenImport.as_str(), "CLEAR_THEN_IMPORT");
    for mode in WriteMode::ALL {
        assert_eq!(WriteMode::parse(mode.as_str()), Some(mode));
    }
    assert_eq!(WriteMode::parse("REPLACE"), None);
    assert_eq!(WriteMode::parse("REPLACED"), None);
    assert_eq!(WriteMode::parse("append"), None);
}

/// 清空**不改变写入语句的选择**（#264）——这是清空模式最容易被写错的一条。
///
/// 派生只看任务定义记下的主键，模式一个字都不参与：有主键仍 upsert，是为了容忍
/// 同一次运行内出现重复主键；无主键仍纯 INSERT。
#[test]
fn clearing_the_target_does_not_change_which_statement_runs() {
    for mode in WriteMode::ALL {
        let _ = mode;
        assert_eq!(
            WriteStatement::for_primary_key(&["ID".to_owned()]),
            WriteStatement::Upsert
        );
        assert_eq!(WriteStatement::for_primary_key(&[]), WriteStatement::Insert);
    }
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

    let encoded = serde_json::to_value(WriteMode::ClearThenImport).expect("write mode must serialise");
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
