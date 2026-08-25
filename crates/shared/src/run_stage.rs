//! The five states a Run passes through, and the two rules that hang off them.
//!
//! The stage name crosses a process line: the child writes it into a
//! `stage_changed` Run Log line, and the long-running parent reads it back to
//! decide whether an abort is still allowed. Before this module the two ends met
//! as bare strings — one side spelled them from an enum, the other compared them
//! against literals — so a rename on either side failed silently, in the one
//! place where silence is most expensive.
//!
//! The wire spelling is a contract (`CONTEXT.md` under **Run Log**: the field set
//! is stable, and the acceptance rigs `jq`-match on `.stage == "COMMITTING"`).
//! `as_str` below is that contract, and it is the only place the five words are
//! written down.

use std::fmt;

use serde::{Serialize, Serializer};

/// What the run process is doing right now.
///
/// Named after the work, not after a verdict: `SUCCEEDED` and `FAILED` are the
/// stages a finished process sits in, and they deliberately share their spelling
/// with a run history row's `outcome`, which is the *verdict* over the same two
/// words. The two are separate vocabularies that happen to agree here; nothing
/// converts between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStage {
    Preparing,
    Streaming,
    Committing,
    Succeeded,
    Failed,
}

impl RunStage {
    /// The closed set, in the order a run walks it. Pinned by a test, like
    /// [`crate::LogEvent::ALL`] — a vocabulary that crosses a process line is
    /// only worth anything if adding to it is a deliberate act.
    pub const ALL: [Self; 5] = [
        Self::Preparing,
        Self::Streaming,
        Self::Committing,
        Self::Succeeded,
        Self::Failed,
    ];

    /// The wire spelling. **Never change one of these** — see the module note.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "PREPARING",
            Self::Streaming => "STREAMING",
            Self::Committing => "COMMITTING",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
        }
    }

    /// The direction that used to be missing, and whose absence is why the
    /// parent process compared strings.
    ///
    /// `None` for anything else, including an empty string: an unrecognised
    /// spelling means the two ends are on different versions, and the callers
    /// that make decisions off a stage must all treat that as "I do not know
    /// what this run is doing" rather than guessing. What to *display* for such
    /// a value is a separate question, answered on the front end, which keeps
    /// the raw text so a half-finished upgrade stays visible.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|stage| stage.as_str() == text)
    }

    /// Whether `source` may still tell `sink` to discard the staging table.
    ///
    /// This is `CONTEXT.md`'s **Abort** invariant, and this is now its one
    /// implementation: *"It is only ever sent before commit: once `COMMITTING`
    /// is entered, the staging table's disposition has passed wholly to `sink`
    /// and source permanently forfeits the right to abort."*
    ///
    /// The two terminal stages are false for a different reason — there is no
    /// longer a process to stop — which is why refusal wording is chosen by
    /// matching the variant rather than by negating this.
    pub const fn abort_allowed(self) -> bool {
        match self {
            Self::Preparing | Self::Streaming => true,
            Self::Committing | Self::Succeeded | Self::Failed => false,
        }
    }
}

impl fmt::Display for RunStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for RunStage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}
