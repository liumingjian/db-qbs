# UX Review — db-qbs Web Console

**Date:** 2026-08 · **Build reviewed:** POC at `http://10.250.0.24:18088/` · **Reviewer:** Claude Opus 5

---

## 1. How this review was produced

The console was driven live with Playwright at three viewport widths (1440 / 1280 / 820) and
captured across all four navigation screens, all four wizard steps in both empty and prefilled
states, the run drawer, and every dialog the app can raise. Twenty screenshots back the findings
below. In parallel, four independent review passes read the source; a finding was only promoted to
this report if it was reproducible in the running app *and* traceable to a specific line of code.
Every claim here carries either a `file:line` reference or a measured number.

Nothing in this report is speculative about intent: where the code documents a deliberate trade-off
(e.g. `SqlEditor.tsx` explaining why `wrap="off"` is required), the report says so and proposes a
change that respects the constraint rather than pretending it does not exist.

## 2. Scope agreed with the product owner

These decisions were settled before writing and bound everything below:

| Question | Decision |
| --- | --- |
| Who uses this | DBAs and developers with solid database skills, **high frequency** |
| Scale | ≤10 datasources; target database has a few dozen tables |
| Screens | Mostly 27" desktop monitors; other media possible |
| Visual direction | **Polish the existing token language.** No new design language |
| Wide screens | **Hybrid**: table screens centre-constrained to ~1800px; wizard and run detail fill the width |
| Dark theme | Not this round — but keep tokens themeable (no hard-coded hex outside `tokens.css`) |
| Upsert semantics | **Copy-only fix.** No new "truncate then reload" mode |
| Run history | Not needed. Keep "most recent run only" |
| Keyboard shortcuts | Not needed |
| Custom SQL | Users **paste** it in from elsewhere |
| Task list order | Creation time, descending |

## 3. Severity model

- **P0** — the interface causes a wrong decision, or stands between the user and production data
  with insufficient weight. Fix before the release demo.
- **P1** — costs a high-frequency user time or a retry on every single use.
- **P2** — design-system convergence. Individually cosmetic; collectively the difference between
  "internal tool" and "product".

---

# P0 — Fix before the demo

## P0-1 · `目标表已切换` describes a swap that never happened

**Evidence:** `web/src/components/DesignSystem.tsx:37` renders `目标表已切换` for
`effect === "SWAPPED"`; `web/src/history.ts:165` appends `，暂存表已切换为目标表`. The sink
actually issues `INSERT ... ON DUPLICATE KEY UPDATE` — `crates/sink/src/mysql_destination.rs:566`
and `:609`. `CONTEXT.md:186-189` states the truth plainly: *"Rows deleted at the source do not
disappear at the target — an upsert only writes, never deletes. That is a deliberate debt."*

**What happens:** after a successful run the console tells the operator the target table was
*switched*. The mental model that phrase creates is atomic replacement: what is in the target now
is what was in the source. What is actually in the target is the **union** of every row that has
ever been loaded, with matching primary keys overwritten. A row deleted at the source on Monday is
still sitting in the target on Friday, and the console has said "已切换" four times in between.

**Why it is P0:** this is the only finding in this report where the interface can cause a DBA to
certify a target table as correct when it is not. Everything else costs time; this one costs trust
in the data.

**Change (copy-only, per the agreed scope):**

- Terminal block: `目标表已切换` → **`已按主键合并写入`**, with the secondary line
  `按主键 upsert：新增和变更已写入；源端删除的行仍保留在目标表`.
- `history.ts:165`: `，暂存表已切换为目标表` → `，已按主键合并进目标表`.
- Wizard step 4 (confirmation) gains one standing line above the action buttons, stating the same
  thing *before* the first run rather than after it.
- `DISCARDED` keeps `目标表未被触碰` — that one is accurate.

**Known remaining debt:** copy-only means the target table can still be a union of old and new
result sets. This fix changes the interface from *misleading* to *honest about a limitation*; it
does not remove the limitation. That is the owner's explicit call and is recorded here so the next
reader does not re-open it as a bug.

## P0-2 · Danger weight is inverted across the whole app

