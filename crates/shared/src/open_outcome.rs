//! What `POST /v1/runs` actually answered, and the one place its two answers are
//! told apart.
//!
//! Opening a run is two-legged. sink runs the mapping precheck first, and when a
//! column needs the 3.5th step — the range check, which only the source can
//! execute, because it counts real rows — it answers "not yet, go count these",
//! keeps nothing, and waits to be asked again with the counts filled in.
//!
//! That second answer travels inside a `200`. It is spelled as an empty
//! `staging_table` plus a populated `range_check_columns`, and both ends have to
//! agree on that spelling: sink writes it, source reads it, and a source that
//! reads only one of the two fields concludes the run is open and pushes a batch
//! into a run that does not exist. So the spelling is written down once, here,
//! and neither end constructs or inspects the pair by hand.
//!
//! The wire is deliberately unchanged. [`OpenRunResponse`] stays the shape it has
//! always been — it carries `#[serde(deny_unknown_fields)]`, so even an added
//! optional field would break a peer on the other version, and the two processes
//! are separately deployed. This module owns the *meaning* of those bytes, not
//! their layout.

use crate::{OpenRunResponse, RangeCheckColumn};

/// The two things `POST /v1/runs` can mean.
///
/// Constructed by sink, read by source, and in neither direction is there a
/// third possibility to forget about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenOutcome {
    /// The run exists: the staging table is created and batches may be pushed.
    Opened {
        run_id: String,
        staging_table: String,
        columns_checked: usize,
    },
    /// The run does *not* exist yet. sink stored nothing; it wants these columns
    /// range-checked and the same request sent again with the results attached.
    RangeCheckNeeded {
        run_id: String,
        columns_checked: usize,
        columns: Vec<RangeCheckColumn>,
    },
}

impl OpenOutcome {
    pub fn run_id(&self) -> &str {
        match self {
            Self::Opened { run_id, .. } | Self::RangeCheckNeeded { run_id, .. } => run_id,
        }
    }

    /// sink → wire.
    pub fn into_response(self) -> OpenRunResponse {
        match self {
            Self::Opened {
                run_id,
                staging_table,
                columns_checked,
            } => OpenRunResponse {
                run_id,
                staging_table,
                columns_checked,
                range_check_columns: None,
            },
            Self::RangeCheckNeeded {
                run_id,
                columns_checked,
                columns,
            } => OpenRunResponse {
                run_id,
                // The sentinel. Empty because there is no staging table: the
                // precheck has not finished, so nothing was created.
                staging_table: String::new(),
                columns_checked,
                range_check_columns: Some(columns),
            },
        }
    }

    /// wire → source.
    ///
    /// An empty `range_check_columns` reads as `Opened`, which is what source has
    /// always done with it: "check these zero columns" is not a request for
    /// anything, and a peer that sends it means the run is open.
    pub fn from_response(response: OpenRunResponse) -> Self {
        match response.range_check_columns {
            Some(columns) if !columns.is_empty() => Self::RangeCheckNeeded {
                run_id: response.run_id,
                columns_checked: response.columns_checked,
                columns,
            },
            _ => Self::Opened {
                run_id: response.run_id,
                staging_table: response.staging_table,
                columns_checked: response.columns_checked,
            },
        }
    }
}
