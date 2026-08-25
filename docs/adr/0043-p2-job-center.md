# ADR-0043: Job Center (P2) — copying the x2doris admin shell, folding run history into the task list, and progress as an integer percentage counted before the run

**Status**: Accepted
**Date**: 2026-08-21
**Inputs**: the owner's eight rulings of 2026-08-21 (in conversation, listed one by one in §1); the
shape artifact [`docs/prototypes/p2-job-center.html`](../prototypes/p2-job-center.html)
**Precedent**: [ADR-0039](0039-v1-ui-increments.md) (v1 rendering-surface increments),
[ADR-0042](0042-p1-list-workflows.md) (P1 list workflows) — this ADR **overturns several of their
clauses**, listed one by one in §9; `ADR-0025` (design direction D) — this ADR **rewrites where its
values come from**, see §3

## Background

After P0's narrowing and P1's completion of the list workflows, the remaining problem was not "which
item is missing" but that **the whole shell is not the same kind of thing as the reference.** Having
looked at P1's screenshots on 2026-08-21, the owner upgraded the ruling from "take x2doris as a
reference" to "**copy x2doris**", and named the fault in the previous round: **holding up our own
immature ideas against an available best practice.**

That criticism is this ADR's first premise, written here so that the next person facing a trade-off
**defers to the reference first** rather than re-arguing "our way is fine too". ADR-0042's background
sentence — "**only the information architecture is copyable, not the visuals**" — **is revoked by this
ADR**: the visuals are copied too, with values measured rather than eyeballed.

The reference is x2doris 1.2.0's job center (the StreamPark lineage: vue-vben-admin + Ant Design 4).
Its instance address and credentials are supplied by the owner when needed and **are not committed**;
neither this ADR, the token file, nor the prototype records them.
Its front end is packed inside a jar, and **we do not dig into the jar** — measuring the computed
styles of the running page directly is both faster and more accurate.

## Decision

### 1. The owner's eight rulings of 2026-08-21 (verbatim, the inputs to this ADR)

| # | Ruling | Where it lands |
|---|---|---|
| 1 | **Copy x2doris**, do not merely reference it: layout, typography, UX, column contents, and inline icon actions all defer to it | Throughout |
| 2 | **Delete the standalone "run history" screen** and fold it into the job center; the list shows only the **most recent** run, and multiple history entries are "for later" | §2 §4 |
| 3 | **List columns**: task name · source table · target table · migration progress · run status · start time · duration · actions; primary key / conditions / error code / target-table effect move into the detail drawer | §2 §4 |
| 4 | **All inline actions are icons** (run · run detail · edit · rename ｜ delete), identified by `title` | §5 |
| 5 | **Bulk run + bulk delete**: a checkbox column, disabled when nothing is selected | §6 |
| 6 | **Migration progress**: run one `COUNT(*)` before the transfer to get the denominator (the backend changes in this release), and show a single **integer percentage** — no decimals, no row counts appended | §7 |
| 7 | **The sider supports collapsing** (256px ⇄ 48px) | §8 |
| 8 | **Light only**; but x2doris's light layout **has a dark `#001529` sider itself**, so copy that | §8 |

### 2. The "run history" screen is removed; the job center is the only list screen

The nav contracts from four items (tasks / run history / datasources / …) to three: **Job Center ·
Datasources · Settings**. `HistoryScreen.tsx` is deleted entirely, and the `#/history` route
**redirects** to the job center rather than 404ing — that address is still circulating in old links
and old docs, and catching it is cheaper than letting people hit a wall.

One row of the list = **one task plus its most recent run.** This merge follows directly from ruling 2,
and it also cancels ADR-0042 §3's "latest run" column — that column was an index built on the premise
of two coexisting screens, and once they merge it simply *is* this table.

