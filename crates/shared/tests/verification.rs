use db_qbs_shared::{swap_rows_in_range, RowCounts, Verdict};

fn counts(
    source_rows: u64,
    staged_rows: u64,
    source_batches: u64,
    received_batches: u64,
) -> RowCounts {
    RowCounts {
        source_rows,
        staged_rows,
        source_batches,
        received_batches,
    }
}

#[test]
fn the_gate_passes_only_when_both_legs_agree() {
    assert_eq!(counts(5, 5, 2, 2).verdict(), Verdict::Passed);
    // An empty result is a legitimate run, not a failure to have read anything.
    assert_eq!(counts(0, 0, 0, 0).verdict(), Verdict::Passed);
}

#[test]
fn rows_that_never_landed_are_named_as_such() {
    assert_eq!(
        counts(5, 4, 2, 2).verdict(),
        Verdict::RowsDiffer {
            source_rows: 5,
            staged_rows: 4,
        }
    );
    // More staged than read is just as wrong as fewer, and for a worse reason:
    // it means something else is writing into the staging table.
    assert_eq!(
        counts(5, 6, 2, 2).verdict(),
        Verdict::RowsDiffer {
            source_rows: 5,
            staged_rows: 6,
        }
    );
}

#[test]
fn a_batch_that_never_arrived_outranks_the_rows_it_was_carrying() {
    // Both legs give way here — the missing batch is *why* the rows are missing.
    // Reporting the cause is what tells the operator to just re-run.
    assert_eq!(
        counts(5, 3, 2, 1).verdict(),
        Verdict::BatchesMissing {
            source_batches: 2,
            received_batches: 1,
        }
    );
    // The rows can still add up while a batch is missing (an empty batch, or one
    // whose rows another batch happened to duplicate). The gate still refuses:
    // an incomplete push is not something to swap into a target table.
    assert_eq!(
        counts(5, 5, 2, 1).verdict(),
        Verdict::BatchesMissing {
            source_batches: 2,
            received_batches: 1,
        }
    );
}

#[test]
fn only_a_passing_verdict_passes() {
    assert!(counts(5, 5, 2, 2).verdict().passed());
    assert!(!counts(5, 4, 2, 2).verdict().passed());
    assert!(!counts(5, 5, 2, 1).verdict().passed());
}

#[test]
fn the_swap_count_is_judged_as_an_interval_because_an_update_counts_twice() {
    // ADR-0035 §4: `ON DUPLICATE KEY UPDATE` counts an insert as 1 and an update
    // as 2, so a re-run that changed every value reports double what it staged.
    assert!(swap_rows_in_range(5, 5));
    assert!(swap_rows_in_range(5, 7));
    assert!(swap_rows_in_range(5, 10));
    // Below the lower bound means the swap wrote fewer rows than it staged —
    // the real failure this assertion exists to catch. The bound holds only
    // because `CLIENT_FOUND_ROWS` is set on the connection (#138); without it a
    // row whose values did not change would count 0 and this would fire.
    assert!(!swap_rows_in_range(5, 4));
    assert!(!swap_rows_in_range(5, 11));
}

#[test]
fn an_empty_run_swaps_nothing_and_that_is_in_range() {
    assert!(swap_rows_in_range(0, 0));
    assert!(!swap_rows_in_range(0, 1));
}
