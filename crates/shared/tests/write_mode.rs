//! The write-mode vocabulary, pinned.
//!
//! These strings cross the process line *and* get written into the per-run TOML
//! task file, so they are pinned here for the same reason `run_stage.rs` pins
//! its five: never change one of them. Adding a value is fine — the clear-then-
//! import mode arrives that way — but a rename silently splits an old source
//! from a new sink.

use db_qbs_shared::{WriteMode, WriteStatement};

#[test]
fn the_write_mode_spellings_are_frozen() {
    assert_eq!(WriteMode::ALL.len(), 1);
    assert_eq!(WriteMode::Append.as_str(), "APPEND");
    for mode in WriteMode::ALL {
        assert_eq!(WriteMode::parse(mode.as_str()), Some(mode));
    }
    assert_eq!(WriteMode::parse("REPLACE"), None);
    assert_eq!(WriteMode::parse("append"), None);
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
}

#[test]
fn the_statement_spellings_are_frozen_too() {
    assert_eq!(WriteStatement::Upsert.as_str(), "UPSERT");
    assert_eq!(WriteStatement::Insert.as_str(), "INSERT");
}