Three separate controls, ranked by how much production data they can destroy versus how much
friction they impose:

| Control | Data at risk | Friction today |
| --- | --- | --- |
| `批量发起` (`JobCenterScreen.tsx:418-425`) | Rewrites **N** production tables | **None.** One click |
| `清理本次写入` (`RunDrawer.tsx:67-68`) | `DELETE` from a production table | Native `window.confirm` |
| `批量删除` (`JobCenterScreen.tsx:426-433`) | Deletes task *definitions* | Full modal listing every name |

The most destructive action has the least friction and the least destructive has the most. The
`BulkDeleteDialog` (`JobCenterScreen.tsx:869-914`) is genuinely well built — it lists every task
name in full. It is simply attached to the wrong action.

**Changes:**

1. **`批量发起` gets a confirmation modal** listing each selected task as `任务名 → 目标表`, so the
   operator reads the list of production tables about to be written before writing them. Primary
   button `发起 N 个任务`.
2. **Single-row `发起运行` stays one-click.** This is a high-frequency action on a task the user
   already configured and confirmed; a per-row dialog would be pure tax. Deliberate asymmetry.
3. **`清理本次写入` graduates from `window.confirm` to a real `Modal`** (`ui.tsx` already has one).
   It must state three things the native dialog cannot: the fully-qualified `库.表`, the row count
   about to be deleted, and — in its own emphasised line — **`清理是删除，不是还原`**. The current
   copy, `确定清理这一次运行写入的数据？此操作不可撤销。`, is the weakest possible framing: "清理"
   reads like tidying up. Confirm button: `删除这 N 行`.
4. **Danger styling gets a real hierarchy.** `app.css:100` gives `.is-danger` a white background
   with a red border, and the *confirm* button inside `BulkDeleteDialog` uses the same class as the
   toolbar button that merely *opens* it. Add a filled `.is-danger.is-solid` variant (red fill,
   white text) reserved for the button that actually commits destruction; demote the toolbar
   trigger to a ghost.

