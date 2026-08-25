# ADR-0042: Filling in the list workflows after the x2doris admin paradigm (P1) — filter strips, latest run, client-side paging, inline connection test

**Status**: Accepted
**Date**: 2026-08-21
**Inputs**: the P1 handoff document and the redacted x2doris reference screenshots
(`docs/prototypes/p1-claude-code-handoff.md`, `docs/prototypes/p1-x2doris-reference.md`,
`docs/prototypes/assets/`). They were retired once P1 landed and survive only in git history;
the body of this ADR *is* the ruling on those inputs.
**Precedent**: [ADR-0039](0039-v1-ui-increments.md) (v1 rendering-surface increments) — this ADR is
its **successor increment** and overturns none of its shape rulings;
[ADR-0041](0041-v2-scope-trial-readiness.md) (v2 = trial readiness)

## Background

With v1 closing the loop and v2 opening the delivery path, the first feedback from the field was not
"it cannot move the data" but **"the lists are not enough"**: tasks had only a search box and gave no
sign of whether the last run succeeded, run history could be filtered by task alone, and dozens of
rows on one screen could only be scrolled.

The reference is x2doris, a comparable synchronisation product, and its admin paradigm.
**Only the information architecture is copyable, not the visuals**: db-qbs already has a similar
light workbench shell, so this round **introduces no Ant Design, changes no brand visuals, and does
not touch `tokens.css`.**

P0 already landed a round of narrowing (removing the table-creation and target-DDL entry points,
dropping the non-v1 nav placeholders, and flattening internal jargon). This ADR records the layer
after it: **the list workflows.**

## Decision

### 1. One list paradigm across three screens: filter strip → table card → pagination

The task screen and the run-history screen share one skeleton. The datasource screen **does not
follow it** (see §5).

| Screen | Filters | Paging |
|---|---|---|
| `#/tasks` | task name, source, target, latest status | 20 per page |
| `#/history` | task, status | 20 per page |
| `#/datasources` | **none** | **none** |

**Filtering is explicit**: changing a dropdown does not re-filter; it takes effect on pressing
「查询」, and 「重置」 clears everything and returns to page 1.
This is not a new ruling — it extends to the task screen what the run-history screen already did in
P0. A list that jumps rows while you type costs more, at a scale of dozens of rows, than one extra
click saves.

In implementation this is two pieces of state — "the set being edited" and "the set in effect" —
with the rules collected in `web/src/listing.ts`.

### 2. Paging can only be client-side, and must not pretend to be server-side

The current API has no `limit/offset` (`listTasks()` and `listRunHistory()` both return the whole
set). **This release does not add one** — server-side paging would also require fixing the ordering,
the cursor, and where the total comes from, which is another ticket's worth of work.

So paging turns pages of **the whole set already fetched**, and the interface holds that line:

- The N in `共 N 条` is the count **after filtering**, not the server's total.
- The card header shows two numbers only when "filtered ≠ total" (`筛出 12 / 共 30 个`) — writing
  「筛出 30 / 共 30」 with no filter applied manufactures a distinction that does not exist.
- When the total fits on one page, **the pagination strip is not rendered at all**.
- Out-of-range page numbers are always clamped (guaranteed by `paginate`); deleting the only row on
  the last page falls back to the previous page rather than showing a blank screen.

### 3. "Latest run" on the task screen: read once, no polling, and do not compress the three axes into one coloured tag

On entering the task screen it makes **one extra call** to `listRunHistory({})` and joins the latest
row per task client-side by `task_id`. "Latest" means the greatest `started_at`, tie-broken by
`run_record_id` — not because the id carries meaning, but because this cell must be **reproducible**:
rendering the same data twice must not produce different rows.

Three boundaries:

1. **No polling, no auto-refresh.** The task screen is not a run monitor. The column header says so
   outright: 「状态可能有延迟，以发起结果为准。」 Saying it to someone's face is cheaper than letting
   them treat it as a live dashboard.
2. **No colour, no tag.** The three axes (run outcome / target-table effect / error code) each have
   their own shape on the run-history screen; this cell is merely an index. Compressing it into one
   coloured dot would suggest it is the whole conclusion. The full plain-language sentence hangs off
   `title`.
3. **「尚未运行」 and 「读取失败」 are different things.** The first is a fact (this task has no
   history); the second is that this particular read failed. When the history read fails **the task
   list still renders**, and only this column reports 「读取失败」 — merging them would be drawing a
   conclusion on the server's behalf.

The "latest status" filter therefore has five values: 成功 / 失败 / 进行中 / 结局不明 / 尚未运行.
The first four are verbatim `historyPresentation(row).kind`; **no separate vocabulary is introduced.**

### 4. Run-history column order: conclusion first, IDs in the detail

New order: **task · outcome · error code · rows · duration · started at · actions · expand**.

**The "run parameters" column is dropped** — the expanded detail already carries it, and it is the
column most easily blown out on this screen (parameter names are defined by the task itself, so
their length is uncontrolled). Full IDs, row-count reconciliation, per-stage timings, and the source
SQL all stay in the detail, whose structure is unchanged.

### 5. The datasource screen gains an inline "test connection", but **still no filter strip and no "connection status" column**

What ADR-0039 §2 ruled out was a **persistent connection-status column**: it would either poll every
database in the background or display a stale green dot. That ruling stands verbatim. What is added
here is **a single inline question and answer** — the one you clicked yourself, with an explicit
moment of asking, which never vouches for a fact from a minute ago. The result is **transient**: not
stored, not polled, gone on refresh, and cleared immediately when that datasource is edited or
deleted (its connection fields may have just changed).

