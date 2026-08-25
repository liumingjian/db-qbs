# ADR-0039: Interface increments closing the v1 loop — the datasource screen enters the nav, two-column mapping, the target-table dropdown; zero design-system change

**Status**: Accepted
**Date**: 2026-08-19
**Source**: [#123](https://github.com/liumingjian/db-qbs/issues/123) (map [#117](https://github.com/liumingjian/db-qbs/issues/117))
**Prototype**: [`docs/prototypes/0123-v1-ui-increments.html`](../prototypes/0123-v1-ui-increments.html)
(6 screens + one variant comparison, rendered and checked for real)
**Related**:
`ADR-0025` (the authority on visual language and the design system; **this ADR does not touch
`tokens.css` and adds no component**, but requires correcting two factual statements in
`docs/design-system/README.md`, see §9),
`ADR-0037` (its §5 voided "the UI offers no read/write surface for connection configuration" — this
ADR is that ruling's interface delivery; its §6 field set, §7 deletion refusal, and §8 binding hung
off `Task` are untouched),
`ADR-0038` (§2 the mapping shape, §3 the target table upgraded to a dropdown, §6 the primary-key
selection surface, §10 unit annotation — **that ADR fixes semantics only; this one fixes presentation**),
`ADR-0035` (the primary key is mandatory),
`ADR-0036` (the structured spec is the single source of truth; the interface shows SQL read-only),
`ADR-0023` (§4 "illuminate, never judge" stands verbatim),
`ADR-0026` (the stance that "a disabled state itself lies", invoked by §2 here),
`ADR-0024` (its §2 negative clause was half-voided by ADR-0037 §5),
`ADR-0033` (two units for `length`; §7 here annotates without fixing),
[#122](https://github.com/liumingjian/db-qbs/issues/122) (the walkthrough entry point and case
numbering belong to it; this ADR only settles the account)

## Background

The first four tickets on this map (#118/#119/#120/#121) settled all the semantics, landing as
ADR-0035 through ADR-0038. What remains is **what they look like on screen** — and this ticket is
heavier than the M3 interface increment: M3 added five information sites, while this one adds
**an entire screen** (datasource management).

So this ticket's only real risk is **prising open the design system**: per `CLAUDE.md`, changing
`docs/design-system/README.md` or `tokens.css` means **re-running the whole V1–V25 walkthrough**.
M3's answer was "zero design-system change" (#102), and this ticket aims for the same verdict.

**Verdict: won.** All six screens are assembled from existing elements, `tokens.css` does not change
by one byte, and the new CSS landing in `web/src/app.css` totals **4 rules** (§9).

### The owner's two rulings on 2026-08-19 (inputs to this ADR)

1. **The field has only three to five datasources.**
2. **A datasource must pass a connection test before it can be saved** (overturning this ticket's
   earlier proposal of "not enforced, only hinted"; see §3).

## Decision

### 1. Datasources enter the nav as the third item

> The left nav goes from `Tasks / Run history` to `Tasks / Run history / Datasources`; the non-V1
> placeholders (scheduling, alerting) keep their positions.

**Not folded into the task screen**: datasources are shared by several tasks and are created, edited,
and deleted independently. Folding them in would mean managing credentials while creating a task —
exactly the coupling ADR-0037 §7 ("refuse deletion while tasks still reference it") guards against.

**Third, not first**: it is a thing you **set up once and rarely return to**, so it should not occupy
the first slot.

**The nav item itself changes nothing** — `.nav-item` and §7's "nav placeholder" rule are reused
verbatim with one more entry. README §7's component inventory is therefore unchanged; what changes is
the factual description of the reserved placement beside it (§9).

### 2. The datasource list: no filter strip, no "connection status" column

- **No filter strip and no type filter** (owner ruling 1): three to five rows fit on one screen, and a
  search box is pure noise. The toolbar on the task screen exists because tasks grow into the dozens —
  **do not copy it across.**
- **No "connection status" column**: it would either poll every database in the background or show a
  stale green dot, and both are worse than showing nothing (the same stance as ADR-0026, "a disabled
  state itself lies"). Whether it connects is asked on the spot by clicking "test connection".
- **A "referenced by" column** (`3 个任务` / `未被引用`): that count must be queried anyway to decide
  §4's 409, so displaying it is free — and it happens to answer "can I delete this?".
- **Columns**: name (with `datasource_id` in small text) / type / connection / user / password /
  referenced by / actions. The connection cell shows `connect_string` for Oracle and
  `host:port / database` for MySQL — **two different field sets, each shown in its own way in one
  column**, rather than fabricating a fake unified "connection string" (the same reason ADR-0037 §6
  does not store a DSN string).

### 3. Create / edit dialog: one form, two field sets; **store only what connects**

- **Type is chosen at creation and immutable when editing**: changing the type means a different
  datasource, and tasks are already bound to it by id (ADR-0037 §8).
- **Field sets follow ADR-0037 §6**: three for Oracle (`connect_string` / `username` / `password`),
  five for MySQL (`host` / `port` / `database` / `username` / `password`).
- **The password is write-only**: it always displays empty when editing, with a neutral badge at the
  top right reading 「已设置 · 留空 = 不改」 or 「未设置」. The interface **never reads a password
  back**, not even the ciphertext (ADR-0037 §5).
- **"Test connection" uses the values currently in the form**, not the stored ones — otherwise a
  changed password could not be verified.
  **When the password is left empty while editing, the test uses the stored password**: the same
  interpretation rule as "empty = unchanged" on save, and the two must never diverge.

> **Save threshold (owner ruling 2): the current form's combination of connection fields must have
> passed a connection test at least once, or the save button is disabled.**
> Changing any connection field invalidates the previous result immediately and requires a retest.
> **One exception: renaming only.** With no connection field touched, it saves without a retest — that
> set of values has not changed, a retest buys no new information, and it would require the database to
> happen to be online at the moment of a rename.

**Cost accepted**: while the database is not yet provisioned or the network not yet opened, this
datasource **simply cannot be entered**, and someone must come back and fill it in after ops opens the
way. What that buys is **every datasource in the database having genuinely connected at least once**,
so "cannot connect" does not defer its explosion to the moment a run is submitted. The owner chose
"everything entered is usable" over "freedom to enter".

**Presenting the test result**: success is **one line of plain text** (`连接成功 · 186 ms · dw_stage`);
failure reuses the existing `.form-error`.
**No error code tag** — `test-connection` belongs to no run, and drawing a `SINK_ENVIRONMENT` tag would
suggest a run had failed, colliding with ADR-0025 README §3 ("the three axes serve runs only; their
shapes may not be reused"). The failure body **echoes the driver error verbatim**
(`ORA-12541: TNS:no listener`) followed by a plain-language sentence, ordered per README §3 ("prose
first, code second").

### 4. Deleting a referenced datasource: refuse, and name the tasks

ADR-0037 §7 fixed the 409, but answering only "tasks still reference it" is not enough — the person then
has to page through the task screen one by one. **List the task names**: that reference list must be
queried anyway to decide the 409.
The shape is one extra list inside the existing delete dialog's `.delete-copy` body — **zero new
elements**.

### 5. Builder, target side: a native `datalist` carries the filterable dropdown

ADR-0038 §3 required the target table to be **upgraded from a hand-typed field to a filterable dropdown**.

> **Method: native `<input list="..."> + <datalist>`.**

- **Zero new components, zero new CSS**; the browser supplies substring filtering and keyboard operation.
- **Typing still works** — someone who remembers the full name need not scroll a list, which is exactly
  the scenario the owner described (remembering one or two keywords makes typing harder than picking).
- **Cost written down**: `datalist`'s dropdown styling belongs to the browser, no per-item subtitle is
  possible, and Safari's experience is a little rough. Replacing it with a hand-drawn combobox would add
  a whole component plus a keyboard-accessibility layer — **not bought in v1.**

**The target table's column list (the result of `POST /v1/target/columns`) is presented with the existing
`.data-grid`**, with columns: target column / type / length (characters) / nullable / default / constraint
/ mapped from.

- **Illuminate, never judge** (ADR-0038 §3's closing paragraph, ADR-0023 §4): `PRIMARY` / `UNIQUE u_code`
  sit in the "constraint" column for reference, and **the primary key is still ticked by the user**.
  Columns that are unmapped, non-nullable, and without a default (the class the precheck will reject) are
  **not blocked here**, only dimmed as a whole row. The builder permitting what the precheck rejects is
  deliberate — `information_schema` and the target table at the moment of the run may disagree.
- **Unmapped rows are dimmed entirely** (1 CSS rule): they are reference information, not a to-do item.
- The card's footnote states that this result **is discarded on refresh** (ADR-0038 §8).

### 6. Builder, two-column mapping: a permanent input box, with the primary-key tick moved beside the target field

> The existing six-column table gains a **"target field"** column, pre-filled with the source name by
> default (ADR-0038 §2: that is not a make-do default; it *is* the correct expression of identity mapping).

**Control shape A, "a permanent input box", is chosen over B, "click to edit"**:

- The customer's named requirement is "same-name mapping by default, hand-pick when they differ", so
  **renaming is a routine action, not an exceptional one**, and charging an extra click per renamed column
  prices it at the worst case.
- B's benefit (edited columns carry a visible trace) **already exists once the two columns sit side by
  side** — source name on the left, target name on the right; different means edited.
- A is a plain `<input>`, while B needs a "text ↔ input" toggle state, and **one more state is one more
  place to go wrong.**

**The primary-key tick moves from far left to the right of the target field**: ADR-0038 §6 fixed that the
primary key stores **target column names**, so the thing being ticked must sit beside the name it refers
to. Leaving it at the far left would suggest the source column is being ticked.

**Unselected rows**: the target-field input and the primary-key tick are **both disabled and dimmed** —
an unselected column has no target name to speak of.

### 7. Unit annotation: write "(characters)" in the header, with a static note beneath

Delivering ADR-0038 §10, in two parts:

1. The length headers of both source and target columns **read "长度（字符）"** — the two columns do in
   fact share a unit.
2. A **static note** beneath the table: this column is in characters, while **the mapping precheck judges
   in bytes** (ADR-0033), so on columns containing Chinese the two disagree (10 Chinese characters are 30
   bytes under `utf8mb4`). V1 does not unify them; when they collide, the precheck's verdict governs.

**Zero judgement, zero metadata** — the same handling as ADR-0027 §8's treatment of `ERROR 1118`.
Annotating the unit without that sentence is not enough: the first Chinese column in the field will hit it
and nobody will know why.

### 8. Task screen: one more column, "source → target", by name not by id

- The task list gains **one column** holding two segments per row: `生产核心库 → 数仓 MySQL`. **Use the
  datasource names**; `datasource_id` appears only on the datasource screen.
- The two dropdowns in the create-task dialog were already wired when ADR-0037 landed; this ticket adds one
  thing: **when there is no datasource at all, the dropdown offers not a blank but a route to the
  datasource screen** (「去『数据源』建一个 →」).
- **No "create a datasource inline"**: a dialog inside a dialog would give one form two entry points, and
  once the "store only what connects" behaviour (§3) diverges between them, the resulting inconsistency is
  the hardest kind to find. The cost is one interruption while creating a task, but per ruling 1 (three to
  five datasources) it happens only a handful of times in total.

### 9. The design-system ledger: `tokens.css` untouched, the README corrected on facts only

**Four new CSS rules in total, all landing in `web/src/app.css`; no class is added under
`docs/design-system/`:**

| # | Rule | Purpose |
|---|---|---|
| 1 | `.data-grid .cell-input` | The target-field input inside a table (matching the cell height) |
| 2 | `.field-badge.is-neutral` | The neutral variant of the password "set / not set" badge — an existing class with three colour tokens swapped, the same route as M3's `.row-size-warning.is-crit` |
| 3 | `.inline-result` | The single line of plain text for a successful connection test |
| 4 | `.data-grid tr.is-unmapped td` | Dimming an unmapped target column's whole row |

**But `docs/design-system/README.md` must be corrected in two factual places** — both were voided by
ADR-0037, and leaving them makes the file false:

- **The closing paragraph of §5**, "there is no connection-configuration management page; that is a
  decision, not an omission" (restating ADR-0024 §2's negative clause) — that clause was half-voided by
  ADR-0037 §5 and has not been the operative ruling since 2026-08-19.
- **The "reserved placements" in §7** — add the datasource screen, shaped as "app shell + card + data
  table + dialog, with no new components".

> **Ruling: these two README changes trigger the whole V1–V25 walkthrough by the literal wording of
> `CLAUDE.md`. Accepted; no exemption sought.**

The reason is that **the gate's credibility is worth more than the convenience of this one occasion**: once
"text changes may be exempt" is opened up, the cost of adjudicating "is this text or visual?" next time far
exceeds simply running the walkthrough — M2's `--ok-bg` incident (recorded in README §8) is what happens
after a criterion is softened.
**When the walkthrough runs, and which ticket's acceptance it counts toward, belongs to
[#122](https://github.com/liumingjian/db-qbs/issues/122)**; this ADR only records the debt here, and
**it may not be quietly skipped inside an implementation ticket.**

**M3's W1–W6 walkthrough** is governed the same way by `CLAUDE.md`: this ticket touched tables other than
`DiagnosticTable`, but the `.precheck-reports` layout and `DiagnosticTable`'s column structure are
**unchanged** — whether W1–W6 re-runs is likewise #122's call.

## Costs and validity

1. **The mandatory connection test blocks "enter it before the database is provisioned"** (§3). If the field
   reports this step is too rigid, the answer is an explicit "saved but unverified" state, **not** quietly
   loosening the threshold.
2. **The README change triggers the whole of V1–V25** (§9). This is the most expensive item in the ticket,
   and it is bought at a stated price.
3. **`datalist`'s styling and its Safari experience** (§5). A better dropdown means a hand-drawn component,
   which would prise open README §7's component inventory.
4. **The `length` unit discrepancy is annotated, not fixed** (§7). ADR-0033 settled it; unification comes
   after v1.
5. **Target tables and target columns really connect to MySQL each time they are opened** (ADR-0038 §8 does
   not cache). With many tables the first screen will be slow; do not optimise before the symptom appears.
   The answer when needed is front-end filtering, and `datalist` is front-end filtering already.

## Impact list

| Location | Change |
|---|---|
| `web/src/App.tsx` | A third nav item and the `Page` type; a new `DatasourceScreen` and the datasource dialog; the mapping and primary-key columns repositioned; the target-side card (`datalist` + target column reference table); a "source → target" column on the task list; the guidance shown when there is no datasource |
| `web/src/api.ts` | Two new calls, `/api/target/tables` and `/api/target/columns` (the source-side proxies of ADR-0038 §3); `TaskSpec.columns` becomes `ColumnMapping[]` |
| `web/src/app.css` | §9's four rules; deletion of the now-void copy in `.target-side-note` reading "no target-table dropdown … is a decision, not an unfinished feature" |
| `docs/design-system/README.md` | Two factual corrections, in §5's closing paragraph and §7's reserved placements (**triggers the whole of V1–V25**; the entry point belongs to #122) |
| `docs/design-system/tokens.css` | **Untouched** |
| Walkthrough | New cases for the datasource screen, the two-column mapping, and the target column reference; numbering and ownership belong to #122 |

**Threshold**: this ADR changes front-end presentation only, but the implementation ticket also lands
ADR-0038's transfer-semantics change, so that ticket **must run all three rigs (M1/M2/M3)** plus
`npm run typecheck && npm run build && npm test`. Until those have run, no report may say "passed".

## Addendum (2026-08-19, during implementation-spec ticketing): the primary key follows a renamed target; `column_precision` gets no v1 interface

Breaking down the implementation tickets surfaced two interface behaviours that none of ADR-0035 through
ADR-0039 answered. The owner ruled on both on the spot, recorded here so implementers do not each decide
separately.

### 1. Renaming a target field carries the ticked primary key with it

§6 moved the primary-key tick beside the target field, and `ADR-0038` §6 fixed that
`TaskSpec.primary_key` stores **target column names**. Together they left a gap: **the user renames a
column's target from `C_ID` to `CUST_ID` while that column is ticked as the primary key** — at that moment
the `C_ID` sitting in `primary_key` points at a name that no longer exists.

> **Ruling (owner, 2026-08-19): the tick is preserved, and that entry in `primary_key` is renamed to match.**

The reason is that **in the user's perception they ticked "this row", not "this string"**. Renaming is a
routine action per §6's own words ("renaming is a routine action, not an exceptional one"), and letting a
routine action knock out state elsewhere charges the user for an implementation detail (that the primary key
stores names rather than row numbers).

**Two alternatives rejected**:

- **Renaming clears that column's primary-key tick** — changing one letter would require re-ticking,
  treating a normal rename as an anomaly.
- **Renaming blocks saving until the primary key is re-confirmed** — the hardest option, and it interrupts
  precisely the routine action.

**One implementation constraint follows**: the target-name input and the primary-key set **are not two
independent pieces of state**. `primary_key` must either be derived from "which rows are ticked" or be
rewritten in sync on rename.
**No intermediate state may exist in which the interface shows a tick while `TaskSpec` holds the old name** —
that would travel all the way to the sink precheck before exploding as "primary key columns must be among the
columns selected", with the user having done nothing wrong.

### 2. `column_precision` gets no interface entry point in v1

The `(p,s)` of a bare `NUMBER` or a numeric expression column (`ADR-0030` §4.2) can currently only be changed
by editing the task file or calling the API directly; the builder has no editing entry point.

> **Ruling (owner, 2026-08-19): not added in v1; deferred.**

Tables in the field almost always declare `NUMBER(p,s)`, so hitting a bare `NUMBER` is unlikely; and when it
does happen the task simply **cannot be created** rather than **moving data incorrectly** — the failure mode
is on the gate's side, which is acceptable.

**The "precision to be configured" marker on the column-fetch surface stays** (one of ADR-0032's five M3
information sites): it still states truthfully which columns need human attention, only the place to act is
not in the interface for now. **No static note is added for it either** — v1 does not paste copy onto the main
path for a low-frequency shape.

**Validity**: add the entry point when the first bare `NUMBER` column appears in the field, shaped as filling
`(p,s)` in place in that column of the fetch table — no new screen, no new component.

### 3. "Store only what connects" requires a **draft** test-connection endpoint, `POST /api/datasources/test-connection`

(Added 2026-08-19, from the implementation of [#130](https://github.com/liumingjian/db-qbs/issues/130))

§3 fixed that **"test connection" uses the values currently in the form, not the stored ones**, but the impact
list named only three lines under `web/src` and `api.ts` — **which is not enough.** The existing test entry
point is `POST /api/datasources/<id>/test-connection`, which reads a row from the database by id and tests it:

- **In creation there is no id at all**: the datasource is not yet in the database, so "store only what
  connects" has no subject at the moment of creation.
- **When editing with a changed password, testing by id tests the old password**, so what passed and what is
  about to be stored are not the same set of values — which is precisely what the test exists to prevent.

> **Ruling: add an id-less draft endpoint, `POST /api/datasources/test-connection`.**
> The body is a `DatasourceInput` plus an optional `datasource_id` (present only when editing).

- **The interpretation of an empty password is identical to that of saving** (`PUT`): given a `datasource_id`
  it uses the stored one, and in creation an empty password genuinely means no password. §3 states "the two
  must never diverge", and here that is delivered as one shared private function
  (`DatasourceStore::draft_password`) rather than written twice.
- **It writes to no store**: the resolved connection is discarded after use, creating no datasource and no
  entry in the run registry — the same handling as ADR-0038 §3's two target metadata entry points.
- **The reply carries `elapsed_ms` and `label`** (the database name for MySQL, the connect string for Oracle),
  which §3's `.inline-result` line (`连接成功 · 186 ms · dw_stage`) needs. **Still no error code tag.**
- **The existing by-id endpoint stays**: it answers "can this stored datasource still connect?", a different
  question from the draft test.
- **Route ordering is hard**: the draft endpoint must precede the by-id one, or the `test-connection` segment
  will be swallowed as a datasource id. The case
  `the_draft_test_connection_reads_the_form_values_and_writes_nothing` guards exactly this (when swallowed it
  returns 404 rather than 400).

**This does not change §9's "zero design-system change" verdict**: what is added is a backend endpoint, and the
interface still has only those four CSS rules.