**Update (#256): change 3 is moot.** 「清理本次写入」and the write ledger behind it were removed
whole; the run drawer has no such button any more. Changes 1, 2 and 4 stand — the danger hierarchy
they introduced is still what 批量发起 / 批量删除 use. This paragraph stays because the review is a
record of what was decided, not a to-do list.

## P0-3 · The confirmation page renders "never checked" as "passed"

**Evidence:** `web/src/wizard.ts:1168` — `findings: checkIsFresh(draft) ? draft.check!.value.findings : []`.

**What happens:** if the target-table check was never run, or its inputs went stale
(`checkIsFresh` fails after edits — see `wizard.ts:791-818`), the findings list collapses to `[]`.
Step 4 renders an empty findings list identically to a clean one: **`已通过`**. The two states that
must never be confused — *"we looked and it is fine"* and *"we never looked"* — are pixel-identical
on the last screen before the user writes to production.

Worse, `wizard.ts:1151-1156` already computes an `excused` string explaining precisely why the check
could not run (`目标端 Agent「x」不在线，这一步查不了；保存不受影响，运行要等它回来`). **No component
reads it.** The reasoning exists, is correct, and is discarded before it reaches the screen.

**Change:** make the check a three-state control on step 4 — `已通过` (fresh, no findings) /
`有 N 处需要处理` (fresh, findings) / **`尚未检查`** (`state === "none"` or `"stale"`), the last
rendered in a neutral-warning tone with the `excused` reason printed underneath when present, plus
an inline `立即检查` button. Never let the absence of information render as good news.

## P0-4 · `结局不明` strips exactly the evidence needed to resolve it

**Evidence:** `RunDrawer.tsx:107-114` renders the `结局不明` block; `RunDrawer.tsx:160` gates
`FailureEvidence` on `kind === "failed"`, and `RunScreen.tsx:282` does the same. `UNKNOWN` is
neither `ok` nor `failed`, so it falls through both gates and gets no evidence panel at all.

**What happens:** the one outcome that *requires* a human to go and look at the database is the one
outcome the console tells nothing about. The operator is informed the result is unknown and handed
no target host, no schema, no table name, no staging table name — the exact four facts needed to go
reconcile it by hand.

**Change:** render `FailureEvidence` for `UNKNOWN` as well, retitled `核对线索` and containing the
target address, `库.表`, the staging table name, and the last known row count. Add one sentence that
answers the question every operator will otherwise ask in chat: **`重跑是安全的——写入是按主键幂等的`**.
That single line converts a support ticket into a button press.

## P0-5 · No modal or drawer manages focus

**Evidence:** `ui.tsx:29-37` — the `Modal` installs an Escape-key handler and nothing else. There is
no focus trap, no initial focus, and no focus restoration on close. `RunDrawer` has the same gap.

**What happens:** opening a modal leaves keyboard focus behind it on the page. Tab walks out of the
dialog and into the obscured background, where the focused control is invisible but still
activatable. Closing the dialog drops focus back to `<body>`, so the next Tab restarts from the top
of the document. For a screen-reader user the dialog is effectively not announced as a dialog.

This is the only genuine accessibility defect in the app severe enough to sit in P0 — and it applies
to every destructive confirmation added in P0-2.

**Change:** in `ui.tsx`, focus the first interactive element on mount (or the element carrying
`autoFocus`), trap Tab/Shift-Tab within the dialog, restore focus to the invoking element on close,
and set `role="dialog"` + `aria-modal="true"` + `aria-labelledby` pointing at the title. Apply the
same to `RunDrawer`. One implementation, shared.

---

# P1 — Costs a high-frequency user time on every use

## P1-1 · The job centre hides the reason it exists

**Measured at 1440px:** `scrollWidth = 1639`, `clientWidth = 1132`. **507px of every row is off
screen.** What is off screen: `迁移进度`, `运行状态`, `启动时间`, `运行时长` — 100% invisible without
horizontal scrolling. At 1280px `目标表` goes too.

The file's own doc comment (`JobCenterScreen.tsx:42`) states the design intent: *"一行 = 一个任务 +
它最近一次运行"*. The run half of that sentence is the half you cannot see.

Contributing causes, each independently fixable:

- Nine columns (`JobCenterScreen.tsx:657-669`) with `white-space: nowrap` on every cell
  (`app.css:120`) and `min-width: 1080px` on the grid (`app.css:119`) — nothing may ever compress.
- `源表` spends ~453px rendering a custom-SQL fragment that is truncated before its `FROM` clause,
  so the widest column in the table conveys nothing.
- A 32-character hex `task_id` occupies line two of **every** row (`JobCenterScreen.tsx:700-701`).
- The sticky action column (`app.css:182`) is opaque and paints over `目标表` mid-glyph, with no
  ellipsis to signal truncation — text simply ends.
- Meanwhile ~1500px of a 27" monitor is empty margin.

**Changes:**

1. **Reorder** to put outcome before configuration: `任务名 · 运行状态 · 迁移进度 · 目标表 · 源表 ·
   启动时间 · 运行时长 · 操作`.
2. **Cap `源表` and `目标表`** with `max-width` + `text-overflow: ellipsis`, full value in `title`.
   For custom SQL, render a `自定义 SQL` chip plus the first identifier, not a raw fragment.
3. **Remove `task_id` from the row.** It belongs in the run drawer and on hover. It is a debugging
   affordance being paid for by every row, on every screen, forever.
4. **Centre-constrain the table to ~1800px** (agreed hybrid strategy) so a 27" monitor gains real
   columns instead of margin.
5. **Give the sticky action column an opaque background matched to the row's hover state** so it
   reads as a deliberate overlay rather than a rendering bug.

## P1-2 · The wizard opens on three red errors before you have done anything

**Evidence:** `wizard.ts:865, :871, :874` produce `请先选一张源表`, `请先选目标表——字段映射要对着它才有意义`,
`至少要选一列` on an empty draft. `TaskWizardScreen.tsx:728` renders them in `.wizard-mapping-problems`
with `role="alert"` — which fires on mount, announcing three failures for a form nobody has touched.

The text directly above already says the same thing neutrally, so the screen states its to-do list
twice: once as guidance, once as accusation.

**Change:** split blockers into **to-dos** (a field is empty) and **errors** (a field is filled and
wrong — `wizard.ts:897-908, :917`: duplicate source columns, duplicate targets, missing primary key).
To-dos render as a neutral checklist with no `role="alert"`; errors keep red and the live region.
Red must mean *you did something wrong*, or it will be ignored by the time it matters.

## P1-3 · The custom-SQL editor is a 145px box in a 300px column

**Evidence:** `app.css:460` — `.wizard-context .source-sql-editor .sql-text-input { min-height: 145px }`,
inside a 300px-wide context column. `SqlEditor.tsx` sets `wrap="off"` deliberately: the highlight
layer and the textarea must align pixel-for-pixel or the caret drifts, and soft wrapping breaks that
alignment. The documented cost is horizontal scrolling.

Users **paste** SQL in from elsewhere. A pasted 40-line query lands in a viewport showing roughly
7 lines by 34 characters, with horizontal scrolling on every one of them. This is unusable for its
actual primary use.

**Change:** when `fetchMode === "sql"`, the editor **leaves the 300px context column and takes the
main area** — the schema tree is irrelevant in this mode anyway (see P1-9). In its new home it gets:
line numbers, a **soft-wrap toggle** (off by default, preserving the alignment contract; when on,
apply identical wrapping to both layers so the invariant still holds), a `格式化` button, and a
fullscreen affordance. Minimum height ~420px. The `WHERE` editor (`app.css:371`, `min-height: 92px`)
grows to ~160px and gains the same treatment at smaller scale.

Read-only SQL — `.ddl-output` (`app.css:442`) and `pre.drawer-sql` (`app.css:235`) — currently
renders with **no highlighting at all**, while the editable field is fully coloured. Reuse the
existing `.sql-highlight` renderer for both; the tokens are already defined (`tokens.css:65`).

## P1-4 · A target table that does not exist yet is a dead end

Three guards compound into an unrecoverable state:

- `TaskWizardScreen.tsx:589` — the target table can **only** be chosen from the fetched list.
- `TaskWizardScreen.tsx:565` — the "refresh target columns" button is
  `disabled={draft.spec.target_table === ""}` with the title `先选择目标表`. You cannot refresh until
  you have selected, and you cannot select what has not been fetched.
- `TaskWizardScreen.tsx:762-765` — the DDL block only renders for a table that already exists.

`CONTEXT.md:329-336` puts automatic target-table creation explicitly out of V1: DDL is generated for
a human to run. That is a reasonable boundary — but the flow that makes it workable is missing. The
user must abandon the wizard (destroying the draft — see P1-5), run DDL elsewhere, and start over.

**Changes:**

1. Allow a **typed target table name** alongside the tree. If it is not in the fetched list, show a
   `尚不存在` chip rather than an error.
2. **Generate DDL for that not-yet-existing table** from the current column selection — this is
   exactly the case the DDL feature is for, and it is the one case it refuses to handle.
3. Decouple the refresh button from `target_table`: refreshing the *table list* has no dependency on
   having selected one.
4. Once the user has run the DDL, `刷新` finds the table and the draft continues — no restart.

## P1-5 · Every exit from the wizard destroys the draft

**Evidence:** `App.tsx:370-374` — `commitNavigation` unconditionally nulls the wizard draft.

A task definition is 10–15 decisions including a hand-pasted SQL query. Any navigation — including
one triggered to go look up the table name the wizard would not let you type (P1-4) — discards all
of it.

**Change:** persist the draft to `sessionStorage` (`App.tsx:87` already establishes the
localStorage pattern for the sider) and change the guard dialog's primary button from a plain
confirm to **`保留草稿并离开`**, with `丢弃草稿` as the secondary. Returning to the wizard resumes.

## P1-6 · The run detail has no address

**Evidence:** `App.tsx:163` — `activeRun` is React state overlaying the current page. The hash
router's `Page` union has no run entry, so run detail is not a route.

Consequences for a high-frequency user: it cannot be bookmarked, cannot be shared into chat ("look
at this failure"), cannot be reopened after a refresh, and browser Back closes it in a way that also
leaves the page beneath it.

Compounding this, `RunScreen.tsx:349-376` renders `PrecheckReports` — the per-column mapping
precheck table, one of the most useful diagnostic surfaces in the product — **only** on the full run
screen, which is reachable only through `App.tsx:435` inside `handleStart`. If you did not start the
run in this browser tab, you cannot see it.

**Changes:** add a `#/runs/<task_id>` route; make the in-row run indicator a real link to it; and
render `PrecheckReports` in `RunDrawer` as well, so the diagnosis is reachable from the list.

## P1-7 · The target-check step is a one-sentence screen

Step 3 auto-runs when step 1 completes, so in the happy path the user clicks `下一步` onto a screen
that says one sentence and clicks `下一步` again. A pure tax on the most common path, paid every
time a task is created.

**Change:** fold the check result into step 4 as a status block (which P0-3 is already reshaping).
Expand it to a full step **only** when the check has findings or could not run. Four steps become
three in the happy path, and the extra screen appears exactly when it has something to say.

While there: `app.css:474-475` styles `is-current` and `is-done` identically, so on step 4 all four
rail markers are solid blue and the rail cannot answer "where am I". Give `is-current` a filled
marker with a ring, and `is-done` an outlined marker with a check.

## P1-8 · `已用时 00:00` for the first minute of every run

**Evidence:** `RunScreen.tsx:245-253` — live metrics read zero throughout `PREPARING`, which was
observed lasting ~54s. `RunScreen.tsx:251` labels `detail.ms` as `已用时`, but `RunScreen.tsx:291`
labels the *same field* `累计批次耗时`. They are not the same quantity, and the first name is wrong:
it is batch time, which is legitimately zero before the first batch.

Meanwhile `progress.ts:191-196` treats `total_rows === null` as an unconditional pre-count failure
and says so in the tooltip — including during `PREPARING`, when the count is merely *not finished
yet*. The console fabricates a failure that has not happened.

A run that shows `00:00 · 迁移进度 —` with a tooltip claiming the count failed is indistinguishable
from a hung run. The user's only recourse is to stop and retry a run that was working.

**Changes:**

- `已用时` reads real wall-clock time since run start; the batch figure keeps the name
  `累计批次耗时` in both places.
- Add **`最后动静：N 秒前`** driven by `last_ts` — which is already fetched and already unused. This
  is the actual liveness signal and it is being thrown away.
- Branch the `迁移进度 —` tooltip on phase: during `PREPARING`, `正在统计总行数`; only after
  `STREAMING` begins does `总行数统计失败，进度按已写入行数展示` become true.
- `RunScreen.tsx:161-166` prints `将在页面可见时继续读取。` on **every** poll failure, including
  network errors on a visible page. Say that only when `document.hidden`.

## P1-9 · Step 2 is an empty screen in custom-SQL mode

In `自定义 SQL` mode the source schema tree has nothing to show, so step 2 renders empty. Combined
with P1-3, the wizard devotes a 300px column to a tree that is inert while cramping the editor that
is doing all the work.

**Change:** in SQL mode, step 2 shows the **result-set preview** (columns and types derived from the
query) instead of the tree — the information the user actually needs to map columns — and the layout
re-flows per P1-3.

## P1-10 · Create mode locks the datasource pickers

**Evidence:** `TaskWizardScreen.tsx:503` and `:568` both wrap the datasource `<select>` in
`draft.mode === "edit" &&`. In create mode the datasource is fixed by the entry dialog and cannot be
changed inside the wizard — you must cancel out (destroying the draft, P1-5) and start again.

Related: `TaskWizardScreen.tsx:345-356` — `fetchTargetTables` is keyed only on datasource ids, so it
never refetches. A table created in another window during a long wizard session stays invisible
until reload.

**Changes:** show both selects in create mode (changing one clears exactly the dependent state that
`wizard.ts:306, :330-345, :385` already knows how to clear). Add the datasource-version/list to the
fetch key, or simply refetch on the manual refresh button once P1-4 decouples it.

The pre-creation entry dialog itself should go: `entry.ts:179-188` auto-selects when there is exactly
one option, and this shop has ≤10 datasources with most tasks flowing between the same pair — so for
most users it is an empty interstitial. Keep it **only** when it has something to block on: the gate
evaluation at `entry.ts:66-93` (a genuinely well-built piece of logic) and the offline-agent
reasoning at `entry.ts:132-145`.

## P1-11 · The list's stop button ignores the reasons it already has

**Evidence:** `runStage.ts:294-320` computes `abortRefusal` with three distinct, well-written
reasons for why a run cannot be stopped. `troubleshooting.ts:9-17` — `rowRunAction` returns "stop"
unconditionally and never consults it. The user clicks stop, gets a 409, and learns the rules by
trial and error.

Also in that file, `troubleshooting.ts:24-44` routes `MAPPING_FAILURES` to step 3 (mapping is step
1) and `SOURCE_FAILURES` to step 2 (the source *datasource* is the right destination). 8 of 15
failure kinds return `null` and offer nothing at all.

**Changes:** feed `abortRefusal` into the row action so the button is disabled with the real reason
in a `title` — using the `<span title>` wrapper pattern that `RunDrawer.tsx:276-289` already gets
right (a `title` on a `disabled` button never displays; `TaskWizardScreen.tsx:642-653` has the same
latent bug). Fix the two routing targets. Give the eight silent failure kinds a default next step
pointing at the run drawer's evidence panel.

## P1-12 · System settings does not deserve a nav slot

`app.css` and `App.tsx:110-115` give `系统设置` one of four top-level navigation slots. It contains
build metadata a high-frequency user reads approximately once. `App.tsx:534` also hard-codes a
`当前工作台` chip that displays a constant.

**Change:** demote to an `关于` popover under a top-right icon; return the slot to the three screens
that carry the work.

---

# P2 — Design-system convergence

Individually cosmetic. Together they are why the app reads as an internal tool rather than a
product. All are in-language per the agreed direction: these tighten the existing tokens, they do
not replace them.

| # | Finding | Evidence | Change |
| --- | --- | --- | --- |
| P2-1 | Two card-header systems coexist | `app.css` | Converge on `.card-header`; delete the ad-hoc variant |
| P2-2 | Icon sizes are ad hoc; row actions are 24px while every other control is 32px | `app.css:103` | Three tokens: `--icon-sm/md/lg`. Row actions to 28px minimum — they are the highest-frequency targets in the app |
| P2-3 | No type scale: headings and body are both 14px | `tokens.css` sizes 14/14/14/12/12 | Ladder to 20/16/14/12. Titles must outrank body text |
| P2-4 | Native unstyled `<select>` in the primary creation form | `TaskWizardScreen.tsx:503` | Bring `select` into the form-control styles (height `--ctl-h`, `--radius`, matching border) |
| P2-5 | `--mute` ≈3.5:1 carries the datasource name — below AA for body text | `tokens.css` `rgba(0,0,0,.45)` | That column moves to `--dim` (≈7:1). Keep `--mute` for genuinely secondary text |
| P2-6 | Entry-dialog agent status measured ≈2:1 | `TaskEntryDialog.tsx` | Raise to the semantic ink tokens (`--ok-ink` and its counterparts) |
| P2-7 | Chinese labels set in a Latin-only mono stack | `app.css:261`, escapes at `:402, :604, :612` | `--font-num` for numerics and identifiers only; `--font-cn` for all Chinese |
| P2-8 | Modal footer scrolls away in long dialogs | `app.css:251` | `position: sticky; bottom: 0` on `.modal-footer` with a top border |
| P2-9 | Two different scrim opacities | `app.css:217` (.45) vs `:250` (≈.29) | One `--scrim` token, applied to both |
| P2-10 | Segment-timing `.kv` grid is fixed-column; numbers left-aligned | `app.css` | `auto-fit` columns; right-align numerics with `--font-num` so magnitudes line up |
| P2-11 | Mapping chips use `--badge-radius: 0` against a 4px-radius product | `tokens.css` | Match `--radius` unless the square is load-bearing |
| P2-12 | Nothing responds between 760px and infinity | `app.css` breakpoints 760/620/480 | Add the ~1800px container cap (P1-1) and a 1280px column-priority tier |
| P2-13 | `role="tree"` over plain `<button>` children | `TaskWizardScreen.tsx:535, :588` | Either implement `treeitem` semantics or drop to `role="listbox"`/`option`, which matches the actual behaviour |
| P2-14 | Filtered-empty list is a dead end: `没有匹配的任务` with no exit | `JobCenterScreen.tsx:633` | Add `清除筛选`. The unfiltered empty state (`:618-628`) is good — match its quality |
| P2-15 | Row actions shift position: `运行详情` renders conditionally | `JobCenterScreen.tsx:757-812`, `:787` | Reserve the slot. For high-frequency users, positional muscle memory is the whole value of an icon row |
| P2-16 | Pagination hides under one page except when the user picked 50/100 | `ui.tsx:216-222` | Keep the page-size control visible whenever the list has ever exceeded one page |
| P2-17 | `目标表需要处理` is the default label for a null state | `TaskWizardScreen.tsx:748` | Name the actual condition, or fall back to `尚未检查` (P0-3) |

---

# 4. Explicitly out of scope this round

Recorded so they are not re-raised as oversights:

- **Run history.** Owner's decision: most-recent-run-only is sufficient. `RunDrawer.tsx:20-33`
  already documents this as intentional.
- **Keyboard shortcuts.** Not wanted.
- **A "truncate then reload" load mode.** P0-1 is copy-only by decision.
- **Dark theme.** Not this round — but no new hard-coded hex outside `tokens.css`, so it stays cheap
  later.
- **A new visual language.** The x2doris/Ant Design 4-derived token set stays. Everything above
  fixes execution within it.
- **Auto-collapsing the sider below 1280px.** Users are on 27" desktops; the wasted width at the
  *top* end is the real problem, and P1-1 addresses it.

# 5. What was already right

Worth stating, because several of these are better than the surrounding code and should not be
casualties of the rework:

- `entry.ts:66-93` — gate evaluation. Clear, complete, correctly ordered.
- `runStage.ts:294-320` — `abortRefusal`'s three distinct reasons. The best state modelling in the
  app; P1-11 only asks that it be *used*.
- `JobCenterScreen.tsx:869-914` — `BulkDeleteDialog` lists every affected name. P0-2 does not
  improve it; it copies it to where it is needed more.
- `RunDrawer.tsx:276-289` — the `<span title>` wrapper for disabled buttons, done correctly. Make it
  the house pattern.
- `JobCenterScreen.tsx:618-628` — the unfiltered empty state.
- `SqlEditor.tsx` — the alignment invariant is documented at the point where a future editor would
  otherwise break it.

# 6. Suggested sequence

1. **P0-1 and P0-2** — copy plus two dialogs. Small, and they are the two that carry data risk.
2. **P0-3, P0-4, P0-5** — all three are about surfacing reasoning the code already computes, or
   sharing one focus implementation.
3. **P1-1** — the job centre. Highest daily return of anything in this report.
4. **P1-3, P1-4, P1-5, P1-9** — the wizard as one piece of work; they touch the same layout.
5. Remaining P1.
6. **P2** as a single design-system pass, ideally in one commit so the tokens move together.

# 7. Appendix — screenshot index

Captured 2026-08 against the POC with Playwright 1.62.1.

| Screenshot | Screen |
| --- | --- |
| `01-jobs` / `13-jobs-1280` / `12-jobs-narrow-820` / `20-jobs-scrolled-right` | Job centre at 1440 / 1280 / 820, plus scrolled right |
| `02-datasources` / `30-datasource-form` | Datasources, list and form |
| `03-agents` / `31-agent-form` | Target agents |
| `04-settings` | System settings |
| `05-run-drawer` | Run drawer |
| `06-entry-dialog` / `07-entry-dialog-filled` | Pre-creation entry dialog |
| `08-wizard-step1` / `22-wizard-step1-filled` | Wizard step 1, empty and prefilled |
| `23-wizard-step2` / `27-wizard-step2-preview` | Wizard step 2 |
| `24-wizard-step3` / `26-wizard-step3-checked` | Wizard step 3 |
| `25-wizard-step4` | Wizard step 4 |
| `21-edit-guard` / `32-rename` / `33-delete-confirm` | Guard, rename and delete dialogs |
