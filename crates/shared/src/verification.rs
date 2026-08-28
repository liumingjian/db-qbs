//! The gate that stands between a finished push and the swap, and the one place
//! its judgement is written down.
//!
//! `CONTEXT.md` under **Verification**: it compares the row count the source read
//! against the row count that actually landed in the staging table, both numbers
//! pinned to one definition, with the batch counts taking part at the same level.
//! The comparison happens inside sink's swap transaction, so that "the thing
//! counted" and "the thing swapped" are the same snapshot.
//!
//! That is one rule, and before this module it had five implementations: sink's
//! real swap, source's re-check of the commit response, and three test fakes —
//! two of which returned `staged_rows = source_rows` and so kept the gate open
//! for every run that ever went through them. The ends had already drifted once:
//! sink widened the `swapped_rows` assertion to an interval and source's mirror
//! of it was missed, which failed every re-run that changed a value (#135, C4④).
//!
//! So the judgement lives here, next to [`crate::RunStage`], for the same reason:
//! it crosses the process line. The *wording* deliberately does not — each end
//! phrases the failure for its own reader, and only the verdict is shared.

use crate::WriteStatement;

/// The four numbers the gate is made of.
///
/// `source_rows` is the source's fetch-loop accumulator and `staged_rows` is
/// `SELECT COUNT(*)` over the staging table — never the sum of the per-batch
/// `rows_written`, which is what the gate exists to distrust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowCounts {
    pub source_rows: u64,
    pub staged_rows: u64,
    pub source_batches: u64,
    pub received_batches: u64,
}

/// What the gate decided, and — when it refused — which leg gave way.
///
/// The two failures are separate because the operator's next move differs: a
/// batch that never arrived is a transport problem and re-running is enough,
/// whereas rows that went missing between an accepted write and the staging
/// table is the case worth reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Passed,
    BatchesMissing {
        source_batches: u64,
        received_batches: u64,
    },
    RowsDiffer {
        source_rows: u64,
        staged_rows: u64,
    },
}

impl RowCounts {
    /// Batches are judged first: when a batch never arrived, the rows it carried
    /// are missing *because of that*, and saying so is more useful than
    /// reporting the row shortfall it caused.
    pub const fn verdict(&self) -> Verdict {
        if self.received_batches != self.source_batches {
            return Verdict::BatchesMissing {
                source_batches: self.source_batches,
                received_batches: self.received_batches,
            };
        }
        if self.staged_rows != self.source_rows {
            return Verdict::RowsDiffer {
                source_rows: self.source_rows,
                staged_rows: self.staged_rows,
            };
        }
        Verdict::Passed
    }
}

impl Verdict {
    pub const fn passed(self) -> bool {
        matches!(self, Self::Passed)
    }
}

/// Whether the swap's reported `affected_rows` is consistent with what was staged.
///
/// **The judgement forks on the statement, and the statement is passed in rather
/// than sniffed at the call site** — this module exists because the same rule
/// once had five implementations that drifted, and a `match` at each caller
/// would be that mistake again in a new shape.
///
/// - [`WriteStatement::Upsert`] — an interval, not an equality: under
///   `ON DUPLICATE KEY UPDATE` MySQL counts an insert as 1 and an update as 2
///   (ADR-0035 §4), so a re-run that genuinely changed values reports more than
///   it staged. The lower bound stays at `staged_rows` rather than 0 because
///   `CLIENT_FOUND_ROWS` already absorbs the third case, "an existing row whose
///   values did not change" (#138) — without that connection flag this bound
///   would be wrong.
/// - [`WriteStatement::Insert`] — **strictly equal**. A plain `INSERT ... SELECT`
///   has no update leg and no matched-row leg, so every one of the three cases
///   that widened the upsert interval is gone. Keeping the interval here would
///   mean accepting a swap that wrote twice what it staged, which on a
///   primary-key-less target is precisely the accident worth catching.
pub const fn swap_rows_consistent(
    statement: WriteStatement,
    staged_rows: u64,
    swapped_rows: u64,
) -> bool {
    match statement {
        WriteStatement::Upsert => {
            swapped_rows >= staged_rows && swapped_rows <= staged_rows.saturating_mul(2)
        }
        WriteStatement::Insert => swapped_rows == staged_rows,
    }
}
