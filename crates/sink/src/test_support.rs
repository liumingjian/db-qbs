//! The second [`Destination`] adapter, and the only one besides [`MysqlDestination`].
//!
//! Before this module there were four fakes. Two of them
//! (`tests/sink_skeleton.rs`, `tests/batch_write.rs`) had an `atomic_swap` no
//! test ever called; a third (`src/http.rs`) answered every swap with
//! `staged_rows = request.source_rows`, which is the gate's own question answered
//! with its own input; the fourth (`tests/commit.rs`) re-implemented the
//! comparison by hand, so the rule had a second author. A fake that keeps the
//! gate open passes exactly the runs the real destination would refuse, and
//! nothing in the [`Destination`] interface asked any of them to do better.
//!
//! So this one keeps a staging table. It appends what it is told to write, counts
//! what is actually there when the swap comes, and hands those numbers to the same
//! [`RowCounts`] the real destination uses. To exercise a failing gate a test
//! removes rows from staging with [`InMemoryDestination::lose_staged_rows`] —
//! which is what the m1 acceptance rig does to real MySQL (`DELETE FROM stg`),
//! and is a cause rather than a verdict.
//!
//! It lives in the library rather than under `tests/` because the integration
//! tests and `http.rs`'s own unit tests have to share it: `#[cfg(test)]` items
//! are invisible to `tests/`, and "one fake" is the whole point. It is compiled
//! unconditionally; a few KB in the binary is the price.

use std::collections::HashMap;
use std::sync::Mutex;

use db_qbs_shared::{MysqlServerInfo, RowCounts};

use crate::{
    AtomicSwapError, AtomicSwapRequest, AtomicSwapResult, CreateStagingError, Destination,
    DropStagingError, TargetColumn, TargetKey, WriteBatchError,
};

/// One `write_batch` call, recorded whole so batching tests can assert on the
/// shape of the call rather than on its effect.
#[derive(Clone, Debug)]
pub struct BatchCall {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub max_rows_per_insert: usize,
}

/// A `Destination` that keeps its staging table in memory.
///
/// Every field is public and every error knob is a `Mutex<Option<_>>` that is
/// `take`n on use, so a test arms a single failure the same way it did with the
/// four fakes this replaces.
pub struct InMemoryDestination {
    pub columns: Vec<TargetColumn>,
    /// The unique constraint on the target table. Defaults to `PRIMARY KEY (D_BIZ)`:
    /// most tests' target table is a configured one, and a missing constraint is
    /// the exception a test states on purpose, not the resting state.
    pub keys: Vec<TargetKey>,
    /// staging table name → the rows currently in it.
    pub staging: Mutex<HashMap<String, Vec<Vec<Option<String>>>>>,
    pub calls: Mutex<Vec<BatchCall>>,
    pub created: Mutex<Vec<(String, String)>>,
    pub dropped: Mutex<Vec<String>>,
    pub swap_requests: Mutex<Vec<AtomicSwapRequest>>,
    pub create_error: Mutex<Option<CreateStagingError>>,
    pub write_error: Mutex<Option<WriteBatchError>>,
    pub swap_error: Mutex<Option<AtomicSwapError>>,
    pub drop_error: Mutex<Option<DropStagingError>>,
    /// Fail the write once the chunking has reached this sub-statement index.
    pub fail_chunk: Mutex<Option<usize>>,
    /// What `write_batch` reports back, when the test needs it to disagree with
    /// the number of rows it was handed.
    pub affected_rows: Mutex<Option<u64>>,
    /// Always 0 in production (ADR-0035 §4). Settable here so a test can prove
    /// the number is carried through rather than invented on the way out.
    pub purged_rows: Mutex<u64>,
    pub count_ms: Mutex<u64>,
    pub target_rows: Mutex<HashMap<(String, String), Vec<Option<String>>>>,
    /// What this destination claims its MySQL is (#257). `None` is the resting
    /// state — the fake is not connected to anything, so "never observed" is the
    /// honest default, and it is also the state the info endpoint must report as
    /// unknown rather than as 8.0.
    pub server: Mutex<Option<MysqlServerInfo>>,
}

