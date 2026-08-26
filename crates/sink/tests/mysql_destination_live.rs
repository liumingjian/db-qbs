//! The three [`MysqlDestination`] behaviours that only a real MySQL can answer (#196).
//!
//! Everywhere else a test double stands in for this destination, and what the
//! doubles substitute away is exactly the part that talks to MySQL. Three facts
//! were therefore believed only by reading the implementation:
//!
//! 1. `CLIENT_FOUND_ROWS` makes an existing row whose values did not change count
//!    as 1 rather than 0 (#138) — the whole reason `swap_rows_in_range` may put its
//!    lower bound at `staged_rows` instead of 0;
//! 2. the swap's `affected_rows` lands in `[staged, 2×staged]` (ADR-0035 §4), and
//!    **both ends are reachable**, which is why the judgement is an interval;
//! 3. the Connection Ritual hangs off the pool's connection-creation hook, so the
//!    **second** connection the pool opens comes up configured rather than bare.
//!
//! **Why these three and not the rest of that file** is argued once, in the module
//! note at the top of `src/mysql_destination.rs`; this file does not repeat it.
//!
//! **`#[ignore]` rather than a runtime skip**: a test that decides for itself to do
//! nothing still reports `ok`, and "3 passed" for three tests that never opened a
//! socket is the one outcome worth avoiding here. Ignored tests are counted as
//! ignored, so a plain `cargo test` tells the truth on a machine with no docker.
//!
//! ## The environment it is pointed at
//!
//! All five of `DB_QBS_TEST_MYSQL_HOST` / `_PORT` / `_USER` / `_PASSWORD` /
//! `_DATABASE` are **required**: these tests only run when someone asked for them
//! by name, so a missing one is a broken environment and says so, and defaulting
//! any of them risks running against the wrong database. [`RIG_SCRIPT`] sets all
//! five and is the intended way in.
//!
//! Every table this file creates is prefixed with a per-test unique name and
//! dropped on the way out, so it is safe to point at a database that has other
//! things in it.

use std::env;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use db_qbs_shared::swap_rows_in_range;
use db_qbs_sink::{
    build_staging_ddl, check_connection_settings, AtomicSwapRequest, AtomicSwapResult, Destination,
    MysqlDestination, TargetConnection,
};
use mysql::prelude::Queryable;
use mysql::{Conn, Opts, OptsBuilder};

/// What the rig script runs, quoted in the message a missing variable produces.
const RIG_SCRIPT: &str = "docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh";

/// The `CLIENT_FOUND_ROWS` half of #138.
///
/// A re-run that stages exactly what is already in the target table is the case
/// the flag exists for: MySQL's default counts such a row as 0, and a swap that
/// reports 0 against 3 staged rows falls out of `[staged, 2×staged]` and is
/// rejected as `SWAP_FAILED` — a correct run refused. The control assertion at
/// the end is the same upsert over a connection **without** the flag, so the test
/// says which of the two connections is responsible rather than merely that the
/// number came out right.
#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn a_rerun_that_changes_nothing_is_still_counted_row_for_row() {
    let mut rig = Rig::open("found_rows");
    let rows = [("1", "a"), ("2", "b"), ("3", "c")];

    let first = rig.stage_and_swap("r1", &rows).expect("first swap");
    assert_eq!(first.staged_rows, 3);
    assert_eq!(first.swapped_rows, 3, "three fresh rows are three inserts");

    let rerun = rig
        .stage_and_swap("r2", &rows)
        .expect("a re-run that changes nothing must not be refused");
    assert_eq!(rerun.staged_rows, 3);
    assert_eq!(
        rerun.swapped_rows, 3,
        "with CLIENT_FOUND_ROWS an unchanged existing row counts as matched, not as 0"
    );

    let without_the_flag = rig.upsert_over_a_plain_connection(&rows);
    assert_eq!(
        without_the_flag, 0,
        "the control: without CLIENT_FOUND_ROWS MySQL counts an unchanged matched row as 0"
    );
    assert!(
        !swap_rows_in_range(3, without_the_flag),
        "and 0 against 3 staged rows is outside the interval, which is the spelling of \
         \"remove the flag and it starts refusing correct runs\""
    );
}