**Multiple run histories for one task are out of scope for this release** (ruling 2's wording, "for
later"). Recorded as a debt; the trigger is in §Validity.

### 3. `tokens.css` values now come from measurement, not invention

ADR-0025's direction D and "light only" both **stand**; what changes is **where the numbers come from**.
The old tokens were invented (a self-consistent set with no provenance); each new one records x2doris's
**measured computed value**, and the file header states the method. **To change one in future, go back
and measure**; do not tune by feel.

The largest changes: brand `#2C8AF0 → #3C7EFF`, row height `44 → 41`, controls `30 → 32`, table font
size `13.5 → 14`, tag radius `3 → 0` (square), cards **lose the 1px border** (layering is the white card
on grey ground alone), the four semantic colours swap to Ant Design 4's set, and a `--sider-*` family
plus density tokens such as `--head-h` / `--cell-pad` / `--topbar-h` are added.

**The one thing not copied is the font stack**: x2doris is an English interface with no CJK fonts in its
stack, while db-qbs serves Chinese users, so three Source Han / Noto CJK fallbacks are appended to the
same stack. This is **correcting a gap in the reference, not disobeying it** — on macOS and Windows the
original stack already reaches a CJK font; on Linux there is no fallback.

`web/src/app.css` `@import`s this file, so changing a token reskins the whole front end instantly; but
the many hard-coded values inside `app.css` (`.app-shell`'s 188px left column, `.data-grid`'s 12px
header, and so on) **do not follow automatically**, and aligning them one by one is the bulk of the
implementation work — see §10.

### 4. Division of labour between columns and detail: the list answers "did it work", the drawer answers "why"

Column order (ruling 3, verbatim): **☐ · task name · source table · target table · migration progress ·
run status · start time · duration · actions**. The task name carries a grey `task_id` beneath it, and
the source/target tables carry their datasource name beneath — two lines per cell, as the reference does.
The action column pins right (the reference's `ant-table-cell-fix-right`).

**The run-status column is one-dimensional**: 成功 / 失败 / 进行中 / 结局不明 / 尚未运行, all five as
**solid square tags**. This does **not** conflict with ADR-0025 §3's "the three shape axes must not be
swapped", **because it is not axis 2 at all** — it is one cell of index. Axis 2 (target-table effect) and
axis 3 (error code) **move wholesale into the drawer**, where they keep their original block and tag
shapes. This must be pinned in the walkthrough criteria, or the next person will judge this column by V9;
see "Walkthrough triggers · re-judging the V series" at the end.

**The detail drawer** (right side, 760px) holds everything about the task's **most recent** run:
plain-language conclusion bar + axis-2 block + axis-3 tag, row-count reconciliation, per-stage timings,
the task definition (primary key / conditions), run parameters and both ids, and the source SQL.
Nothing that was in the old run-history screen's expanded row is missing.

### 5. All inline actions are icons — ADR-0042 §6 is hereby void

Ruling 4 directly overturns ADR-0042 §6's "inline primary actions get text". That clause reasoned that
"picking out which one runs it from a row of four identically coloured icons relies on memorising
positions", which **does not hold in front of the reference**: the reference is all icons, and it uses two
dividers to split them into "run" ｜ "view / edit / rename" ｜ "delete", with delete alone tinted red.
Position is therefore **structured**, not memorised.

Icon semantics are carried by `title` (which is also the `aria-label`). The `.button.is-row-action` class
is deleted with it.

### 6. Bulk run and bulk delete

The checkbox column sits at the far left, with select-all in the header — selecting **the current page
only**, since selecting across pages would let someone act on rows they cannot see. The two bulk buttons
sit at the right of the card header and are **disabled when nothing is selected.**

- **Bulk run**: the front end calls the existing submit endpoint once per row, **serially**, without
  stopping on a failure, and summarises afterwards in one line: 「发起 5 个：成功 4，失败 1（交易流水：
  目标端不可达）」. Serial rather than concurrent, because submitting really does hit databases at both ends.
- **Bulk delete**: one confirmation step that **lists every task name to be deleted** before deleting,
  likewise serial and likewise summarised.

Neither needs a bulk endpoint on the backend. **None is added** — a bulk endpoint would have to define
partial-failure semantics and atomicity, which is a ticket of its own, while at field scale (dozens of
tasks) the cost of calling one by one is negligible.

### 7. Migration progress: `COUNT(*)` before the run, floored to an integer percentage

Ruling 6 is this release's **only backend change**: before starting, `source` runs one `COUNT(*)` against
the source (with the task's conditions applied) and records the total into the run record. The pushed row
count already exists, so the front end computes `floor(done / total * 100)`.

Four boundaries:

1. **Floor, do not round.** `100%` appears only when it has genuinely finished — showing 99.98% as 100%
   is lying with the display.
2. **No decimals, no row counts appended** (ruling 6's wording). A decimal point skews this column's
   alignment, and a second decimal place carries zero information about "how far along".
3. **A failed count does not block the run**: the total is recorded empty, the progress column falls back
   to `—`, and its `title` says so ("未取到总行数"). Failing an entire transfer for the sake of a progress
   bar trades the main function for decoration.
4. **A "not yet run" row shows `—`, not `0%`.** 0% means "it ran and moved no rows", which is not the same
   as "it never ran".

The cost is stated openly: **every submission adds one full-table count on the source**, which on a large
table takes real time, and there is a gap between it and the subsequent read — the denominator is the fact
at the moment of starting, not a live one. That count's duration is **recorded as its own stage timing**
(the "pre-run count" entry in the drawer) and is not mixed into the read duration.

ADR-0026 §3, "live progress shows no percentage and uses an indeterminate bar", **is overturned by this
clause**: its premise was "the denominator is unobtainable", and ruling 6 bought the denominator, so the
premise is gone.

### 8. The shell: dark sider + collapsible + white top bar

The sider is dark `#001529` at 256px wide, with 10px rounded menu blocks and the selected item filled with
the brand colour. Ruling 8's "light only" means **no dark theme and no theme switch** — the dark sider is
**part of the reference's light layout**, and the two do not conflict. This needs recording because
ADR-0025 explicitly rejected a dark theme and V25's criteria include "no dark theme".

The collapse trigger sits at the far left of the top bar (`menu-fold ⇄ menu-unfold`), 256px ⇄ 48px, with
the collapsed state showing icons only, identified by `title`. The collapsed state is **stored in
`localStorage`**: it is a one-off preference about one's own screen width, and resetting it on every visit
means remaking the decision every time.

### 9. Clauses this ADR overturns (one by one, unambiguously)

| Source | Original clause | Disposition here |
|---|---|---|
| ADR-0042 §Background | "Only the information architecture is copyable, not the visuals; `tokens.css` is untouched" | **Revoked.** Visuals are copied and tokens re-measured (§3) |
| ADR-0042 §6 | Inline primary actions get text, secondary actions stay icons | **Void.** All icons (§5) |
| ADR-0042 §3 | The task screen's "latest run" column | **Merged into this table**: once the two screens merge it *is* this table (§2) |
| ADR-0042 §1 §4 | The run-history screen's filter strip and column order | **Cancelled with the screen** (§2) |
| ADR-0026 §3 | Live progress shows no percentage, uses an indeterminate bar | **Overturned.** The denominator has been bought (§7) |
| ADR-0039 §9 / ADR-0042 §7 | The existing rule table in `app.css` | **Rewritten**, see §10 |
| Design system README §7 | Shell with a 188px white left nav / 52px top bar / 1px card border / 30px controls | **Rewritten** (§10) |

**What is not overturned**: ADR-0025's direction D and "no dark theme"; ADR-0025 §3's three shape axes and
their semantics (they move wholesale into the drawer with not one shape changed); ADR-0037's credential
boundary; ADR-0041 addendum 2's re-run entry point (see self-ruling 1 at the end); ADR-0042 §2's
"client-side paging that does not pretend to be server-side" and §5's inline datasource connection test.

### 10. The design-system ledger: **both `tokens.css` and the README change**

Unlike ADR-0042 §7, this round touches both files:

- `tokens.css`: the re-measurement of §3, **already landed**.
- `README.md` §7's component inventory: the shell row's 188px / 52px / white left nav, the card row's "1px
  border", and the filter row's 30px **each conflict head-on with P2** and must change. At the same time it
  gains six entries — **collapsed sider**, **in-card title block + toolbar**, **pagination strip**,
  **checkbox column**, **progress cell**, and **detail drawer** — and the "nav placeholder (`M3+` grey
  label)" row is marked **retired** (P0 removed the non-v1 nav placeholders). The **history list** entry
  under §7's "reserved placements" retires with §2.

`app.css`'s rule table is **rewritten wholesale** rather than amended: the hard-coded values across the
shell, tables, buttons, and cards must all move onto tokens.

**The real list after implementation (added 2026-08-21)**:

Added: `.app-shell.is-collapsed` (with three derived rules on `.product-brand` / `.nav-item`) ·
`.fold-toggle` · `.topbar-right` · `.filter-card` · `.filter-field.is-wide` · `.filter-actions` ·
`.table-card` · `.table-title-row` · `.table-title` · `.table-count` · `.table-toolbar` ·
`.toolbar-icons` · `.job-grid` · `.check-column` · `.table-cell` · `.table-side` · `.time-cell` ·
`.row-actions .divider` · `.progress` / `.progress-track` / `.progress-fill` (with `.is-ok` / `.is-bad`) /
`.progress-pct` · `.state` (with `.is-succeeded` / `.is-failed` / `.is-live` / `.is-unknown` / `.is-none`) ·
`.page-btn` (with `.is-active`) · `.page-size` · `.drawer-scrim` · `.drawer` · `.drawer-header` ·
`.drawer-body` · `.drawer-footer` · `.drawer-note` · `.drawer-sql` · `.panel` / `.panel-body` ·
`.kv` (with `.is-pairs`, `.k` / `.v` / `.v.is-bad`) · `.bulk-summary` (with `.is-failed`) ·
`.delete-copy ul` · `.settings-copy`.

Removed: `.latest-run-column` / `.latest-run` / `.latest-run-status` / `.latest-run-time` ·
`.button.is-row-action` · `.action-column.is-wide` · `.history-filters` · `.history-error` ·
`.history-grid` (with `.expand-column` / `.run-id-cell` / `.outcome-column` / `.is-expanded`) ·
`.history-link` · `.history-time` · `.history-detail` / `.history-detail-row` ·
`.history-metric-groups` · `.history-source-sql` · `.missing-run-id` · `.detail-toggle` ·
`.identity-grid` / `.metric-grid` · `.live-summary` · `.unknown-summary` · `.neutral-outcome` ·
`.pagination-page` / `.pagination-controls` · the `tabular-nums` on `.mono`'s `.history-link`.

Rewritten (values moved onto tokens, shapes unchanged): `.app-shell` / `.sidebar` / `.product-brand` /
`.nav-item` / `.topbar` / `.card` / `.button` / `.icon-button` / `.data-grid` / `.action-column` /
`.list-pagination` / `.modal*` / `.form-*` / the builder and run-detail page in full.

One old debt cleared along the way: `app.css` referenced `--lh-copy` in five places, and `tokens.css`
**never defined it** (the old file had only `--lh-ui` / `--lh-prose`), so those five line heights had always
been the browser default. The rewrite changes them to `--lh-prose`.

### Four self-made rulings during implementation (recorded here too)

1. **The third nav item, "Settings", is a read-only screen rather than a greyed-out placeholder.** Ruling 2
   fixed the nav at three items, and the only two kinds of thing this release can change already have homes
   (connection details on the datasource screen, task definitions in the builder). What remains is
   process-level configuration, two entries of which determine how this service itself starts — and opening a
   route to edit one's own startup parameters from an **unauthenticated** interface (ADR-0024, "the port is
   the credential") trades the main function for a settings page. So this screen presents **facts and
   locations**: what can be changed, where to change it, and the key names in `source.toml`. A writable
   settings screen needs authentication first, which is another ticket.
2. **The in-card toolbar keeps only "refresh"; no "row density" and no "column settings".** Those two icons
   are on the prototype because the reference has them, but this release has neither capability.
   **Better one icon fewer than a button that does nothing when clicked** — a fake button costs more than a
   missing one: it makes someone try it to find out.
3. **After changing the page size, the pagination strip no longer disappears when only one page remains.**
   ADR-0042 §2's "the whole strip is hidden when the total fits on one page" is **unchanged at the default
   setting**; the exception applies only once someone has explicitly changed the size. Otherwise, choosing
   100 per page so that the list fits on one page would make the very control that brought you there vanish,
   leaving no way back to 20.
4. **Bulk run fails tasks with run-time parameters on the spot rather than calling the backend.** The bulk
   surface has nowhere to enter parameter values, and sending an empty parameter set would only return the
   backend's "missing parameter". Rather than a round trip for a machine's sentence, it states plainly
   「需要填运行参数，请单独发起」. **Cost stated openly**: most tasks in the field carry a business-date
   parameter, so in this release bulk run is **essentially only useful for parameterless tasks** —
   **the owner ruled on 2026-08-21 to keep it as is** (reasoning and validity in "two closing rulings" at
   the end).

## Consequences

1. **One redesign flipped three sets of gate criteria at once.** The V series, the X series, and the README
   must all change with it, and this is not optional — criteria that do not follow leave the next walkthrough
   running a checklist written against an old interface, which is worse than not running it (`CLAUDE.md`
   rules 2 and 4).
2. **Every submission adds one source-side `COUNT(*)`.** At the field's 100MB scale (ADR-0041 ruling 6) that
   count takes seconds; should a large table appear later, the answer is **to give the count a timeout and
   degrade to "total row count unavailable"**, not to quietly drop the progress column.
3. **Multiple run-history records are temporarily invisible.** Not one has been deleted on the backend and
   `/api/runs` is untouched; only the interface shows just the latest. This is a cost ruling 2 explicitly
   accepted, not an omission.
4. **There is still lag between the list's run status and the real state** (ADR-0042 consequence 3 stands
   verbatim). This release still **does not poll**; the refresh button among the card header's tool icons is
   an explicit action.
5. **During the reskin the interface will briefly look like neither**: in the window where the tokens have
   changed and `app.css` has not, the values come from the new tokens while the layout comes from the old
   rules. So §10's alignment **must be finished in one go before merging**, never merged into trunk in
   batches.
6. **A dark sider + white top bar + grey content area means three sets of foreground colours.** The sider's
   set is listed separately as `--sider-*`, and `--text` / `--dim` **may not** be used on the sider — they
   are from the black family and are unreadable on `#001529`.

## Validity

| Clause | Signal to retire or re-evaluate |
|---|---|
| §2, only the latest run is shown | The field asks "I need to see how the previous run went" — at which point multiple history opens up, shaped as a run selector inside the drawer, **not as restoring the history screen** |
| §7, `COUNT(*)` before the run | A table appears where the count itself is slow enough to be felt (minutes) — at which point it degrades to optional, not to deleting the column |
| §6, serial bulk in the front end | Task counts reach the hundreds, or the field requires partial bulk failures to be retryable — only then is a backend bulk endpoint discussed |
| §3, token values | The reference releases a new version, or db-qbs grows a screen the reference does not have — **measure first, then change** |

## Walkthrough triggers

**All three fire, all must genuinely run, and no exemption is permitted** (`CLAUDE.md` rule 1):

- **V1–V25**: `docs/design-system/tokens.css` and `README.md` both changed, matching trigger 2 literally.
- **X series**: the job center replaces the task screen and the history screen, matching every existing
  trigger.
- **W1–W6**: rewriting `app.css` wholesale touches the `.precheck-reports` layout, matching the trigger.

### Re-judging the V series (one by one, without renumbering)

First, an **error in the handoff document** to correct: V1–V25 judge **shape and semantics**, and **not one
of them judges numbers** like 44px / 13.5px / a radius / `#2C8AF0` — changing a token's number cannot by
itself fail any of them. What genuinely changes is the four below, because of the screen merge and the
weight:

| # | Re-judgement | Reason |
|---|---|---|
| V9 | **N/A (the criterion retires with this ADR).** It originally judged "blocks and grey text coexist in the run-history outcome column; it looks uneven, and making it even would be a lie" — the whole list screen is gone, and the new list's run status is a **one-dimensional index** (§4), where five uniform words are **correct**. Axis 2's "unevenness" criterion transfers wholesale to the detail drawer, carried by V2 / V3 / V7 / V8 | The subject is gone and the direction is reversed |
| V14 | **N/A (the criterion retires with this ADR).** It originally judged "`run_record_id` is the first, primary, clickable column of the run-history list" — that list is gone. The criterion about the **relation** between the two ids moves into the drawer: `run_record_id` beside the drawer title, `run_id` in "run parameters and identifiers" in monospace grey one size down | The subject is gone; the relational criterion is kept with a new anchor |
| V24 | **Second half N/A.** "The builder is not a standalone nav item" **still runs**; the subject of "scheduling is only a greyed-out `M3+` placeholder" was removed back in P0 (ADR-0042 §Background), and this ADR records that silent retirement. **New criteria added**: three nav items, a dark sider, and a collapsed state showing icons only **with the icons centred** (see self-ruling 2) | Half the subject is gone, half is new |
| V25 | **Re-judged**: the emphasis weight changes from "**600, not 700**" to "**500** (measured w500 on headers and title blocks)". The rest still runs — CJK on the system font stack (with CJK fallbacks appended), numbers on the monospace stack with `tabular-nums`, and **no dark theme** (a dark sider is not a dark theme, see §8) | `--weight-em` now comes from measurement |

V1–V5 / V7 / V8 / V11 / V13 / V15 / V16 / V17 / V19 / V20 / V22 / V23 have **criteria unchanged by one
character**; only "which screen to open" reads "run detail" as "**the run detail drawer**".
The N/A cases V6 / V10 / V12 / V18 / V21 stay as they are.

### Handling the X series: no renumbering, retired cases become N/A, new cases start at X13

The handoff document suggested "rewrite the X series along with the job center". **It is not rewritten**;
the rule added in ADR-0040 applies: nothing is renumbered, no row is deleted, a case whose subject is gone
becomes `N/A (the criterion retired with ADR-0043)`, and new shapes get **new numbers.**
The reasoning matches the M2 precedents of A3/A6 and V6/V10: old walkthrough records must keep matching up,
and renumbering would leave the X1–X12 record of 2026-08-21 matching nothing when it is next opened.

The new cases must cover at least: the presentation of the job center's eight columns, the checkbox and the
disabled states of the two bulk buttons, the flooring of the progress percentage and its three empty states,
the tag shape of the five run-status words, the drawer's sections and re-run entry point, and the sider's
two collapse states.

### The rig follows

`v1-probe.py`'s `HISTORY_COLUMNS` constant retires with the history screen and becomes the job center's
column constant; `v1-mock.py`'s `X_BULK` stub data must add total and pushed row counts (or the progress
column has no subject); `run-x-walkthrough.sh` keeps its two-pass arrangement, with the bulk pass also
serving to observe bulk selection.

## Self-made rulings (two, recorded here)

1. **The re-run entry point does not vanish with the history screen**: a "re-run" button at the bottom of the
   detail drawer preserves ADR-0041 addendum 2 and spec #149 section A. The history screen was the only entry
   point for re-running, and removing the screen without catching the entry point would mean casually voiding
   someone else's ruling.
2. **Centre the icons when collapsed; this one detail is not copied**: x2doris does not adjust its menu items'
   `padding-left: 20px` when collapsed, so the selected blue block is sliced into a vertical strip by the
   48px width. That is its **rendering defect**, not its design.
   What is copied is the design, not the bugs — and the test is written here: **you may skip a detail only if
   you can say exactly how it is broken and your fix introduces no new shape.**

## Implementation record (2026-08-21)

All three mandatory commands and all three walkthroughs genuinely ran. The per-case verdicts are below; the
three `*-20260821T051427Z.md` records are kept only in git history under the record-retention policy
(`CLAUDE.md` §Record retention), and
`git log --diff-filter=D -- 'docs/spikes/fixtures/local-rig/*-20260821T051427Z.md'` retrieves the originals.

| Walkthrough | Result |
|---|---|
| V1–V25 | 17 criteria met, 6 N/A, **2 with no subject (V19 / V20)** |
| X1–X18 | 18 criteria met, half a case N/A (the first half of X11) |
| W1–W6 | 3 criteria met, **3 with no subject (W3 / W4 / W5)** |

`npm run typecheck` / `npm test -- --run` (132 passed) / `npm run build` all green;
`cargo test --workspace` green. Real runs are always dispatched to the user's mac (the server lacks memory).

### Something the walkthrough caught: `47a2fed` removed the builder's column-fetch card without running a walkthrough

`47a2fed` (*Prepare x2doris P1 frontend handoff*) deleted the entire "target DDL / fetch DDL /
`.fetch-ready` / `.ddl-placeholder`" section from the builder, split the field-mapping surface out of the
column-fetch table, and swapped the target field from an input to a dropdown. The consequences:

- **The subjects of V19 / V20 and W3 / W4 / W5 disappeared wholesale** — five criteria with nothing left to
  look at.
- **X6's literal criteria differ in two places**: "a permanent input box" measures as a permanent dropdown,
  and "the primary key sits to the right of the target field" measures as two columns away ("target type"
  and "constraint" in between). The criterion's **intent** (permanent rather than an edit state; the primary
  key visible on the same row as the target field) still holds.