Presentation follows the two rules of §3: success is `.inline-result`, **one line of plain text**
(`连接成功 · 186 ms · dw_stage`); failure **echoes the driver error verbatim with no error code tag**
— a connection test belongs to no run, and error code tags come from the protocol's closed set.

Equally, **no filter strip is added to the datasource screen**: the field has only three to five
rows, visible on one screen. x2doris does have filters on its datasource page, and that detail is
**explicitly not copied**.

#### It uses the draft test-connection endpoint, not the by-id one

The by-id endpoint (`POST /api/datasources/{id}/test-connection`) returns only `{ ok: true }`, which
cannot produce the line `连接成功 · 186 ms · dw_stage` — the duration and the database name exist
only on the draft endpoint. The inline test therefore sends a draft generated from the stored
datasource **with the password left empty**, and the server reads "empty = use the stored one",
matching the rule on the save path. The interface still reads back not one character of a password.

**Zero backend change** was a hard constraint this round, and this path was chosen precisely for it.

### 6. Inline primary actions get text; secondary actions stay icons

「发起运行」 on a task row and 「测试连接」 on a datasource row become ghost buttons with text
(`.button.is-row-action`); edit / rename / delete stay icon buttons. The reasoning follows the
reference: **icon buttons are for secondary actions, and an inline primary action may carry text** —
picking out "which one runs it" from a row of four identically coloured icons relies on memorising
positions.

### 7. The design-system ledger: **neither `tokens.css` nor the README changes**

All new CSS lands in `web/src/app.css`; no class is added under `docs/design-system/`, and the
component inventory in README §7 neither grows nor shrinks. Filter strip, data table, card, and
button are all reuses of existing components, and the pagination strip is a tool row beneath the
"data table", not a new piece of visual language.

**New rules** (parallel to the four in ADR-0039 §9; neither set overrides the other):

| # | Rule | Purpose |
|---|---|---|
| 1 | `.filter-field.is-compact` / `.history-filters .filters-refresh` | Narrow fields on the filter strip and the right-pushed refresh button |
| 2 | `.button.is-row-action` | Inline primary action buttons carrying text |
| 3 | `.latest-run*` / `.data-grid th.latest-run-column small` | The two plain-text lines of "latest run" and the staleness note in the header |
| 4 | `.action-column.is-wide` / `.row-test-result` | The widened action column and the transient inline test result |
| 5 | `.list-pagination` / `.pagination-*` | The client-side pagination strip |

**Removed**: `.toolbar` / `.search-field` (the styling of P0's single search box, now unreferenced)
and `.run-params-cell` (unreferenced once §4 dropped that column). `.history-grid`'s `min-width`
tightens from 1260px to 1120px.

## Consequences

1. **The task screen makes one more network request** (fetching run history on entry). The number of
   history rows grows with the retention period, and under a 90-day retention this is still a
   single whole-set fetch — **the direct cost of "no `limit/offset`" from §2.** Do not optimise it
   before the symptom appears; the right fix when needed is an endpoint on `/api/runs` returning only
   the latest row per task, not truncation in the front end.
2. **Filtering and paging are both client-side**, so `共 N 条` is measured against the local copy. If
   the server ever adds paging, this layer must be rewritten together with §2 rather than merely
   swapping the request.
3. **"Latest run" goes stale.** The note in the header is the price paid openly, not a disclaimer —
   if the field reports that this column must be live, the right answer is an explicit refresh or a
   subscription, **not** quietly enabling polling.
4. **The inline datasource test hits a real database.** Clicking through five datasources means five
   connections; the absence of a bulk entry point is exactly why.

## Walkthrough triggers

- **X series fires**: this round changed the datasource screen (§5), matching the literal wording of
  existing trigger 2 in `CLAUDE.md`. It also **adds X10–X12** to the checklist, bringing P1's three
  new information sites (task-screen filter strip + latest run, run-history status filter + paging,
  inline datasource test) under the gate — without them there would be no rendering criterion at all.
  The trigger list therefore goes from two entries to three, and the gate table in `CLAUDE.md` is
  updated to match.
  **Record**: `v1-visual-walkthrough-20260821T025018Z.md` (retired; see git history), with actual
  observations recorded for X1–X12.
- **V1–V25 does not fire**: neither `docs/design-system/README.md` nor `tokens.css` changed (§7).
- **W1–W6 does not fire**: neither the `.precheck-reports` layout nor the `DiagnosticTable` column
  structure changed.

### Three rig changes that follow the criteria

The rig follows the criteria; otherwise the next machine silently runs a stale set of observations
(`CLAUDE.md` rule 4):

1. `v1-probe.py` read run-history cells **by position** (`cells[4]` was the outcome, `cells[9]` the
   actions). §4 changed the column order, so those indices read the wrong cells outright — they are
   collected into a `HISTORY_COLUMNS` constant so positions are written down once.
2. Three new observation groups, `observe_task_filters` / `observe_history_filters` /
   `observe_row_test` (X10–X12).
3. Paging needs more than 20 rows to have anything to act on, but filler rows would bury the X1–X9
   record in noise. The stub therefore gains an `X_BULK=1` switch, and `run-x-walkthrough.sh` makes
   **two passes**: a normal pass producing X1–X12, and a bulk pass (`X_ONLY=pagination`) producing
   only the paging and page-turning of X10/X11.
