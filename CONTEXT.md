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

**Neither `listen` is authenticated, so reachability equals privilege.** Anyone who can connect
to the `source` port holds, in effect, the credentials and write access of **every configured
datasource**; anyone who can connect to the `sink` port can truncate and rewrite any staging or
target table. **Deployment premise**: `listen` binds loopback only by default. When several people
need access, the deployer puts a reverse proxy in front and terminates auth and TLS there —
**the product is unaware of it, validates nothing about it, and relaxes no internal constraint
because of it**, which is why it is absent from the diagram above.

`source` and `sink` are separately deployed processes that communicate only over HTTP. `sink` knows
nothing about Oracle. **`source` opens no MySQL connection at all**, but it does **hold and forward
target credentials**: both kinds of datasource live in a SQLite database on the source side, and the
target connection details cross the wire with `POST /v1/runs`. **The MySQL password crosses that
channel in cleartext**, so **the channel an agent address points at must be trusted** (same host, a
trusted internal network, or TLS/tunnelling the deployer builds) — otherwise the password is being
published to the internet.

Both sides are written in **Rust** with synchronous blocking IO. `source` reaches Oracle **11g**
through the `oracle` crate (ODPI-C), so the source machine must carry a full **Oracle Instant
Client 19c Basic** bundle (brought in offline, no root required). The target is **MySQL 8.0**,
`utf8mb4` throughout.

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

**Import Task**
   One complete job: query a batch of data from Oracle, move it into one MySQL table. A task is
   re-runnable — running **the same task definition** twice must leave the target table in an
   identical state. Consistency comes from **upserting on the primary key**; idempotence rests on the
   target table's unique constraint.

**Task Definition**
   A **structured spec** (table, columns, filter clause, primary key). **It is the single source of
   truth, and the source SQL is generated from it.** **The SQL is not stored in the task definition**
   — it is recomputed on demand, and only pinned as a snapshot of what actually ran on each
   run-history row. **Only `source` reads it; `sink` is unaware it exists.**

**Task Draft**
   A Task Definition **while it is being built**, plus everything the interface needs to judge it:
   which fields the person typed or ticked by hand, which were derived for them (same-name mapping,
   inferred primary key, prefilled target table name, generated task name), and the results fetched
   along the way (the ten-row preview, the target-table check).

   It is **never persisted** — leaving the wizard discards it, and re-entry is through the saved task,
   not through a draft. Its rules live in one module (`web/src/wizard.ts`), never in the screens:
   what a change clears, whether that clearing is worth a confirmation, when a fetched result goes
   stale, and whether the next step may be entered. A **hand-made** value is one the person typed or
   ticked, or one loaded from a saved task when editing; a derived value is not. Only hand-made values
   are worth a confirmation before they are cleared.

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
   Completed inside a single transaction on the target: `INSERT ... SELECT ... ON DUPLICATE KEY
   UPDATE` from the staging table into the target table, then `DROP` the staging table. The target
   table holds either all old data or all new data, never an intermediate state.
   **Rows deleted at the source do not disappear at the target** — an upsert only writes, never
   deletes. That is a deliberate debt.

**Tombstone**
   An in-memory record `sink` keeps for a finished run so that "what happened?" still has an answer
   after the swap. It records the **resource's final state**, not the run's, and has only two values:
   `SWAPPED` (the target table now holds the new data) and `DISCARDED` (the target table was never
   touched). **It is a diagnostic cache, not a state store** — only the most recent 32 are kept, and
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
   can exist in three places on the source host (the child process's stdout, a file the deployer
   redirected it into, and the run-history SQLite database), all held to 0600, and **moving them off
   that host counts as exfiltration, which the product never does**. Logs go to stdout only; the
   program creates no files and rotates nothing.

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
   `TIMESTAMP` scales beyond 6 digits, and it **hard-rejects a target table lacking a primary key or
   unique constraint** (without one, `ON DUPLICATE KEY UPDATE` silently degrades into a plain INSERT).
   It is the **only defence** against values being silently altered — and the only defence that can
   be **switched off wholesale**, which is **known gap 8**.
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
   **It grants no clearance of any kind**: after a person creates the table from it, the mapping
   precheck still runs from scratch and may still reject. There is exactly one interception point, and
   it is the mapping precheck. The **staging table**'s DDL is a different thing: that one is *copied*
   from the target table and generated by `sink`.

