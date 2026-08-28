# CONTEXT — db-qbs

A data query-and-import service. The user queries Oracle on the **source** side and bulk-loads
the result into MySQL on the **target** side. The two sides are network-isolated and neither
database port faces the public internet; the source side may open outbound HTTPS to the target.

**This file states the present only.** Every sentence describes the current shape of the code.
It does not record what something used to be or when it changed — that history lives in git and
in the decision tickets. Do not write it back in here.

> **Positioning**: the long-term goal is a general offline database import tool, but **the first
> release's scope is set by what the customer actually needs**. Shapes the customer does not have
> may be left out; **narrowing a capability boundary must land as an explicit ADR** rather than
> taking effect quietly inside a ticket — especially when the defence being removed guards against
> **silent value corruption** (no error raised, the write succeeds, the data is already wrong).
> Once a capability boundary is in question, judge it by "type surface + a real round-trip against
> the target", never by how often a given production database happens to hit it. The local Docker
> rig is a **legitimate verification environment for those judgements, not a fallback**.

> **"Offline" constrains the database hosts only.** The machines running the `source` and `sink`
> **service processes may reach the internet**, and they talk to each other over it. "The source
> side cannot install a toolchain" is therefore not a usable argument.

## Deployment shape

```
[source network]                                [target network]
┌──────────────────────────┐                  ┌──────────────────────┐
│ source (web service)      │  outbound HTTPS  │ sink ＝ target agent  │
│  ├─ web UI               │  ───────────────▶│  ├─ bulk write MySQL  │
│  ├─ SQL builder          │                  │  ├─ target metadata   │
│  ├─ agent registry       │                  │  └─ /v1/agent/info    │
│  ├─ streaming Oracle read│                  │  listen               │
│  └─ orchestration/verify │                  └──────────┬───────────┘
│  listen (inbound face)    │                            ▼
└──────────┬───────────────┘                          [MySQL]
           ▼
       [Oracle]  ← some tables reach further databases via dblink
```

**`source` requires a login; `sink` does not.** Every `/api/*` request `source` answers needs a
live **Session** — see the glossary entry for what that door is and is not. It covers **one HTTP
face out of two**: anyone who can connect to the `sink` port still holds `DELETE` on the target
database and can truncate and rewrite any staging or target table, bypassing `source` entirely.
The single account's password also defaults to `admin` and never expires, so on an untouched
deployment "can reach the `source` port" and "is inside" are two keystrokes apart.

**Deployment premise, unchanged by the login**: both `listen` addresses bind loopback only by
default. When several people need access, the deployer puts a reverse proxy in front and terminates
TLS there — **the product is unaware of it, validates nothing about it, and relaxes no internal
constraint because of it**, which is why it is absent from the diagram above. The session cookie
carries no `Secure` flag, because these deployments speak plain HTTP; over an untrusted link the
password and the ticket are both readable, and TLS on the proxy is the only answer to that.

`source` and `sink` are separately deployed processes that communicate only over HTTP. `sink` knows
nothing about Oracle. **`source` opens no MySQL connection at all**, but it does **hold and forward
target credentials**: both kinds of datasource live in a SQLite database on the source side, and the
target connection details cross the wire with `POST /v1/runs`. **The MySQL password crosses that
channel in cleartext**, so **the channel an agent address points at must be trusted** (same host, a
trusted internal network, or TLS/tunnelling the deployer builds) — otherwise the password is being
published to the internet.

Both sides are written in **Rust** with synchronous blocking IO. `source` reaches Oracle **11g**
through the `oracle` crate (ODPI-C), so the source machine must carry a full **Oracle Instant
Client 19c Basic** bundle (brought in offline, no root required). The target is **MySQL 5.7 or
8.0**, `utf8mb4` throughout.

**5.7 is an addition to the support matrix, not a replacement for 8.0** — a great many sites have
not upgraded, and "cannot connect" is not an answer to give them. Nothing in the SQL had to move:
no CTE, no window function, and the upsert was already in the form 5.7 accepts. Two things did:

- **`max_allowed_packet` must be raised on 5.7.** Its stock value is 4 MiB against the Connection
  Ritual's hard 64 MiB, so *every untuned 5.7 instance* is judged unusable on its first run. The
  gate is not relaxed — see **Connection Ritual** — so the target's DBA raises it, and the refusal
  message carries the `SET GLOBAL` command and the `my.cnf` stanza to make it stick.
- **Auto-increment is read from `EXTRA` by containment, never by equality.** 8.0 also writes
  `DEFAULT_GENERATED` into that column and 5.7 never does; comparing the whole value against an
  8.0-only spelling made every 5.7 auto-increment column read as ordinary.

The same live round-trip suite is pointed at both versions in turn
(`docs/spikes/fixtures/local-rig/scripts/run-mysql-destination-live.sh both`); nothing in it
branches on the version.

## Glossary

