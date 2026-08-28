//! Write mode — part of the task definition — and the statement shape it lands on.
//!
//! Two things live here and they are deliberately **not** the same thing:
//!
//! - [`WriteMode`] is what the author chose and what the task definition stores.
//!   Today there is exactly one value, `Append`; the clear-then-import mode
//!   arrives later and slots in as a second variant.
//! - [`WriteStatement`] is the SQL the target end actually runs. It is **not**
//!   chosen by anybody: it is decided by one fact only — whether the target
//!   table has a unique constraint to merge on. A table that has one gets
//!   `ON DUPLICATE KEY UPDATE`; a table that has none gets a plain
//!   `INSERT ... SELECT`, because on such a table the upsert clause would
//!   silently degrade into that same plain insert while pretending to dedupe.
//!
//! Keeping them apart is what lets the run-time check exist at all: the task
//! definition records the primary key it was authored against, and if the
//! target table's key situation has moved since, the two derivations disagree
//! and the run **fails** rather than quietly changing statement kind under the
//! same task definition.
//!
//! ## The idempotence promise is now conditional
//!
//! Before this module the product could say, without qualification, that
//! running the same task twice left the target table in the same state. With a
//! primary-key-less target that is no longer true — a second run appends the
//! rows a second time. That is accepted, and it is exactly why the no-key case
//! has to be *visible* everywhere it is decided: the precheck conclusion, the
//! task definition, the task list, and the run detail all say it out loud.
//!
//! Both ends read this module. The derivation ([`WriteStatement::for_primary_key`])
//! has one implementation for the same reason [`crate::verification`] has one:
//! the row-count adjudication forks on it, and two copies would drift.

use serde::{Deserialize, Serialize};

/// How the task writes into the target table. Part of the task definition.
///
/// Serialised as an upper-case string because it crosses the process line and
/// gets written into the per-run TOML task file; the spelling is pinned by a
/// test for the same reason [`crate::RunStage`]'s five are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WriteMode {
    /// Rows are added to whatever the target table already holds. Nothing is
    /// deleted. This is the only mode the product has today, and the default
    /// for anything that does not say.
    #[default]
    Append,
}

impl WriteMode {
    pub const ALL: [Self; 1] = [Self::Append];

    /// The wire spelling. Never change one of these.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Append => "APPEND",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == text)
    }
}

/// The shape of the statement that moves staged rows into the target table.
///
/// Not a choice — a consequence. See the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WriteStatement {
    /// `INSERT ... SELECT ... ON DUPLICATE KEY UPDATE`. Re-running is idempotent.
    Upsert,
    /// Plain `INSERT ... SELECT`. Re-running **doubles the data**.
    Insert,
}

impl WriteStatement {
    /// The one derivation. An empty primary key means the task was authored
    /// against a target table with no unique constraint to merge on.
    ///
    /// It takes the *recorded* key rather than reading the target table because
    /// both ends have to agree on it before either touches MySQL; the target
    /// end separately checks that the table still matches what was recorded and
    /// refuses the run when it does not.
    pub const fn for_primary_key(primary_key: &[String]) -> Self {
        if primary_key.is_empty() {
            Self::Insert
        } else {
            Self::Upsert
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Upsert => "UPSERT",
            Self::Insert => "INSERT",
        }
    }

    /// Whether running this task twice leaves the target table as it was.
    pub const fn idempotent(self) -> bool {
        matches!(self, Self::Upsert)
    }
}