## V1 scope

**In**: Oracle → MySQL, one direction; the SQL builder (pick table / columns / filter clause /
primary key → generate SQL, with **the spec as the single source of truth**); **plus one custom-SQL
path**; batched push; staging table + atomic swap + a mandatory **row-count** verification; the
mapping precheck as a hard gate.

**Out**: MySQL → Oracle; creating the target table automatically (only the DDL is generated, for a
person to run); authentication; scheduling (runs are triggered by hand); multi-table coordination or
transaction-level consistency; **per-row checksums** (deferred to V2 — "values were not silently
altered" is guaranteed by the mapping precheck before the run starts, rather than discovered after
the fact by a checksum on every run); centralised logging or integration with an external log
collector (it crosses the host boundary).

**Known gaps** (deliberate debts, not oversights; each has its own condition for reopening):

1. **An unauthenticated inbound face means reachability equals credential privilege.** Whoever can
   reach the `source` port can run arbitrary SQL against the source database (including the further
   databases dblink points at) and can truncate and rewrite any configured target table; the `sink`
   side is unauthenticated and holds `DELETE`. The exposure covers **every configured datasource**'s
   credentials and write access. The mitigation is the **deployment shape** (loopback / reverse
   proxy), not the product.
   **Reopen when**: a real deployment appears in which source must be reached by several people who
   are not in the same trust domain.
2. **The target password crosses the wire in cleartext** (source → sink). The mitigation is a
   deployment premise; see the deployment-shape section.
3. **Datasource passwords are encrypted at rest** (ChaCha20-Poly1305 with `data_dir/datasource.key`),
   which **only defends against a bare read of the database file once it leaves the host** (backups,
   snapshots). It does **not** defend against anyone holding read access to `data_dir` — the key sits
   in the same directory under the same 0600. Accompanying negative clause: **`/api/*` never returns a
   password, not even the ciphertext.**
4. **Logs contain business values** — failure lines carry `column` and `value`. The mitigation is the
   host boundary plus the reachability of `listen`; it reopens together with gap 1 when authentication
   arrives.
5. **stdout grows without bound** — once source is long-running its stdout neither rotates nor caps,
   and the program does not manage it. Reopen "the program creates and rotates its own file" when
   there is real feedback about filling a disk.
6. **The history SQLite database holds business values for 90 days.** Reopen an independent erasure
   window for `value` when a hard requirement against retaining business-value samples appears.
7. **Columns of indeterminate precision are not intercepted before submission** — the source performs
   no SQL shape precheck, so the problem surfaces only during a real run, or silently loses precision.
   **The mapping precheck is unaffected**: it describes through a cursor on the sink side and remains
   a hard gate.
8. **The mapping precheck can be switched off wholesale.** `DB_QBS_POC_RELAXED_PRECHECK=1` on the
   sink host turns off the type whitelist, nullability, the unique-constraint check and the range
   check **together**, and what happens to an out-of-range value then is decided by the target
   server rather than by this product: under `STRICT_TRANS_TABLES` MySQL refuses the write, so the
   run fails **while pushing batches** — after the source has been read in full and a staging table
   created, rather than before anything started — and without strict mode MySQL truncates the value
   and the write succeeds, which is the silent corruption the precheck exists to prevent. Either
   way the gate is gone; only the shape of the damage differs.
   It is announced **once**, in a `sink_started` warning at startup, and nowhere else: a
   finished run's log, history and screen carry no trace of which mode it ran under. It is not a
   capability, it is a POC-era escape hatch, and the `POC` in its name is the judgement of whoever
   added it.
   **Reopen when**: the first release's acceptance is done and no deployment is found to depend on
   it — at which point it is deleted.