impl Default for InMemoryDestination {
    fn default() -> Self {
        Self {
            columns: Vec::new(),
            keys: vec![TargetKey {
                name: "PRIMARY".to_owned(),
                columns: vec!["D_BIZ".to_owned()],
            }],
            staging: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
            created: Mutex::new(Vec::new()),
            dropped: Mutex::new(Vec::new()),
            swap_requests: Mutex::new(Vec::new()),
            create_error: Mutex::new(None),
            write_error: Mutex::new(None),
            swap_error: Mutex::new(None),
            drop_error: Mutex::new(None),
            fail_chunk: Mutex::new(None),
            affected_rows: Mutex::new(None),
            purged_rows: Mutex::new(0),
            count_ms: Mutex::new(0),
            target_rows: Mutex::new(HashMap::new()),
            server: Mutex::new(None),
        }
    }
}

impl InMemoryDestination {
    /// Arm what this destination reports as its MySQL version and collation.
    pub fn report_mysql(&self, version: &str, utf8mb4_collation: &str) {
        *self.server.lock().expect("server mutex poisoned") = Some(MysqlServerInfo {
            version: version.to_owned(),
            utf8mb4_collation: utf8mb4_collation.to_owned(),
        });
    }

    /// Every row that reached staging, across every staging table. The tests
    /// drive one run at a time, so this reads as "what is in the staging table".
    pub fn staged_row_values(&self) -> Vec<Vec<Option<String>>> {
        self.staging
            .lock()
            .expect("staging mutex poisoned")
            .values()
            .flatten()
            .cloned()
            .collect()
    }

    pub fn staged_rows(&self) -> u64 {
        self.staging
            .lock()
            .expect("staging mutex poisoned")
            .values()
            .map(|rows| rows.len() as u64)
            .sum()
    }

    /// Make rows disappear from staging after they were accepted — the write
    /// said yes, the rows are not there. This is what the gate exists to catch,
    /// and the only way to make this destination fail it.
    pub fn lose_staged_rows(&self, count: usize) {
        let mut staging = self.staging.lock().expect("staging mutex poisoned");
        let mut left = count;
        for rows in staging.values_mut() {
            let removed = left.min(rows.len());
            rows.truncate(rows.len() - removed);
            left -= removed;
            if left == 0 {
                return;
            }
        }
    }

    pub fn target_row_values(&self, target_table: &str) -> Vec<Vec<Option<String>>> {
        self.target_rows
            .lock()
            .unwrap()
            .iter()
            .filter(|((table, _), _)| table == target_table)
            .map(|(_, row)| row.clone())
            .collect()
    }
}

impl Destination for InMemoryDestination {
    fn server_info(&self) -> Option<MysqlServerInfo> {
        self.server.lock().expect("server mutex poisoned").clone()
    }

    fn target_columns(&self, _target_table: &str) -> Result<Vec<TargetColumn>, String> {
        Ok(self.columns.clone())
    }

    fn target_keys(&self, _target_table: &str) -> Result<Vec<TargetKey>, String> {
        Ok(self.keys.clone())
    }

    fn create_staging(&self, staging_table: &str, ddl: &str) -> Result<(), CreateStagingError> {
        if let Some(error) = self.create_error.lock().unwrap().take() {
            return Err(error);
        }
        self.created
            .lock()
            .unwrap()
            .push((staging_table.to_owned(), ddl.to_owned()));
        self.staging
            .lock()
            .unwrap()
            .entry(staging_table.to_owned())
            .or_default();
        Ok(())
    }

