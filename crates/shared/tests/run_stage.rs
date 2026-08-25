use db_qbs_shared::RunStage;

#[test]
fn stage_vocabulary_is_closed_and_stable() {
    // These five words are on the wire and in the acceptance rigs' `jq`
    // expressions. Changing one is a contract break, so it has to be a
    // deliberate act rather than a rename that compiles.
    assert_eq!(
        RunStage::ALL.map(RunStage::as_str),
        [
            "PREPARING",
            "STREAMING",
            "COMMITTING",
            "SUCCEEDED",
            "FAILED",
        ]
    );
}

#[test]
fn every_spelling_parses_back_to_the_stage_that_wrote_it() {
    for stage in RunStage::ALL {
        assert_eq!(RunStage::parse(stage.as_str()), Some(stage));
    }
}

#[test]
fn an_unrecognised_spelling_is_not_guessed_at() {
    // Case, whitespace and near-misses all fail. An unrecognised value means the
    // two ends are on different versions; the only safe reading is "I do not
    // know what this run is doing", which every caller then handles explicitly.
    for text in ["", "streaming", " STREAMING", "STREAMIN", "RUNNING", "null"] {
        assert_eq!(RunStage::parse(text), None, "{text:?} must not parse");
    }
}

#[test]
fn abort_is_allowed_only_before_the_commit_point() {
    // CONTEXT.md, Abort: "It is only ever sent before commit: once COMMITTING is
    // entered, the staging table's disposition has passed wholly to sink and
    // source permanently forfeits the right to abort."
    assert!(RunStage::Preparing.abort_allowed());
    assert!(RunStage::Streaming.abort_allowed());
    assert!(!RunStage::Committing.abort_allowed());
    assert!(!RunStage::Succeeded.abort_allowed());
    assert!(!RunStage::Failed.abort_allowed());
}

#[test]
fn serialising_a_stage_writes_the_wire_spelling() {
    assert_eq!(
        serde_json::to_string(&RunStage::Committing).expect("stage serialises"),
        "\"COMMITTING\""
    );
}