**Target Agent**
   The `sink` process on the target host, a **first-class concept** in the product. It has a name,
   an address, and an identity that is **stable across restarts** (the `agent_id` it reports from
   `GET /v1/agent/info`, persisted in an `agent-id` file next to `sink.toml`).

   **The target database is reachable only through it**: connection test, table listing, column
   fetch, and the writes of a run — all four paths, no exceptions and **no fallback**. If the agent
   is offline or its identity does not match, the operation fails on the spot. Every MySQL datasource
   **must** select an agent. Registration is source-initiated (it is stored only once a probe
   succeeds); liveness is maintained by a background probe every 15 seconds plus an immediate probe
   before each use. **An identity mismatch does not count as online** — "a different agent is
   answering at this address" and "nothing is running" are two distinct incidents, reported separately.

   The agent also reports **which MySQL it is connected to**: `@@version`, and the default collation
   of `utf8mb4` on that server. `source` opens no MySQL connection of its own, so the agent is the
   **only** source of this information, and the generated **target DDL** depends on it.
   The report is a **cache, not configuration**: the agent holds no target credentials of its own
   (they arrive with each run), so it learns the answer only on a path that carries them — a target
   check or the opening of a run — and repeats the last thing it observed afterwards. Before it has
   ever observed one, and for agents built before this was reported at all, the answer is **unknown**,
   and unknown is reported as unknown. **Nothing downstream may substitute a guess for it.**