- `api.ts`'s `fetchColumns()` (`POST /api/columns`) is still there, with **no call site anywhere in the
  interface**.

**This is not a regression caused by P2** (the subjects were still present in `33e9ec5` and in v1's
acceptance at `85805b1`), but it is **a regression the gate failed to catch**: `CLAUDE.md` rule 1 exists
precisely to stop "the interface changed and no walkthrough ran".

### The owner's two closing rulings of 2026-08-21

**Ruling one: the column-fetch card is hereby void, and V19 / V20 plus W3 / W4 / W5 become N/A.**

That deletion in `47a2fed` is **treated as an intentional narrowing**, not something to roll back. V1
therefore **does not provide target DDL**: the user still creates the target table in the target database
themselves (ADR-0027's "the product will not create your table" stands verbatim); the product simply no
longer assembles that statement for them. What is lost is the convenience of "a directly executable
`CREATE TABLE`", and **no judgement is lost** — the mapping precheck still hard-rejects on the sink side
(ADR-0009), and the target-column reference table (`target-columns-title`) is still there, still listing
column names, types, lengths, nullability, and constraints.

In the walkthroughs: V19 / V20 in `m2-visual-walkthrough.md` and W3 / W4 / W5 in `m3-visual-walkthrough.md`
are **rewritten as `N/A (the criterion retired with this ADR)`, with nothing renumbered and no row deleted**
(per the rule added in ADR-0040, for the same reason as V6 / V10 / V12: old walkthrough records must keep
matching up). `api.ts`'s `fetchColumns()` and the backend's `POST /api/columns` **both stay** — an endpoint
is part of the protocol surface and is not deleted merely because the interface has no caller for now; the
function is annotated "currently has no UI caller".

**Validity**: when the field says "I do not know how to create a table from the types you gave me", the
conversation about restoring target DDL reopens, shaped as a "generate the DDL from this mapping" action
beside the target-column reference table — **not** as bringing the whole column-fetch card back.

**Ruling two: bulk run stays as it is** (see §10, self-made ruling 4). Tasks with run-time parameters fail on
the spot with a prompt to submit them individually, so in this release it is **useful only for parameterless
tasks**. The owner's reasoning: better that it be useless for that class of task than that it ever run
production data with a stale date.

**Validity**: when the field genuinely needs to run parameterised tasks in bulk, the answer is **a dialog at
bulk time collecting one set of parameters for all selected tasks** (those whose parameter names do not match
still go individually) — **not** "re-run each with its own most recent parameters", which would re-run with
last time's date.