/// The interval of ADR-0035 §4, at both of its ends and once in between.
///
/// An equality assertion would be wrong at either end, and the swap only ever
/// reports one number, so the three runs below are the evidence that the interval
/// is the right shape: insert-only sits on the lower bound, an all-values-changed
/// re-run sits on the upper one, and a realistic mixture sits strictly inside.
#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn the_swap_interval_reaches_both_of_its_ends() {
    let mut rig = Rig::open("interval");

    let inserts = rig
        .stage_and_swap("r1", &[("1", "a"), ("2", "b"), ("3", "c")])
        .expect("insert-only swap");
    assert_eq!((inserts.staged_rows, inserts.swapped_rows), (3, 3));

    let updates = rig
        .stage_and_swap("r2", &[("1", "a2"), ("2", "b2"), ("3", "c2")])
        .expect("all-values-changed swap");
    assert_eq!(
        (updates.staged_rows, updates.swapped_rows),
        (3, 6),
        "ON DUPLICATE KEY UPDATE counts a row that really changed as 2"
    );

    let mixed = rig
        .stage_and_swap("r3", &[("1", "a3"), ("2", "b3"), ("4", "d")])
        .expect("mixed swap");
    assert_eq!(
        (mixed.staged_rows, mixed.swapped_rows),
        (3, 5),
        "two changed rows and one new one: 2 + 2 + 1"
    );

    for result in [inserts, updates, mixed] {
        assert!(
            swap_rows_in_range(result.staged_rows, result.swapped_rows),
            "{result:?} must satisfy the judgement both ends share"
        );
    }
}

/// The Connection Ritual is the pool's, not the first connection's.
///
/// Nothing else asserts this. The ritual runs inside the pool's connection
/// creation, so the way to reach the second connection is to make the first one
/// unavailable: one thread parks it inside a statement that blocks, which leaves
/// the pool no choice but to open a new connection for the probe. That connection's
/// own session state then goes to the same [`check_connection_settings`] the ritual
/// uses — the test states no thresholds of its own.
///
/// **The occupier blocks on a lock this test holds, rather than on a sleep.** A
/// sleep would put a clock on the window: finish early and the probe pops the
/// freed connection instead, and a passing ritual reads as a regression. The lock
/// makes the window close when this test says so and not before.
#[test]
#[ignore = "needs a real MySQL; run docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh"]
fn the_second_connection_the_pool_opens_has_been_through_the_ritual() {
    let rig = Rig::open("ritual");

    let first = rig.probe_the_connection_in_use("p1");

    let second = thread::scope(|scope| {
        rig.take_the_lock();
        let occupier = scope.spawn(|| rig.on_a_pooled_connection(&rig.wait_for_the_lock()));
        rig.wait_until_the_pooled_connection_is_blocked();
        let second = rig.probe_the_connection_in_use("p2");
        rig.release_the_lock();
        occupier.join().expect("occupying thread panicked");
        second
    });

    assert_ne!(
        second.connection_id, first.connection_id,
        "the probe must have landed on a connection the pool opened for it, \
         not on the one the occupier is holding"
    );
    check_connection_settings(
        &second.character_set_client,
        &second.character_set_connection,
        &second.character_set_results,
        &second.sql_mode,
        second.max_allowed_packet,
    )
    .expect("the second connection the pool opened came up bare");
}

/// What one connection reports about itself.
#[derive(Debug)]
struct ConnectionProbe {
    connection_id: u64,
    character_set_client: String,
    character_set_connection: String,
    character_set_results: String,
    sql_mode: String,
    max_allowed_packet: u64,
}

/// One target table, one destination pointed at it, and one connection of the
/// test's own — outside the pool, and on the driver's plain defaults, which is to
/// say without `CLIENT_FOUND_ROWS`.
struct Rig {
    destination: MysqlDestination,
    database: String,
    /// Every table this rig creates starts with these characters, which is how
    /// [`Rig::drop`] finds them again without keeping a list.
    prefix: String,
    target_table: String,
    plain_opts: Opts,
    plain: Conn,
    /// The connection holding the occupier's lock, if one is held. A MySQL named
    /// lock lives and dies with the connection that took it, so releasing is
    /// `take()`ing this — and the whole rig can hand it out behind `&self`.
    locker: Mutex<Option<Conn>>,
}