**Import Task**
   One complete job: query a batch of data from Oracle, move it into one MySQL table.
   **The idempotence promise is no longer unconditional.** A task whose target table has a unique
   constraint is re-runnable in the old sense — running **the same task definition** twice leaves the
   target table in an identical state, because consistency comes from **upserting on the primary
   key** and idempotence rests on that constraint. A task whose target table has **no** unique
   constraint writes a plain `INSERT ... SELECT`, and running it twice **doubles the data**. That is
   the first place in the product where the same task definition, run twice, leaves two different
   target states. It is allowed on purpose (#261) — "append this query into a flow table" had no
   outlet otherwise — and it is never silent: the precheck says it in its conclusion, the task
   definition records it, and the task list and run detail both carry a visible marker.

**Task Definition**
   A **structured spec** (table, columns, filter clause, primary key, **write mode**, **schedule**).
   **It is the
   single source of
   truth, and the source SQL is generated from it.** **The SQL is not stored in the task definition**
   — it is recomputed on demand, and only pinned as a snapshot of what actually ran on each
   run-history row. **Only `source` reads it; `sink` is unaware it exists.**
   The **write mode** is what the author chose (today only "append"); the **write statement** is what
   the target end actually runs, and nobody chooses that — it follows from one fact alone, whether
   the target table has a unique constraint. An **empty primary key is a value, not a blank**: it is
   the definition's record of "the target table had nothing to merge on". If the target table's key
   situation has moved since, the two derivations disagree and **the run fails**; the statement kind
   is never switched silently under an unchanged definition.

**Schedule**
   Two fields on the task definition: a **five-field cron expression** and an **enable switch**.
   They are two fields because they are two things — clearing the expression to pause a task would
   make someone throw away the line they wrote. An enabled task with no expression is a
   contradiction and is **refused when the task is saved**, as is any expression the parser cannot
   read; the reason it gives is the sentence the person sees, never an error code.

   The expression is stored **as written**. The parser is **hand-written and depends on nothing**:
   the packaging chain is an offline cross-compile, and the only forms in play are `*`, `a`, `a-b`,
   `*/n`, `a-b/n` and comma lists, so `L`, `W`, `#` and seconds fields would all be dead weight. It
   is a **pure function** — an expression plus an instant in, the next fire time out — which is what
   lets the whole semantics be pinned by a table of cases. There is exactly one surprising rule and
   it is Vixie cron's: when **both** the day-of-month and the day-of-week field are restricted, they
   are **or**-ed, not and-ed.

   **The timezone is the server's local timezone**, and the interface states it. The machine running
   `source` is the one that will fire the run, so its wall clock is the only meaningful answer; the
   browser's would be a different two o'clock. The interface therefore shows the timezone and the
   **next fire times** beside the expression, computed by the server through the same parser that
   refuses a bad expression at save time — that read-out is what makes the parser's semantics
   visible instead of assumed.

   **Nothing fires yet.** This is the configuration half; the loop that acts on it is a later ticket.

**Task Draft**
   A Task Definition **while it is being built**, plus everything the interface needs to judge it:
   which fields the person typed or ticked by hand, which were derived for them (same-name mapping,
   inferred primary key, prefilled target table name, generated task name), and the results fetched
   along the way (the ten-row preview, the target-table check).

   It is **never persisted** — leaving the wizard discards it, and re-entry is through the saved task,
   not through a draft. Its rules live in one module (`web/src/wizard.ts`), never in the screens:
   what a change clears, whether that clearing is worth a confirmation, when a fetched result goes
   stale, and whether the next step may be entered. A **hand-made** value is one the person typed or
   ticked, or one loaded from a saved task when editing; a derived value is not. A cascading change is
   worth a confirmation only when it clears a hand-made value. **Leaving is always worth one** — whether
   anything in the draft is worth keeping is the person's judgement, not the wizard's; hand-made values
   are merely what that question can *list*, and listing nothing is not a reason to skip it.

**Filter Clause**
   One field, `where_clause`: **a free-form fragment spliced verbatim after `WHERE`**, not including
   the word `WHERE` itself. Blank means no `WHERE` at all, i.e. read the whole table. It is **not
   parsed, not rewritten, not reversed** — whether it runs is Oracle's call, reported when it runs.
   The single rule it must pass is **no semicolon**: that is not injection defence (the text is the
   user's own SQL) but a ban on **statement splicing**, which would make the previewed query and the
   executed query two different queries. Table and column names are still whitelisted as unquoted
   Oracle identifiers — they come from the interface's own selections; the filter clause is the
   deliberate exception, because it is authored rather than generated.

**Custom SQL**
   An optional hand-written SELECT inside a task definition — the second data-retrieval path beside
   "pick a table". **It is an input field of the spec, not an authority**: at execution it is wrapped
   as an **opaque subquery** inside the generated projection
   (`SELECT q.<source column> AS <target field> FROM ( it ) q`), so column selection, field mapping,
   and the primary key are still the spec's call, and **it is never executed verbatim**. The system
   neither parses it nor reverses it back into a structured model.
   Three constraints: no separate filter clause may be attached (write the filtering into the SQL
   itself), no dblink may be set at the same time, and every result column must carry an unquoted
   identifier name (expression columns need their own alias).
   **Do not call it "editable SQL"** — what is hand-written is the inner subquery; the generated
   statement itself is always read-only and always recomputed.

**Run**
   One actual execution of a task definition, with a unique `run_id` and a state. **Starting one takes
   no input beyond the task's identity** — clicking start runs it; there is no dialog and there are no
   parameters. A re-run produces a new `run_id` and never reuses the old one. The mutual-exclusion key
   is the task: **one task may not have two runs in flight**, and the 409 says exactly that.
   **The state lives only in `source`, and only in process memory** — see **Run Stage**. `sink`
   holds no run state, only the resource lifetime of the staging table. When the source process dies,
   the run ceases to exist.

**Run Stage**
   What the run process is doing right now: five values, `PREPARING` / `STREAMING` / `COMMITTING` /
   `SUCCEEDED` / `FAILED`, named after the work rather than after a verdict. **It is a closed set with
   one definition** (`crates/shared/src/run_stage.rs`), spelled once and pinned by a test, because the
   name crosses a process line: the child writes it into a `stage_changed` Run Log line and the parent
   reads it back. Those spellings are the wire contract and never change.

   **Abort permission hangs off it and has no second implementation**: `PREPARING` and `STREAMING` may
   still be stopped, the other three may not. Both ends evaluate that one rule, so the screen greys the
   stop button rather than learning of the refusal from a 409. **Liveness does not hang off it** — a
   run history row is in flight by its own outcome, not by its stage, and a row carrying a finish time
   without a verdict is not in flight but of unknown outcome.

   The two terminal values share their spelling with a Run History row's `outcome`, which is the
   *verdict* over the same two words. They are separate vocabularies that happen to agree; nothing
   converts between them. A spelling the reader cannot place is **shown as it arrived, never
   swallowed** — it means the two ends are on different versions, which is exactly what you want on
   screen — but nothing is ever decided from it: unrecognised reads the same as absent.

**Batch**
   The smallest unit pushed within a run: **5000 rows or 16 MiB, whichever comes first**.
   It is **purely a push-side split** — beyond "which run, which segment" it carries no data
   identity, and the protocol conveys no boundary information. The sequence number increases
   monotonically from 1 and is used only for **ordering assertions and diagnostics**.

**Staging Table**
   `<target>__stg_<run_id>`, created on the target for each run. Every batch lands in the staging
   table first and is atomically swapped into the target table only after verification passes.
   **Its structure is generated from the mapping precheck's result** — each column typed after the
   target table, all nullable, no indexes, no primary key. A failure to create it fails the whole
   run, and **"table already exists" is never resolved by dropping and recreating**. Orphan tables
   left by a crash are cleaned by hand, found via the `__stg_` marker and the timestamp in the name.

**`run_id`**
   The unique identifier of a run, shaped `<14-digit UTC timestamp>_<6 hex chars>`
   (e.g. `20260813091530_a3f19c`, 21 characters). **There is exactly one such form across the whole
   chain** — the protocol, the logs, and the staging table name all carry the same string.
   It sets the budget for the target table name: MySQL caps identifiers at 64 characters, and after
   subtracting `__stg_` and the `run_id`, **a target table name longer than 37 characters is rejected
   by the precheck**.

**Abort**
   The cleanup action by which `source`, on hitting an error, tells `sink` to discard the staging
   table. It is idempotent and **promises nothing about reliability** — a failed abort is logged and
   not retried. Whether it is still permitted is a property of the **Run Stage**, with one
   implementation both ends read. It exists to clear the most common leftover: "the process is
   still alive, this run just failed." **It is only ever sent before commit**: once `COMMITTING`
   is entered, the staging table's disposition has passed wholly to `sink` and source permanently
   forfeits the right to abort.
   Abort is not a state; it is an action on the `FAILED` path.

**Swap**
   Completed inside a single transaction on the target: an `INSERT ... SELECT` from the staging table
   into the target table, then `DROP` the staging table. The target table holds either all old data
   or all new data, never an intermediate state.
   **There is more than one write statement, and the target table picks it, not the author.** With a
   unique constraint it is `INSERT ... SELECT ... ON DUPLICATE KEY UPDATE` — merge on the key. With
   none it is the plain `INSERT ... SELECT`; the upsert clause is omitted rather than carried along,
   because a clause that can never fire only misleads whoever reads the statement.
   **The row-count adjudication forks with it**: the upsert path accepts `[staged, 2×staged]`
   (MySQL counts an update as 2, and `CLIENT_FOUND_ROWS` puts the floor at `staged` rather than 0),
   while the plain insert demands **strict equality**. One pure function in `shared` decides both,
   and the statement is passed into it rather than sniffed at each call site.
   **Rows deleted at the source do not disappear at the target** — neither statement ever deletes.
   That is a deliberate debt.
   **The swap leaves nothing behind but the target table.** It writes no per-row record of what it
   wrote: the write ledger `sink` used to keep in the customer's own database
   (`__db_qbs_write_ledger`, one row per written primary key) is gone, and with it the "undo a run"
   action that was its only consumer (#256). The reasoning is in **Standing limits** item 11; the
   swap now drops that table when it finds one, so a deployment that has it sheds it on the next
   run.

**Tombstone**
   An in-memory record `sink` keeps for a finished run so that "what happened?" still has an answer
   after the swap. It records the **resource's final state**, not the run's, and has only two values:
   `SWAPPED` (the target table has absorbed this run's rows — **merged by primary key**, per **Swap**
   above; it does not mean the table was replaced wholesale) and `DISCARDED` (the target table was
   never touched). **It is a diagnostic cache, not a state store** — only the most recent 32 are kept, and
   losing them costs no correctness, only diagnosability. **Run history does not absorb it**: a
   tombstone answers "did the target table move?", a question `source` asks and only `sink` can answer.

**Run Log**
   **JSON Lines** written to stdout by `source` and `sink` — one JSON object per line. Its main
   consumers are troubleshooting agents and the long-running parent process, not a human at a
   terminal: the prose still sits in the `message` field, but **the structure is in the fields, not
   in the formatting**. Six common fields — `ts` / `level` / `event` / `run_id` / `task` /
   `component` (the producing end: `source-orchestrator` / `source-run` / `sink`) — and `run_id`
   **may be `null`**. **The contract is "the field set is stable, the formatting is not"**; fields are
   only ever added, never removed and never redefined.
   Failure lines carry `column` and `value`, **so the logs contain business data**: business values
   can exist in four places on the source host (the child process's stdout, a file the deployer
   redirected it into, the run-history row, and the raw-line table described below), all held to
   0600, and **moving them off that host counts as exfiltration, which the product never does**.
   Logs go to stdout only; the program creates no files and rotates nothing.
   **The parent also keeps the child's raw lines**, verbatim, in the same local database as run
   history and related by run — because folding is lossy: an event the fold does not recognise is
   ignored by definition, so "where did last night's run get stuck?" cannot be answered from the
   folded row. Lines that are not even JSON are kept too, as they arrived. Three properties are
   load-bearing:
   **retention is the stricter of 7 days and the most recent 10 runs per task**, expired lines being
   dropped by the same purge-on-write the history table uses (no background task);
   **`value` is truncated to 64 characters before storage** — enough to tell which column went wrong,
   not enough to be a copy of the data, and a truncated line carries `value_truncated: true` so the
   display layer never reads half a value as a whole one;
   and they are read back **by cursor** (`GET /api/runs/{}/logs?after=<seq>`, session-guarded),
   returning only the lines after the cursor. **Polling, never a long connection** — this backend is
   a synchronous blocking stack with no async runtime, and one hanging connection would occupy a
   whole worker thread. Rendering a line as a sentence is the display layer's job; the wire format
   stays structured, or the parent would be regex-matching prose.

**Run History**
   A row the long-running `source` parent process writes for **every submission**, and the only basis
   on which the UI can answer "did the month-end run go through?". The parent builds it by parsing
   and aggregating the child process's run log; it shares a database with the task definitions.
   **It is a historical record, not a state store** — authoritative run state still lives only in the
   child's memory, and a history row is a **best-effort projection of the log**. Losing it costs no
   correctness, only traceability.
   Its identity is the **`run_record_id`** minted when the parent accepts the submission; `run_id` is
   a **nullable field** on the row, and `null` means the submission **never reached sink** (the
   precheck rejected it). Retention is by age, defaulting to 90 days.
   **Because it carries the `column` and `value` of failures, this SQLite file holds real business
   values sampled from the source database for 90 days. It ranks alongside the credential files and
   is held to 0600.**

**`run_record_id`**
   The identifier minted the moment the parent process accepts a submission; it is the primary key of
   run history. Its **extension differs from `run_id`'s**: `run_record_id` identifies "one submission
   the parent accepted", `run_id` identifies "one run `sink` knows about". When a submission fails the
   precheck, the former exists and the latter does not. **The two are always displayed together** and
   never substituted for one another.

**Connection Ritual**
   Four assertions every MySQL connection in `sink` must pass before it is handed to business code:
   all three connection-layer charset variables are `utf8mb4`, `sql_mode` is explicitly set to
   `STRICT_ALL_TABLES` and reads back equal, and `max_allowed_packet >= 64 MiB`. Any failure renders
   **the entire sink unusable** — not merely one run.
   **It hangs off the pool's connection-creation hook, not the top of the business code** — otherwise
   the second connection in the pool comes up bare. It concerns no particular column and happens
   before the mapping precheck, so it never appears in the precheck's per-column report.
   The pool behind it is a real one — **bounded, and reusing** — but that changed nothing about the
   ritual: the pool has exactly one place that creates a connection, and it runs the ritual
   unconditionally. First connection, Nth connection, and the replacement for one that stopped
   answering are all the same case. A pooled connection that fails its ping is **thrown away rather
   than reused**: keeping it would bet the ritual's session variables on the server's reconnect
   behaviour, which is precisely the uncertainty the ritual exists to remove. The four assertions are
   the definition of an agent being usable; **pooling is not a licence to skip them**.

   **The 64 MiB is a hard gate on both supported versions**; it is what stops a large batch being
   truncated at the protocol layer, which surfaces as a syntax error and sends whoever is on call
   digging through business data that is fine. MySQL 5.7's stock value is 4 MiB, so on 5.7 the
   refusal is the *normal* first outcome rather than a rare misconfiguration — which is why the
   message is written to be obeyed rather than investigated: it names the `SET GLOBAL` command and
   the `my.cnf` line, one to take effect now and one to survive the restart.

**Verification**
   A mandatory gate before the swap, not an optional step. **It compares the row count the source
   read against the row count actually landed in the staging table**; on failure the staging table is
   discarded and the target table is left alone. The source's commit seals the staging table, after
   which any batch write for that `run_id` is refused. Fidelity at the value level is guaranteed by
   the **mapping precheck**, not by verification.
   **Both numbers are pinned to one definition**: the source number is the **fetch-loop accumulator**
   (not a second `COUNT(*)`, which would be a different read-consistency snapshot), and the staging
   number is **`SELECT COUNT(*) FROM stg`** (not the sum of per-batch `rows_written` — the point of
   verification is to distrust the intermediate links). Batch counts take part in the gate at the same
   level, and the comparison happens **inside sink's swap transaction**, so that "the thing counted"
   and "the thing swapped" are the same snapshot.
   **Its real span is from "how much source claims it read" to "how much actually landed in staging"**
   — `source_rows` is self-reported and sink cannot audit it, so **the leg from the source database to
   the source accumulator falls outside the gate's coverage**.

**Canonical Form**
   The one determinate string representation a value taken from Oracle is normalised into before it
   is written to MySQL: numerics stripped of trailing zeros, dates in a fixed format, NULL with its
   own marker. It is the storage rule of the transfer chain.
   **Its on-the-wire representation is a separate matter**: every value in a payload is either a JSON
   string or `null`, `NULL` is JSON `null`, a `NUMBER` may never be transmitted as a JSON number, and
   everything is UTF-8.

**Mapping Precheck**
   The per-column check of "source column type → target column type" performed before a run is
   submitted. **It is a hard gate**: fail it and the run may not start. It rejects types outside the
   whitelist, `DECIMAL`s with insufficient precision, `NUMBER`s declared without precision, and
   `TIMESTAMP` scales beyond 6 digits.
   **The primary-key gate now applies to the upsert path only.** When the task ticks a primary key it
   still **hard-rejects a target table lacking a matching primary key or unique constraint** (without
   one, `ON DUPLICATE KEY UPDATE` silently degrades into a plain INSERT) — that rule is unchanged in
   force, only in scope. When the task ticks none, the mirror-image rule applies and is just as hard:
   the target table must **still** have no unique constraint. Either way this stays the **single**
   interception point; no other place gets to decide the statement kind.
   Passing is not the whole answer: the precheck also returns **conclusions**, which do not block but
   must be read. Today there is one — "目标表无主键 → 本任务为纯追加写，重跑会产生重复数据".
   It is the **only defence** against values being silently altered — and the only defence that can
   be **switched off wholesale** — see **Standing limits**.
   A column whose fit cannot be settled from metadata alone gets a **3.5th step, the range check**:
   sink names the columns and the target shape derived for each, source counts how many rows of the
   **real data** fall outside it, and **sink judges those counts** — source reports facts, never a
   verdict. This is the **one place in the system where sink asks source to run SQL**, because the
   distribution of the source's own data is the one thing sink cannot reach. It costs a second
   `POST /v1/runs`: the first answers "count these, then ask again", **creates no run and stores
   nothing**, so a range check that is never answered leaves nothing behind.
   **It is split in half by HTTP**: describing the source SQL happens in `source`, while reading target
   metadata, comparing column by column, and creating the staging table happen in `sink`. **`source`
   makes no per-column type judgement whatsoever** and reports the describe result verbatim, so that
   **all type judgement is concentrated in `sink`** — which is what makes "report every column at
   once" actually hold.

**Column Fetch**
   A **read-only, side-effect-free** action during task authoring: open a cursor against **the SQL the
   spec computes right now**, describe it, and bring back the columns the query will actually emit.
   A hint of `(p,s)` for a bare `NUMBER` arrives with the fetch request and is discarded after use —
   **it does not enter the task definition**. It **creates no run, mints no `run_id`, never touches
   `sink`, and writes to no store**; the result is purely transient and lost on refresh.
   It exists for one reason: the **target DDL** must be generated from authoritative metadata, and the
   data dictionary's copy is not authoritative.

**Target DDL**
   A `CREATE TABLE` statement handed to a person to run themselves. **The product does not create the
   target table**; it only generates this statement. `source` derives it **forward** from the source
   column metadata obtained by **column fetch** — inverting the mapping precheck's three rules
   determines it uniquely, and **no input from the target side is needed** (the table does not exist
   yet). **Primary key columns are `NOT NULL` with a `PRIMARY KEY (...)`, every other column
   nullable**, and `utf8mb4` is explicit.
   The **collation** is the one exception to "no input from the target side is needed": MySQL's
   default collation for `utf8mb4` differs between server versions, so it is taken from what the
   **target agent** reported (see *Target Agent*) and written out as an explicit `COLLATE`.
   When the agent reported nothing, the statement carries `DEFAULT CHARSET=utf8mb4` and **no
   `COLLATE` at all**, leaving the choice to the target server's own default — the behaviour that
   predates the report. Picking a collation on the agent's behalf is forbidden: a wrong one surfaces
   only much later, as comparisons and sorts quietly giving the wrong answer.
   **It grants no clearance of any kind**: after a person creates the table from it, the mapping
   precheck still runs from scratch and may still reject. There is exactly one interception point, and
   it is the mapping precheck. The **staging table**'s DDL is a different thing: that one is *copied*
   from the target table and generated by `sink`.

**Session**
   The ticket that lets a request through `source`'s HTTP face. There is **one account**, `admin`,
   with **no registration and no second administrator**; its password ships as `admin` and changes
   only when a person changes it. Failed logins are **not** rate-limited, cooled down, or locked
   out, and the interface never says that the factory password is still in use — all three are
   deliberate.

   The ticket is an opaque random token in an `HttpOnly; SameSite=Strict` cookie, stored in the
   same SQLite database as the tasks. It **survives a restart of `source`** — unlike run state,
   which lives only in process memory — so an upgrade does not sign everyone out. Expiry is
   **sliding**: a session dies after 8 idle hours, counted from its last request, not from login.
   Every authenticated response re-issues the cookie so the browser's copy slides with it.

   **Concurrent sessions are allowed**: the same account may be logged in from several browsers,
   and a new login evicts none of them. Two things do evict: signing out kills exactly the one
   ticket that asked, and **changing the password kills every session but the one that changed it**.
   The password is stored as an Argon2id hash; the session token is stored **as-is**, because read
   access to `data_dir` is already total compromise and a second digest would only add a step.

   Forgetting the password has exactly one way out, on the source host:
   `db-qbs-source reset-password --config <source.toml>`, which returns the password to `admin`
   and voids every session.

**Source API**
   Every HTTP request `source` answers goes through one function: `Api::handle(&Request) -> Response`
   in `crates/source/src/http.rs`. It lives in the **library, not the binary** — `server_main.rs` is
   under twenty lines and owns nothing but the process entry point. Tests therefore drive the whole
   API in-process; none of them spawns a process or opens a socket.

   **Authentication is a column on the route table, not a line in any handler**, and it is checked
   before dispatch. Exactly three routes are public — `GET`, `POST` and `DELETE` on `/api/session` —
   and everything else answers `401` without a session. A request that matches no route at all also
   answers `401` when unauthenticated rather than `404`, because the difference between the two is
   enough to enumerate the table from outside. Static assets stay public; the login page has to load.

   Routes are **data** (`routes()`), matched in two passes: literal patterns first, patterns carrying a
   `{}` placeholder second. **Declaration order in the table means nothing** —
   `/api/datasources/test-connection` cannot be swallowed by `/api/datasources/{}` however the table is
   written, and `/api/agents/{}/probe` cannot be swallowed by `/api/agents/{}`. A placeholder matches
   exactly one path segment: non-empty, no `/`. Adding a route without a test fails the suite, because
   `every_route_reaches_its_handler` reconciles its own table against `routes()`; adding one without
   declaring whether it is public fails `every_route_declares_its_access` the same way.

   **Failure has one shape**: `{"error": {"message": "..."}}`, on every route and every status.
   `kind` is an **optional field inside that envelope**, never a second shape beside it: it says who
   the operator has to go to next — their own input (`request`), the source database (`oracle`), the
   target agent or the sink behind it (`agent` / `sink`), or the generated target DDL (`target_ddl`).
   A screen that needs the attribution reads `kind`; one that does not treats it as absent. Anything
   extra a failure carries (`oracle_code`, `failure_kind`, the offending columns) rides **inside the
   same envelope**, so the web client parses one shell and never has to know which endpoint it called.
   Every request body is read by one function, `read_json_body` — one wording for an unreadable body,
   one for invalid JSON, and one 1 MiB cap, with no handler rolling its own.

   The process-level half is deliberately **outside** `Api`: listening and translating `tiny_http`,
   first-boot config migration, the background agent probe, SIGTERM, and sealing unfinished runs on
   restart all live in `crate::server::serve`. That half — and only that half — is what the binary-spawning
   sentinels in `tests/source_skeleton.rs` exist to prove.

**Sink API**
   `sink`'s HTTP face has the same shape, one entry point down: `Api::handle(&Request) -> Response`
   in `crates/sink/src/http.rs`, over the crate's own `Request`/`Response`. `tiny_http` is confined to
   three places in that file — `serve`'s listener loop, the `handle_request` that feeds it, and the
   translation pair at the bottom — and no route or handler knows it exists. Unlike `source`, `sink`
   keeps that process-level half in the same file rather than a `server.rs`; the file is 700 lines,
   not 2400. Tests drive all ten routes in-process (`crates/sink/tests/api.rs`), against
   `test_support::InMemoryDestination` behind the `SinkService` seam.

   Routes are **data** (`routes()`) and matched in the same two passes, literal before placeholder, so
   a literal such as `/v1/runs/probe` could not be swallowed by `/v1/runs/{}` however the table is
   written — the table holds no such pair today, and the guard in `api.rs` is written against
   `routes()` rather than against one named route, so it covers the next one added. A placeholder is
   exactly one path segment: non-empty, no `/` — one `match_pattern`, where there used to be a
   `run_resource` and a `run_action` saying the same thing twice.
   `every_route_reaches_its_handler` reconciles its own table against `routes()`, so a new route
   without a test fails the suite. The failure log reads the same table, so a run-scoped route added
   later names its `run_id` without anyone remembering to go and say so.

   **There is no authentication column**, because `sink` has no login at all: anything that can reach
   the port can drive it with the credentials the caller supplies. Failure keeps the sink envelope —
   `{"error": {"code", "message", "run_id", "details"}}` — unchanged by this shape.

**Concurrency**
   **One person doing several things at once, and nothing more.** The `source` HTTP face is served
   by a fixed pool of worker threads sharing one listener, so a slow Oracle column fetch or ten-row
   preview on one screen does not freeze the task list on another. The SQLite-backed stores (tasks,
   datasources, login, run history) are each safe to touch from several threads, and **no lock is
   ever held across blocking IO** — Oracle calls, agent probes and sink requests all happen with
   every lock released. The one mutual exclusion that is a product rule, not an implementation
   detail, is **one task runs at most once at a time** (a second start returns 409).

   The **agent** side is built the same way and for the same reason. Its HTTP face is served by a
   fixed pool of worker threads over one listener; the run registry lock is taken only long enough to
   hand out a run's destination handle and reserve its batch sequence number, with **every MySQL
   write happening outside it**; and the target-side connection pool is bounded and reusing. Those
   three were one thing, not three: while any of them serialised the process, the other two bought
   nothing — a single commit's table-wide rewrite would still hold up every other task's batches.
   How many runs may be in flight on one agent is a **configured number** (`max_concurrent_runs` in
   `sink.toml`, 4 by default), not an accident of process structure; over it, opening a run is
   **refused on the spot rather than queued** (`RUN_QUOTA_EXCEEDED`), because a caller hanging on a
   connection that will not move is worse than being told the quota is full. Worker threads are a
   separate, larger number: the refusal itself, and the read-only endpoints, must still answer while
   several slow writes are in flight.

   Concurrency also **widens the mutual exclusion key from a task to a task ∪ its target table**:
   at most one run at a time per (agent, database, target table). A target table is just a string
   with no uniqueness constraint, so two unrelated tasks may legitimately point at the same one. In
   the serial era that was harmless; concurrently it is silent data loss — one run rewriting the
   whole table while another upserts rows into it deletes those rows, and **both runs report
   success**. This one is **adjudicated on the agent side, never on the source's honour**: there may
   be several sources, and none of them holds the truth about the target database. The refusal
   (`TARGET_TABLE_BUSY`) names **which target table is held by which run**, and the comparison
   ignores letter case, because whether MySQL table names are case-sensitive depends on the host.

   **Multiple users are explicitly out of scope**: there is exactly one account, tasks have no
   owner, and there is no per-user visibility, sharing or audit trail. Everyone who logs in sees and
   can change everything. Concurrency here is about **not making one person wait for themselves**;
   reading it as a step toward multi-tenancy would be a mistake.

## Standing limits

Deliberate debts, not oversights. **Each is a fact about the code as it stands**, not a plan: what
gets paid off and when lives in the issue tracker, not here.

1. **Only one of the two HTTP faces has a door.** `source` requires a login; `sink` does not, and
   `sink` is the end that holds `DELETE` on the target database. Whoever can reach the `sink` port
   can truncate and rewrite any staging or target table without touching `source`. The mitigation
   is the **deployment shape** (loopback / reverse proxy), not the product.
2. **The single account's password defaults to `admin` and never expires.** Nothing forces a change,
   nothing rate-limits a guess, and the interface never mentions it. On an untouched deployment the
   login is two keystrokes, and behind it sit the credentials and write access of **every configured
   datasource**: arbitrary SQL against any source database (including whatever dblink reaches) and a
   full rewrite of any target table.
3. **The target password crosses the wire in cleartext** (source → sink), as do the login password
   and the session cookie on any deployment that is not behind TLS. The mitigation is a deployment
   premise; see the deployment-shape section.
4. **Datasource passwords are encrypted at rest** (ChaCha20-Poly1305 with `data_dir/datasource.key`),
   which **only defends against a bare read of the database file once it leaves the host** (backups,
   snapshots). It does **not** defend against anyone holding read access to `data_dir` — the key sits
   in the same directory under the same 0600, and so does the session table. Accompanying negative
   clause: **`/api/*` never returns a password, not even the ciphertext.**
5. **Logs contain business values** — failure lines carry `column` and `value`. The mitigation is the
   host boundary and the file permissions, not the login: the logs go to stdout, which no session
   guards. The parent's stored copy of those raw lines is the one exception — it sits behind the
   login and carries `value` **truncated to 64 characters** — but the stdout stream it was copied
   from is not truncated and not guarded.
6. **stdout grows without bound** — `source` is long-running and its stdout neither rotates nor caps,
   and the program does not manage it.
7. **The history SQLite database holds business values**, sampled from the source database, under the
   same 0600 as the credential files. Two retentions apply to two different copies: the folded
   history row keeps `column` / `value` in full for 90 days, while the raw run-log lines in the same
   file keep `value` truncated to 64 characters for the stricter of 7 days and the last 10 runs per
   task. **The long one is the folded row, not the raw lines** — the copy that lives longest is the
   one that was never truncated.
8. **Columns of indeterminate precision are not intercepted before submission** — the source performs
   no SQL shape precheck, so the problem surfaces only during a real run, or silently loses precision.
   **The mapping precheck is unaffected**: it describes through a cursor on the sink side and remains
   a hard gate.
9. **Neither write statement ever deletes.** Rows removed at the source stay in the target table
   forever; see **Swap**.
9a. **A run against a primary-key-less target table is not idempotent.** Every re-run appends the
   whole result set again, and nothing in the product detects or repairs the duplicates: there is no
   dedupe pass, no "already imported" marker, and the verification gate only compares this run's
   staged rows against this run's inserted rows, so a doubled target table passes it. This is the
   debt accepted with #261 in exchange for supporting flow-style target tables at all. It is made
   visible in four places (precheck conclusion, task definition, task list, run detail) and mitigated
   nowhere. The way out, when it is wanted, is the clear-then-import write mode.
10. **A finished run cannot be undone.** There is no "undo" action and no record of which rows a
   run wrote. Undo used to exist, backed by a write ledger table `sink` created inside the
   customer's target database; it was removed whole, and the removal is irreversible (#256). Two
   reasons. First, the promise could not be kept: once clear-then-import is a write mode, the rows
   it deletes were never in the ledger, so "undo" could not put them back — and even for upsert,
   undo deleted the rows the run had overwritten rather than restoring them. Second, the price was
   a product-owned table growing row-for-row with the customer's business data, inside the
   customer's own database, written on every commit and read by nothing else. The recovery path
   for a bad run is a corrected re-run, not an undo.
11. **On MySQL 5.7 the metadata reads are slower, and that is left alone.** 5.7's
   `information_schema` is not backed by a data dictionary — the `COLUMNS` / `STATISTICS` /
   `TABLES` queries the target agent runs are answered by opening table definitions, so on a
   database with many tables listing tables and reading a table's columns take visibly longer than
   on 8.0. It costs seconds on the wizard's metadata steps and nothing on the transfer itself,
   so **no optimisation is planned**: caching it would introduce a staleness question that the
   product does not otherwise have.
12. **The wizard's first-step two-pane geometry is a CSS-only invariant** — one row that renders only
   for certain data breaks the alignment; the rules and the worked counter-example live in the
   「两栏取数区的框线」 section of `web/src/app.css`. The row both panes share is rendered by one
   component (`DatasourceRow`), not copied per pane: a hand-written copy makes the invariant depend
   on two places being edited together.