    fn write_batch(
        &self,
        staging_table: &str,
        columns: &[String],
        rows: &[Vec<Option<String>>],
        max_rows_per_insert: usize,
    ) -> Result<u64, WriteBatchError> {
        if let Some(error) = self.write_error.lock().unwrap().take() {
            return Err(error);
        }
        self.calls.lock().unwrap().push(BatchCall {
            columns: columns.to_vec(),
            rows: rows.to_vec(),
            max_rows_per_insert,
        });

        // A failing sub-statement leaves nothing behind: the real write runs the
        // chunks in one transaction, so a test that arms `fail_chunk` must find
        // staging untouched.
        let fail_chunk = *self.fail_chunk.lock().unwrap();
        for (chunk_index, _) in rows.chunks(max_rows_per_insert).enumerate() {
            if fail_chunk == Some(chunk_index) {
                return Err(WriteBatchError::Other(format!(
                    "sub-statement {chunk_index} failed"
                )));
            }
        }

        self.staging
            .lock()
            .unwrap()
            .entry(staging_table.to_owned())
            .or_default()
            .extend_from_slice(rows);
        Ok(self
            .affected_rows
            .lock()
            .unwrap()
            .unwrap_or(rows.len() as u64))
    }

    fn atomic_swap(
        &self,
        request: &AtomicSwapRequest,
    ) -> Result<AtomicSwapResult, AtomicSwapError> {
        self.swap_requests.lock().unwrap().push(request.clone());
        if let Some(error) = self.swap_error.lock().unwrap().take() {
            return Err(error);
        }

        let staged_rows = self
            .staging
            .lock()
            .unwrap()
            .get(&request.staging_table)
            .map_or(0, |rows| rows.len() as u64);
        let count_ms = *self.count_ms.lock().unwrap();
        let counts = RowCounts {
            source_rows: request.source_rows,
            staged_rows,
            source_batches: request.source_batches,
            received_batches: request.received_batches,
        };
        if !counts.verdict().passed() {
            return Err(AtomicSwapError::VerifyFailed {
                staged_rows,
                count_ms,
            });
        }

        let key_indices = request
            .primary_key
            .iter()
            .map(|key| {
                request
                    .columns
                    .iter()
                    .position(|column| column.eq_ignore_ascii_case(key))
                    .expect("the service precheck keeps primary keys among selected columns")
            })
            .collect::<Vec<_>>();
        let rows = self.staging.lock().unwrap()[&request.staging_table].clone();
        let mut target_rows = self.target_rows.lock().unwrap();
        for row in rows {
            let key = serde_json::to_string(
                &key_indices
                    .iter()
                    .map(|index| &row[*index])
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            target_rows.insert((request.target_table.clone(), key), row);
        }

        Ok(AtomicSwapResult {
            staged_rows,
            purged_rows: *self.purged_rows.lock().unwrap(),
            // No upsert here, so nothing can be counted twice: the interval
            // `swap_rows_in_range` allows for is a MySQL `affected_rows` fact,
            // not something a test double should invent.
            swapped_rows: staged_rows,
            count_ms,
        })
    }

    fn drop_staging(&self, staging_table: &str) -> Result<(), DropStagingError> {
        if let Some(error) = self.drop_error.lock().unwrap().take() {
            return Err(error);
        }
        self.staging.lock().unwrap().remove(staging_table);
        self.dropped.lock().unwrap().push(staging_table.to_owned());
        Ok(())
    }
}

/// The `datetime` target column the commit-path tests share, primary-key shaped:
/// a primary-key column must be `NOT NULL` (ADR-0035 §2, rule 3).
pub fn datetime_target_column(name: &str) -> TargetColumn {
    TargetColumn {
        name: name.to_owned(),
        column_type: "datetime".to_owned(),
        data_type: "datetime".to_owned(),
        precision: None,
        scale: None,
        length: None,
        datetime_precision: Some(0),
        nullable: false,
        character_set: None,
        ordinal: 1,
        default_value: None,
        extra: String::new(),
    }
}