impl Rig {
    fn open(label: &str) -> Self {
        let target = TargetConnection {
            host: required("DB_QBS_TEST_MYSQL_HOST"),
            port: required("DB_QBS_TEST_MYSQL_PORT")
                .parse()
                .expect("DB_QBS_TEST_MYSQL_PORT must be a port number"),
            username: required("DB_QBS_TEST_MYSQL_USER"),
            password: required("DB_QBS_TEST_MYSQL_PASSWORD"),
            database: required("DB_QBS_TEST_MYSQL_DATABASE"),
        };

        let plain_opts = Opts::from(
            OptsBuilder::new()
                .ip_or_hostname(Some(target.host.clone()))
                .tcp_port(target.port)
                .user(Some(target.username.clone()))
                .pass(Some(target.password.clone()))
                .db_name(Some(target.database.clone())),
        );
        let mut plain = Conn::new(plain_opts.clone()).expect("the test's own MySQL connection");

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before the epoch")
            .as_nanos();
        let prefix = format!("qbs_live_{label}_{}", stamp % 1_000_000_007);
        let target_table = format!("{prefix}_t");
        plain
            .query_drop(format!(
                "CREATE TABLE `{}`.`{target_table}` (\
             `K` VARCHAR(16) NOT NULL, `V` VARCHAR(32) NOT NULL, PRIMARY KEY (`K`)\
             ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
                target.database
            ))
            .expect("creating the target table");

        let destination = MysqlDestination::connect(&target).expect("connecting the destination");
        Self {
            destination,
            database: target.database.clone(),
            prefix,
            target_table,
            plain_opts,
            plain,
            locker: Mutex::new(None),
        }
    }

    /// One whole run against the target table: staging table from the target's own
    /// metadata, one batch, one swap, staging table dropped. The production chain,
    /// minus the HTTP in front of it.
    fn stage_and_swap(
        &mut self,
        run: &str,
        rows: &[(&str, &str)],
    ) -> Result<AtomicSwapResult, String> {
        let staging_table = format!("{}_stg_{run}", self.prefix);
        let columns = self
            .destination
            .target_columns(&self.target_table)
            .expect("reading target columns");
        let ddl = build_staging_ddl(&self.database, &staging_table, &columns);
        self.destination
            .create_staging(&staging_table, &ddl)
            .map_err(|error| format!("{error:?}"))?;

        let names: Vec<String> = columns.iter().map(|column| column.name.clone()).collect();
        let values: Vec<Vec<Option<String>>> = rows
            .iter()
            .map(|(key, value)| vec![Some((*key).to_owned()), Some((*value).to_owned())])
            .collect();
        self.destination
            .write_batch(&staging_table, &names, &values, 100)
            .map_err(|error| format!("{error:?}"))?;

        let result = self
            .destination
            .atomic_swap(&AtomicSwapRequest {
                run_id: format!("{}_{run}", self.prefix),
                staging_table: staging_table.clone(),
                target_table: self.target_table.clone(),
                primary_key: vec!["K".to_owned()],
                columns: names,
                source_rows: rows.len() as u64,
                source_batches: 1,
                received_batches: 1,
            })
            .map_err(|error| format!("{error:?}"));
        self.destination
            .drop_staging(&staging_table)
            .expect("dropping the staging table");
        result
    }

    /// The swap's own upsert, over the connection the test owns — which was built
    /// on the driver's defaults, so without `CLIENT_FOUND_ROWS`. Returns MySQL's
    /// `affected_rows`.
    ///
    /// `INSERT … SELECT … FROM` a staging table, not `VALUES`, because that is the
    /// form `build_swap_upsert_statement` produces. The builder is private to the
    /// destination, so the shape is matched by hand here; what differs between the
    /// two connections is the flag, and the flag counts matched rows the same way
    /// whichever form issued the upsert.
    fn upsert_over_a_plain_connection(&mut self, rows: &[(&str, &str)]) -> u64 {
        let staging_table = format!("{}_control", self.prefix);
        let values = rows
            .iter()
            .map(|(key, value)| format!("('{key}','{value}')"))
            .collect::<Vec<_>>()
            .join(",");
        for statement in [
            format!(
                "CREATE TABLE `{}`.`{staging_table}` LIKE `{}`.`{}`",
                self.database, self.database, self.target_table
            ),
            format!(
                "INSERT INTO `{}`.`{staging_table}` (`K`,`V`) VALUES {values}",
                self.database
            ),
        ] {
            self.plain
                .query_drop(statement)
                .expect("staging the control rows");
        }

        self.plain
            .query_drop(format!(
                "INSERT INTO `{}`.`{}` (`K`, `V`) SELECT `K`, `V` FROM `{}`.`{staging_table}` \
                 ON DUPLICATE KEY UPDATE `V` = VALUES(`V`)",
                self.database, self.target_table, self.database
            ))
            .expect("the control upsert");
        self.plain.affected_rows()
    }

    /// Runs one statement of the test's own on a connection out of the pool.
    ///
    /// `create_staging` is the only door the `Destination` interface leaves for
    /// this: it hands its second argument straight to the connection, and the
    /// MySQL implementation never looks at the staging-table name it is given, so
    /// nothing here has to be a staging table.
    fn on_a_pooled_connection(&self, statement: &str) {
        self.destination
            .create_staging("not a staging table", statement)
            .expect("running a statement on a pooled connection");
    }

    /// Runs a statement through the pool and reads back the session state of
    /// whichever connection served it.
    fn probe_the_connection_in_use(&self, label: &str) -> ConnectionProbe {
        let table = format!("{}_{label}", self.prefix);
        self.on_a_pooled_connection(&format!(
            "CREATE TABLE `{}`.`{table}` AS SELECT \
             CONNECTION_ID() AS connection_id, \
             @@character_set_client AS character_set_client, \
             @@character_set_connection AS character_set_connection, \
             @@character_set_results AS character_set_results, \
             @@SESSION.sql_mode AS sql_mode, \
             @@max_allowed_packet AS max_allowed_packet",
            self.database
        ));

        let mut reader = self.new_plain_connection();
        let row: (u64, String, String, String, String, u64) = reader
            .query_first(format!("SELECT * FROM `{}`.`{table}`", self.database))
            .expect("reading the probe")
            .expect("the probe table must have one row");
        ConnectionProbe {
            connection_id: row.0,
            character_set_client: row.1,
            character_set_connection: row.2,
            character_set_results: row.3,
            sql_mode: row.4,
            max_allowed_packet: row.5,
        }
    }

    /// The named lock the occupier parks on. Named after the rig, so two runs
    /// against the same MySQL never wait on each other.
    fn lock_name(&self) -> String {
        format!("{}_occupied", self.prefix)
    }

    fn take_the_lock(&self) {
        let mut locker = self.new_plain_connection();
        let held: Option<u64> = locker
            .query_first(format!("SELECT GET_LOCK('{}', 10)", self.lock_name()))
            .expect("taking the lock");
        assert_eq!(held, Some(1), "the rig's own lock must be free");
        *self.locker.lock().expect("lock-holder mutex poisoned") = Some(locker);
    }

    /// The statement the occupier runs: it returns only once [`Rig::release_the_lock`]
    /// has been called, and holds its pooled connection until then.
    fn wait_for_the_lock(&self) -> String {
        format!("DO GET_LOCK('{}', 60)", self.lock_name())
    }

    fn release_the_lock(&self) {
        self.locker
            .lock()
            .expect("lock-holder mutex poisoned")
            .take();
    }

    /// Blocks until the occupying statement is visibly waiting, which is the moment
    /// the pool's only idle connection is provably gone. Polling the process list
    /// rather than sleeping a fixed while: the point is *that* the connection is
    /// taken, not how long it took.
    fn wait_until_the_pooled_connection_is_blocked(&self) {
        let mut watcher = self.new_plain_connection();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let waiting: Option<u64> = watcher
                .query_first(
                    "SELECT COUNT(*) FROM information_schema.PROCESSLIST \
                     WHERE INFO LIKE 'DO GET\\_LOCK%'",
                )
                .expect("reading the process list");
            if waiting.unwrap_or(0) > 0 {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the occupying statement never showed up in the process list"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn new_plain_connection(&self) -> Conn {
        Conn::new(self.plain_opts.clone()).expect("a further MySQL connection")
    }
}

impl Drop for Rig {
    /// Cleanup never panics — a failing test must report its own failure, not be
    /// buried under a double panic while unwinding — but it never fails silently
    /// either: whatever is left behind is named on stderr so the next run's
    /// leftovers have an explanation.
    fn drop(&mut self) {
        let mut left_behind = Vec::new();
        let tables: Vec<String> = match self.plain.query(format!(
            "SELECT TABLE_NAME FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = '{}' AND TABLE_NAME LIKE '{}%'",
            self.database, self.prefix
        )) {
            Ok(tables) => tables,
            Err(error) => {
                eprintln!("!! {} left everything behind: {error}", self.prefix);
                return;
            }
        };
        for table in tables {
            if let Err(error) = self.plain.query_drop(format!(
                "DROP TABLE IF EXISTS `{}`.`{table}`",
                self.database
            )) {
                left_behind.push(format!("table {table}: {error}"));
            }
        }
        // The swap writes the ledger, and the ledger outlives the staging table.
        if let Err(error) = self.plain.query_drop(format!(
            "DELETE FROM `{}`.`__db_qbs_write_ledger` WHERE target_table LIKE '{}%'",
            self.database, self.prefix
        )) {
            left_behind.push(format!("ledger rows: {error}"));
        }
        if !left_behind.is_empty() {
            eprintln!("!! {} left behind {}", self.prefix, left_behind.join("; "));
        }
    }
}

/// These tests only run when asked for by name, so a missing variable is a broken
/// environment rather than a reason to do nothing quietly.
fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| {
        panic!("{name} is not set — these tests need a real MySQL; {RIG_SCRIPT} brings one up")
    })
}
