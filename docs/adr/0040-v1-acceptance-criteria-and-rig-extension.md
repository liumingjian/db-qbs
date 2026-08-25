# ADR-0040: v1 acceptance criteria and the rig's fourth entry point — 100k rows / 100MB is already met by M1, the only new dimension is peak memory, and all three existing rigs need their criteria re-derived

**Status**: Accepted
**Date**: 2026-08-19
**Ticket**: [#122](https://github.com/liumingjian/db-qbs/issues/122) (the sixth and last ticket on map [#117](https://github.com/liumingjian/db-qbs/issues/117))
**Precedent**: `ADR-0028` (M2, the first time), `ADR-0032` (M3, the second) — this ADR is the third, and follows them

## Background

The first five tickets of the v1 closed-loop map are all settled: `ADR-0035` (primary-key upsert),
`ADR-0036` (structured `TaskSpec`),
`ADR-0037` (datasources and credentials),
`ADR-0038` (column mapping and the target-side column face),
[ADR-0039](0039-v1-ui-increments.md) (UI increments). This ticket decides: **what makes those five count as
accepted, and how the rig extends.**

### Three facts assumed when the ticket was opened; two of them flipped on inspection

| What the ticket assumed | What inspection found |
|---|---|
| "100k rows / about 100MB" needs a new fixture | **Flipped.** M1's `wide-100k` already exceeds it — see §1 |
| "should the three existing rigs be re-run" is an open question | **Flipped.** Not a question — all three **currently fail against the new API**, so they must change; see §5 |
| "should M3's W1–W6 be re-run" is decided by #123 | **Trigger source changed.** #123 indeed did not touch it, but `ADR-0036` §5 did; see §6 |

### Two owner rulings of 2026-08-19 (inputs to this ADR)

1. **The db-qbs server runs on a dedicated machine with ample memory.**
2. **100k rows is a nightly batch; it can take as long as it takes.**

## Decision

### 1. No new fixture for "100k rows / ~100MB" — M1's `wide-100k` already exceeds it by 3.3x

`t_m1_wide` in `acceptance/oracle.sql` is **68 `VARCHAR2(48)` columns + `NUMBER(8)` + `DATE`, 100,000 rows**,
about **3.3 KB per row**. The measured batch-size distribution in the 9/9 report of 2026-08-16 converts directly:

> `wide.jsonl`: 100000 rows / **21 batches**, batch size min/p50/max = **14.5 / 16.78 / 16.78 MB**
> → on-wire payload for one import ≈ **336 MB**

"About 100MB" is **already exceeded on the row-width axis**, and on the real transfer path rather than on a probe.
**Building a fixture that lands exactly on 100MB is pure waste** — it is weaker than the existing case, and passing
it would prove nothing the existing case does not already prove.

**Therefore:**

- Requirement ⑤ of v1 is met by **M1's `wide-100k` plus the memory assertion in §3 of this ADR**, and
  **only both together**. The row-count and row-width half lives in M1, the memory-shape half in the new entry
  point. **Neither half is duplicated anywhere.**
- **The M1 report must gain a "payload accounting" line**: source row width (bytes/row), batch count,
  batch-size p50, estimated total payload. Without it nobody will later be able to see where "about 100MB"
  was met — the data was always there, nobody had read it as evidence for this criterion.
- Success criterion 1 in `STRATEGY-V1.md` ("a 70+ column, 100k-row wide table runs end to end")
  **is the same thing said differently**: 68 columns + 2 = 70 columns, not a coincidence. **No separate criterion.**

**No elapsed-time criterion** (owner ruling 2). The rig's Oracle runs under an amd64 emulation layer, so absolute
seconds never counted under `ADR-0005`; on site this is a nightly batch, so duration is not an experience
constraint either. **Reports record the measured seconds as usual — read for trend, never judged pass/fail.**

### 2. A fourth entry point `run-v1-acceptance.sh`, with scenario numbers in the C series

Following the M3 precedent in `ADR-0032` §2: **start a new one, do not stuff scenarios into an existing entry point.**
The reasoning is unchanged from M3 — an entry point's scenario set is **a constant of its milestone**, and adding to
it makes historical anchors like "9/9" and "B1–B6" drift over time, so old reports stop matching.

**Numbering**: M1 has no letter (the scenario name is the number), M2 is **A1–A14**, M3 is **B1–B6**, v1 is **C1–C6**.
Letters are globally unique and are **never reused or renumbered, under any circumstances**.

> **One difference from the M3 case — do not copy it wrong**: M3's line that "9/9 is a constant unaffected by later
> milestones" **does not hold this time**. v1 changes **the transfer semantics themselves** (the write model becomes
> upsert) rather than adding a capability on top. The **criteria** of all three existing rigs must therefore be
> re-derived — **numbers and scenario counts stay put, the assertion wording and expected values change** (§5).

### 3. The memory assertion: measure `VmHWM` / `ru_maxrss`, compare the slope across two levels, judge each process separately

This is the only genuinely new acceptance dimension in v1. The criterion must be **a number that can be judged
PASS/FAIL automatically**, not a paragraph of observation — otherwise it is not a gate.

#### 3.1 What to measure: the kernel's own high-water mark, not a sample

- **source (`db-qbs-source-run`, a one-shot process)**: the wrapper takes the child's **`ru_maxrss`** via `wait4()`.
- **sink (a long-lived process)**: read **`VmHWM`** from `/proc/<pid>/status` after the run finishes.

**Both are kernel-maintained monotonic high-water marks, not polled samples.** This is the premise the criterion
rests on: a peak is a point event, and sampled RSS probing will miss it — and a missed peak makes the criterion
**falsely green**, which is worse than having no criterion.
(`spike-bulk` already reads `VmHWM`, and `acceptance/m2-source-run-wrapper.py` already has the wrapper; copy both.)

#### 3.2 Who to measure: source and sink **separately**, never combined

The two ends have **two independent memory risks**, and one combined number lets them mask each other:

- **source** is the streaming reader — the risk is "the batching buffer grows with total row count".
- **sink** is the staging-table writer and in-transaction swapper — the risk is "`INSERT ... SELECT` or the swap
  transaction holds a whole batch in memory".

#### 3.3 How to judge: subtract the baseline, compare slopes, factor of 2

Run the same wide table (`t_m1_wide`) once at **10k rows** and once at **100k rows** (a **10x** data difference), then assert:

```
peak(100k) - baseline  <=  2 x ( peak(10k) - baseline )
```

**Judged once for source and once for sink; both must be green to PASS.**

- **`baseline` = the high-water mark once the process is up and connected but has not yet moved a single row.**
  Without subtracting it, the process's fixed overhead (runtime, connection pool, Instant Client) dilutes the slope
  into a false green — a process with 200MB of fixed overhead gives a ratio near 1 even if the data part really does
  grow linearly.
- **The factor 2**: purely linear gives **10x**, purely constant gives **1x**. **2** is the criterion for
  "definitely not linear", with headroom for the three things that will certainly happen: batch buffering,
  allocator fragmentation, and glibc not returning pages to the kernel.
  **It is a shape criterion, not a performance metric** — do not read it as a yardstick for "how far memory has been
  optimised"; that is an M4-and-later concern.
- **All four absolute numbers (two processes x two levels) go into the report verbatim**, plus the baselines.
  Writing PASS without the numbers leaves no history to compare against when the factor is later loosened or tightened.

#### 3.4 No absolute ceiling (owner ruling 1)

On site this is a dedicated server with ample memory. An absolute ceiling would just be a number pulled from the air,
and the first time it was hit it would be loosened on the spot —
**a criterion loosened on the spot once is no longer a criterion**.

> **Expiry**: if the deployment shape changes to co-existing with the legacy system on one machine, this clause needs
> an absolute ceiling set from that machine's real headroom. The trigger is a **change in deployment shape**,
> not a larger memory number.

#### 3.5 sink is long-lived, so it must be restarted between the two levels

`VmHWM` is the high-water mark **for the process's lifetime**, and only rises across runs. Without a restart, the
second level reads the first level's residue, the ratio is identically 1, and **the criterion is permanently
falsely green**. The script must restart sink between the two levels, and **the report must record that the restart
actually happened**.

### 4. The C series: six scenarios covering the new capabilities of the first five tickets

| No. | Scenario | Assertions | Source |
|---|---|---|---|
| **C1** | Datasource CRUD + connection test | ① create one Oracle source and one MySQL target; ② **a connection test with wrong credentials fails → it cannot be saved** (the negative case of owner ruling 2); ③ the API view **returns not even the ciphertext**; ④ deleting a datasource referenced by a task → **409, and the body names the referencing tasks**; ⑤ renaming only skips the connection test | ADR-0037 §6/§7, ADR-0039 §3/§4 |
| **C2** | Column mapping and the target column face | ① map a source column to a **differently named** target column, then verify target data by the **target name**; ② the default prefill is the identical name (identity mapping); ③ call `/v1/target/tables` and `/v1/target/columns` once each, closed set does not grow | ADR-0038 §2/§3 |
| **C3** | User-supplied filter conditions | ① one constant condition and one "fill at run time" condition, one run each, row counts change as expected; ② in the generated SQL **every value is a bind variable, constants included**; ③ the UI has no hand-edit-SQL entry | ADR-0036 §1/§2 |
| **C4** | Idempotence of primary-key upsert | ① **run the same run twice, the target row count does not change**; ② `staged <= affected <= 2 x staged`; ③ `purged_rows` is always **0**; ④ change one source column value and re-run → that column **is updated** on the target (proving upsert, not INSERT IGNORE); ⑤ **target table without PK/UNIQUE → precheck refuses to run** (the silent-degradation guard of ADR-0035) | ADR-0035 §1 |
| **C5** | Three negative cases for the three branches of the mapping precheck | ① a primary-key column nullable on the target → **reject**; ② a mapped non-key column nullable → **allow**; ③ an unmapped column with neither `COLUMN_DEFAULT` nor `EXTRA` → **reject** | ADR-0038 §5 (which assigns "the three negative cases" to #122) |
| **C6** | Memory shape | The two slope assertions of §3, one for source and one for sink | §3 of this ADR |

**C4's assertion ④ may not be dropped.** Verifying only "run twice, row count unchanged" would also pass under
`INSERT IGNORE` — and `INSERT IGNORE` silently swallows a changed source value, exactly the class of
**silent value change** that `ADR-0034` §1b names. An unchanged row count is necessary, not sufficient.

**No C7 for "100k/100MB"**, for the reason in §1: that criterion lives in M1, and setting up a second one only
creates two sources of truth that will each drift.

### 5. The three existing rigs: all must change, all must be re-run, criteria re-derived clause by clause

All three scripts **currently fail against the new API** (their headers say so themselves, and all three assign the
fix to this ticket). This section nails down **which assertions have flipped in meaning** —
**the implementation ticket follows this table rather than re-deriving it.**

#### 5.1 M1 (9 scenarios; numbers and scenario count unchanged)

| Scenario | Current criterion (the DELETE era) | New criterion (upsert) |
|---|---|---|
| `wide-100k` / `narrow-100k` | row count = 100000 | **unchanged**, plus the payload-accounting line of §1 |
| `empty-result` | `purged_rows = 7` (empty result set → 7 rows for that day are purged) | **The meaning inverts entirely**: `purged_rows = 0`, and the target's rows for that day **are untouched**. Under upsert an empty result set means doing nothing |
| `source-kill-rerun` | after the re-run the target hash returns to `baseline` (the sentinel is purged) | **The sentinel survives**: after the re-run the hash is `target_with_sentinel`. Its primary key does not collide, so upsert never touches it |
| `sink-kill-rerun` | same as above | same as above |
| the other 4 | unrelated to the write model | unchanged |

> `empty-result` and the two kill scenarios are **three faces of one premise**: "idempotence = re-flushing the whole
> business-date range". ADR-0035 replaced that premise, so those three assertions **are not rewordings — their
> conclusions invert**. Do not just change the numbers.

#### 5.2 M2 (14 A scenarios; numbers unchanged, no renumbering)

The call surface moves from the retired `source_sql` / `biz_date` payload to `TaskSpec` + a bound datasource id.
On the criteria side there is exactly one inversion:

- **`A3-column-fetch-shape-failure` and `A6-run-shape-failure` lose their subject** —
  `ADR-0036` §5 removed the SQL shape precheck entirely (the generator structurally cannot produce a bad shape).
- **Ruling: keep the numbers, skip them in the script, and mark them `N/A（判据已随 ADR-0036 §5 退役）` in the report.**
  **Do not delete the numbers, do not renumber, do not promote another scenario into the slot.** Numbers are
  historical anchors; renumbering breaks the old reports of 2026-08-16. And an N/A row stating "retired, and by what"
  answers "where did A6 go" far better than a vanished number.

#### 5.3 M3 (B1–B6; numbers unchanged)

The call surface **has not been touched at all** (all six still use the `source_sql` / `biz_date` shape) and must move
to the new payload wholesale. On the criteria side:

- **B1's "the sentinel is deleted" → "the sentinel survives"** (same reason as M1's two kill scenarios).
- **B4 / B6's "the sentinel survives" is unchanged** — they were refused-to-run cases all along, so the target was
  never touched.

#### 5.4 Re-run bar

**Once changed, all four of M1 / M2 / M3 / C run.** The map #117 clause "anything touching transfer semantics must run
the three rigs" is **promoted to four** for v1.
`M2_HOST_CARGO_TARGET=x86_64-apple-darwin` is required; anything that actually runs goes to the mac.

### 6. Walkthroughs: V1–V25 on its own ticket, run once and sealed; W1–W6 re-runs but with a different trigger; new screens get their own X series

#### 6.1 The whole of V1–V25: it is the acceptance of the "README correction" ticket, placed after the UI implementation tickets

[ADR-0039](0039-v1-ui-increments.md) §9 already ruled that the whole of V1–V25 fires **on the literal wording of
`CLAUDE.md`, taken on the chin with no exemption sought**. This ADR **does not reopen that**; it only fixes when it
runs and on which ticket:

- **The two factual corrections to `docs/design-system/README.md` become one minimal implementation ticket of their own**,
  placed **after the datasource screen and builder-control tickets, and before overall v1 acceptance**.
- **V1–V25 runs once on that ticket and is sealed on completion**, rather than being re-run on every UI ticket.
- **The reason is that the thing being corrected has to exist first**: what README §5's "no connection-config page"
  should become, and which line §7's placeholder needs, both depend on **what the datasource screen and the target
  dropdown actually look like**. Placed earlier, it corrects a fact that does not yet exist; placed later it corrects
  accurately and runs the walkthrough exactly once.
- **No quiet skipping inside the implementation tickets** (ADR-0039 §9's own words). If that ticket's acceptance
  report lacks 25 individual observations, it is not done.

#### 6.2 M3's W1–W6: it re-runs, and the trigger is ADR-0036 §5

ADR-0039 §9's self-check conclusion — "the `.precheck-reports` layout and the `DiagnosticTable` column structure are
unchanged" — **holds for #123 but not for v1 as a whole**:

> `ADR-0036` §5 removed the SQL shape precheck, so **`.precheck-reports` now holds only the mapping precheck
> section** — precisely the "change to the `.precheck-reports` layout" that the M3 gate in `CLAUDE.md` names.

**Ruling: W1–W6 re-runs, recorded on the implementation ticket that removes the shape precheck.**
W2's contrast clause ("the `shape-failed` state still places the two panels side by side") was already corrected on
2026-08-19 under #121; the wording now in `m3-visual-walkthrough.md` is the wording to use for the re-run, **unchanged**.

#### 6.3 New screens: a separate `v1-visual-walkthrough.md`, numbered X1–X8

Following the M3 precedent in ADR-0032 §8 — **a separate checklist file, not additions to the V series**. The V series
is M2's design-system fixed point; adding numbers to it makes "the whole of V1–V25" — the very gate object §6.1 must
run — drift over time.

Numbered **X1–X8**, in `docs/spikes/fixtures/local-rig/v1-visual-walkthrough.md` (delivered with this ticket).
All form criteria come from ADR-0039; **this ADR creates no new form ruling.**

### 7. The two visual-gate sections of `CLAUDE.md` merge into one (the map records #122 as where this lands)

The existing M2 and M3 sections each repeat the same three things (trigger condition / checklist file / no bare pass
claims), and v1 would make it a third repetition. **Merge into one "visual gates" section whose body is a table**:
trigger condition → checklist file → number range.

**The merge changes only the organisation; not one gate loses any force.** All three original clauses are preserved
verbatim: ① "any change fires it, no exemptions"; ② "record actual observations, never a bare pass claim";
③ "if nothing fired, say 'not run' and why".

## Corrections and additions to existing documents

| Document | Change |
|---|---|
| `CLAUDE.md` | the two visual-gate sections merge into one and gain the v1 X series (§7); #122's consolidation lands here |
| `docs/spikes/fixtures/local-rig/v1-visual-walkthrough.md` | **new**, X1–X8 (§6.3) |
| `docs/STRATEGY-V1.md`, v1 acceptance section | "criteria and rig entry point to be settled by #122" now points at this ADR, and states that requirement ⑤ is met in two places (§1) |
| `docs/spikes/fixtures/local-rig/scripts/run-m{1,2,3}-acceptance.sh` | the two "assigned to #122" TODO comments in the headers become pointers to §5 of this ADR once the implementation ticket lands |
| `docs/adr/0032` §8 | unchanged. "A separate checklist, no V1–V25 re-run" was M3's ruling; v1's answer differs (§6.1), and **the two do not conflict: the trigger conditions differ** |

## Known costs

1. **The v1 acceptance surface goes from three rigs to four**, raising the cost of a full run by about a third.
   Accepted: the write model changed, and leaving the three existing rigs un-re-derived means green against stale
   semantics — worse than slow.
2. **The whole of V1–V25 must be run**, 25 individual observations. Accepted, for the reason in ADR-0039 §9
   (a gate's credibility is worth more than the convenience saved this once).
3. **M2's A3/A6 remain N/A rows permanently**, so two rows in the report no longer participate in pass/fail.
   Accepted: what it buys is numbering stable across milestones, so old reports stay comparable.
4. **The memory criterion governs shape only, not absolute size.** Without a ceiling to hit, "the peak is always high
   but genuinely does not grow" goes undetected. Accepted: the site is a dedicated server with ample memory
   (owner ruling 1), and §3.4 already states the condition for reopening.
5. **C6 needs two levels and two sink restarts**, making it the slowest item in the C series. No cheaper substitute —
   a slope criterion needs two points by definition.

## Expiry

- **§3.4's absolute ceiling**: if the deployment changes to co-existing with the legacy system on one machine, add an
  absolute ceiling derived from the real headroom.
- **§3.3's factor of 2**: three consecutive v1 acceptances well under 2 (ratio < 1.3) allow tightening to 1.5;
  **loosening on a hit is expressly forbidden** — any loosening must first explain where the extra memory went.
- **§1's payload accounting**: currently an estimate derived from the batch-size distribution. If an exact total
  payload is ever needed, add a cumulative byte field to `run_finished` — **not in v1**, the estimate is enough to
  support the "about 100MB" criterion.
- **Elapsed time**: recorded but not judged (owner ruling 2). Only if a "must also run during the day" requirement
  appears does duration need promoting to a criterion.

## Addendum (2026-08-19, while running the whole of V1–V25 under #133): six V criteria have lost their subject or inverted — mark N/A, keep the numbers

### Background

§6.1 placed the whole of V1–V25 on the README-correction ticket
([#133](https://github.com/liumingjian/db-qbs/issues/133)), to run once and be sealed. The actual run hit something
this ADR had not anticipated: **six items in the V series no longer have a subject** — not "not achieved", but three
v1 ADRs removed the thing they describe, or expressly inverted it.

| Item | What the criterion says | Current state | Who ruled it |
|---|---|---|---|
| **V6** | the shape-precheck failure screen shows no final-state block | the shape precheck is gone entirely; the screen cannot be produced | ADR-0036 §5 |
| **V10** | two stacked cards plus a grey "not executed" placeholder card | only one section remains; `.is-skipped` was removed from `app.css` by #132 | ADR-0036 §5 |
| **V11, first half** | the first card lists all six shape-precheck rules | as above. **The second half (the mapping-precheck card laid out column by column plus a total) runs as usual** | ADR-0036 §5 |
| **V12** | the shape-precheck screen carries no error-code tag | the screen does not exist | ADR-0036 §5 |
| **V18** | the persistent badge and "replace whole section" confirm modal for hand-edited SQL | source SQL is now derived live from the structured spec and is **read-only**; the hand-edit entry is gone | ADR-0036 §2 |
| **V21** | there is **no** target-table dropdown and **no** target-column list, and the screen says so explicitly | both are expressly required, and shipped; that copy was deleted when the criterion was overturned | ADR-0038 §3 / ADR-0039 §5 |

**V15 is not among them**: its criterion (the `run_id` field reads `未发起，目标端不知道这次运行`, neither blank nor a
dash) is unchanged word for word; only the means of producing that state (shape-precheck failure) is gone.
**It runs with a different means (the source failing before it sends the request)**, and the checklist's "situation"
column is reworded to "any failure that never issued a request to sink".

### Decision

1. **No renumbering, no deleted rows**; the criterion row is marked in place as `N/A（判据已随 ADR-XXXX 退役）`.
   This copies §5.2's handling of M2's A3/A6 — **numbering stays stable across milestones, and old walkthrough
   records stay comparable**. What V6/V10/V11/V12/V18/V21 each said in the four V-series records of 2026-08-16
   remains findable.
2. **Walkthrough records write the N/A honestly and name what retired it; never "passed", never a quiet skip.**
   This is the same requirement as ADR-0039 §9's "no quiet skipping inside implementation tickets": a criterion
   losing force is a fact to be accounted for, and is a different thing from "not run this time".
3. **The whole of V1–V25 remains one gate**: N/A is **the state of a criterion**, not **an exemption for one run**.
   Next time a trigger fires, these six are still confirmed one by one to have no subject — any of them may be
   revived by a later ADR overturning something.
4. **Do not delete the six and issue new numbers**, and **do not add numbers to the V series**. §6.3 already fixed
   the V series as M2's design-system fixed point, and adding numbers makes "the whole of V1–V25" drift as a gate
   object. Form criteria for v1's new screens all live in X1–X8 in `v1-visual-walkthrough.md`
   (the two things V21 denied are X5 / X7 themselves).

### Why not simply delete them

Deleting rows looks tidier, at three costs: old records stop lining up; "why did this one disappear" becomes a git
excavation; and, worst, **the gate shrinks over time** — once a 25-item checklist is down to 19, nobody notices the
next two going. **Keep and annotate, so the death of a criterion leaves a trace.**

### Expiry

- **If a later ADR reopens the subject of some N/A** (say a shape precheck is wanted again),
  **drop the N/A in place and revive the criterion as written**, without a new number.
- **If N/As keep accumulating past v1** (another five, say), the V series has outlived its usefulness as M2's fixed
  point, and the answer is **a separate V2 checklist with V1–V25 explicitly retired as a whole**, not more patching
  inside it.

## Addendum (2026-08-19, while changing the three existing rigs under #134): §5's three tables missed three cases, nailed down here

### Background

§5 lists "which assertions have flipped in meaning" in three tables for the implementation ticket to follow.
The actual work hit three criteria that **are not in the tables but have flipped just the same** — not new judgements,
but premises §5 overlooked. Recorded here one by one, so that "why does this differ from §5" has an answer.

### 1. §5.1 says "the other 4 are unrelated to the write model" — that fails for the two `commit-disconnect` scenarios

Both produce their state with **an empty result set plus a same-day sentinel**, and assert "the day's range is purged
= the diagnosed SWAPPED really did land on the target". That assertion **rests entirely on DELETE semantics**: under
upsert an empty result set means doing nothing, so whether the swap happened is **indistinguishable on the target**,
and the scenario degenerates into an assertion that is always true — worse than no assertion.

**Ruling**: numbers and scenario count unchanged, **the state is produced with a primary-key-colliding sentinel** —
write a row with `ROW_ID = 1` on the target first, so it collides with the first row of the source result set:

| Scenario | New criterion |
|---|---|
| `commit-disconnect` | after diagnosing `SWAPPED`, the value at `ROW_ID = 1` **is overwritten by the source value** (`M1-00000001`), 100000 rows in the whole table |
| `commit-disconnect-discarded` | after diagnosing `DISCARDED`, `ROW_ID = 1` **is still `discard-sentinel`**, 1 row in the table, 0 staging tables |

The **intent** of the criterion is unchanged (it still proves "what each final state does to the target"); what
changed is **the means of observing it**. Same root as #133's handling of V15: **a criterion must be measurable, and
so must the means of observing it.**

### 2. B1's `N_EXPR` and B2's `C_EXPR` lose their subject — expression columns structurally cannot enter v1

Both columns are SQL expressions (`n_bare * 1`, `v_text || v_text`). After ADR-0036 §2, SQL is derived live from the
spec and the projection structurally emits only `a.C AS C`, with no hand-edit entry in the UI — the line in
`oracle_source.rs` ("metadata correction for expression columns was deleted with ADR-0036 §5:
**expression columns cannot enter v1 at all**") is this clause's counterpart in code.

**Ruling**: follow §5.2's precedent for A3/A6 — **numbers unchanged, records marked N/A naming what retired it.**
Two places:

- **B1**: `N_EXPR` leaves the projection, and the "numeric expression column" item is marked N/A. The bare-NUMBER
  case is still held by `N_BARE` as before, so no coverage is lost.
- **B2**: `C_EXPR` leaves the projection, and the problem count goes **10 → 9** (the other one is below; the final
  number is **8**).

**Do not substitute a virtual column**: `GENERATED ALWAYS AS (v_text || v_text)` describes as `VARCHAR2(20)` — the
length **is determinable**, so the `CharacterLengthMissing` rule never fires at all. Substituting it in brings a case
that looks the part but proves something else, which is worse than an honest N/A.

### 3. B2's `EXTRA` criterion points the wrong way — ADR-0038 §4 relaxed the column-name test to a subset test

`EXTRA` originally asserted `源端结果缺少同名列`. After ADR-0038 §4 relaxed "the two column-name sets are exactly
equal" to **a subset test**, an unmapped **nullable** target column is no longer a problem.

**Ruling: invert the assertion in place** — assert that `EXTRA` produces no problem at all. This is worth more than
deleting it: the subset relaxation itself needs a positive case guarding it, and `EXTRA` is one already to hand.

**Do not turn `EXTRA` into a `NOT NULL` column without a default** to restore the old problem count. That case
(the rejecting side of §5's third branch) **belongs to C5** (the table in §4), and setting up a second one in M3 gives
one criterion two sources of truth that will each drift — the same reason §1 refuses a C7 for "100k/100MB".

**B2's problem count is therefore 8**: `BF` / `BD` / `PAYLOAD` / `C_CHAR` / `N_TOO_WIDE` / `N_TOO_SCALE` /
`N_MISSING` / `D_WRONG`; the other eight rules are unchanged.

### 4. Incidentally: all three rigs' target tables gained primary keys, and M3's B2/B3 gained a `ROW_ID` column

A primary key is mandatory (owner ruling of 2026-08-18), and the sink-side precheck requires the target to
**actually carry** a `PRIMARY KEY` / `UNIQUE` whose column set matches the chosen primary key (ADR-0035 §2). The six
`M3_B*` tables had no unique constraint at all, and B2/B3 had no column that could serve as one. **This is not a
change of criteria, it is the rig fixture catching up to the new write model's premise** — without it all six
scenarios are refused by the "the target table must carry a PRIMARY KEY or UNIQUE constraint" precheck, and B2's count
would not be the eight problems it is meant to prove.

`column_precision` is dropped from M3's task definitions at the same time: it left the task definition with
ADR-0036 §6, and a bare NUMBER's `(p,s)` now **comes from the target DECIMAL column** (`range_check_columns` in
`precheck.rs`), so B1/B4 neither need nor may configure it.

### Expiry

- If the two scenarios in item 1 ever return to "re-flush the whole range" semantics (ADR-0035's expiry clause 1
  being overturned), the means of producing the state flips back, **with the numbers still unchanged**.
- If hand-edited SQL or expression projections are reopened after v1, item 2's two N/As are **dropped in place and the
  criteria revive as written**, without new numbers — word for word the same as #133's addendum expiry clause.

## Addendum (2026-08-19, while landing the C series under #135): the choreography of the memory criterion, what the C series can and cannot prove, and two owner rulings on usage

### Background

§3 and §4 fixed the **criteria** (the formula, what to measure, the factor 2, what each of the six scenarios asserts),
but left the **choreography** to the implementation ticket — #126 states ⑨ is the one place among ten implementation
tickets where a choice remains at implementation time. Five things only became visible while writing the code, and
are nailed down here; two more are owner rulings of 2026-08-19 on **how this check gets used**.
**Not one criterion changed**; what changed is how it gets measured and how the report is written.

### 1. The baseline is **a same-process reading taken once per level**, not one global constant

§3.3 says "two baselines" (one for source, one for sink). That does not hold in implementation:

- **sink is a new process at each level** (§3.5 requires a restart between levels). Baselines across processes are
  not comparable — connection pool, buffer pool and allocator state are all new, and subtracting the first level's
  baseline from the second level's peak subtracts another process's overhead.
- **source is a one-shot process**, so it never even has the option of reusing a baseline across levels.

**Ruling: the baseline follows the level, one measurement per level, and all four baselines go into the report.**
The formula is unchanged; the `baseline` in it is simply the reading from **the same level and the same process**.
This is stricter than a global constant, not looser — the fixed overhead is subtracted more cleanly.

### 2. The source baseline can only come from a run that **really runs and moves zero rows**

`ru_maxrss` is retrieved by `wait4()` **when the child exits**, and cannot be read while the process is alive.
So the source baseline cannot be "read it once it is up" the way sink's can; it can only be **a real run of zero rows**:
connect, load Instant Client, create the staging table, complete the in-transaction swap — **just without moving a
first row**.

The rig produces that run with the constant condition `ROW_ID < 1` (same table, same projection, same path, only the
row count is 0). **The report must verify that this run's `source_rows` really is 0** — if it moved rows, it is not a
baseline but a data-contaminated starting point, and the resulting slope is too small and the criterion too loose.

### 3. C6 gets its own target table `V1_WIDE`; the source table stays `t_m1_wide`

"The same wide table" in §3.3 means **the source table**, and that is followed: C6's source is M1's `t_m1_wide`, with
no new fixture (§1's reasoning: a new one would only be weaker).

**The target end gets its own identically shaped `V1_WIDE`**: C6 leaves 100k rows in the target table, and stacking
that on `M1_WIDE` makes the next M1 rig start from someone else's residue. M1's `scenario_wide` does `DELETE` first,
true, but sharing one target table across four rigs is a premise that **holds only while the execution order happens
to be right**, and is not worth keeping.

### 4. `ru_maxrss`'s unit is not a cross-platform constant — the report records the raw value and the normalised one

**On macOS `ru_maxrss` is bytes; on Linux it is kB.** The rig's source runs on the host mac and its sink in a Linux
container, both are read, and a number without its unit is simply wrong.

The wrapper therefore records three things: the raw value, the platform, and the value normalised to bytes; the four
absolute numbers in the report are **always bytes**. When the same scripts are one day run on a Linux deployment,
this section is the only evidence that explains itself.

### 5. A zero denominator makes the criterion always true — reject it explicitly, never pass silently

`peak(100k) − baseline <= 2 x (peak(10k) − baseline)`: when the 10k increment is **0**, the right side is 0 and the
left side usually is too, so the criterion is **always true**. And a zero increment at 10k has exactly one
explanation — **the measurement is broken** (the baseline was read too late, the wrapper did not attach, the reading
came from another process) — not a good product.

**Ruling: the script explicitly asserts both 10k increments are > 0, and FAILs otherwise.**
Same reasoning as #138 refusing to relax the range's lower bound to 0 and #134 refusing to produce
`commit-disconnect` from an empty result set: **an assertion that is always true is worse than no assertion.**

### 6. Three C criteria have half their subject in the UI — the rig proves the protocol half, and the report states the boundary

C1② ("only a successful test allows saving"), C2② ("default prefill of the identical name") and C3③ ("no hand-edit
SQL entry in the UI") have subjects **partly in the front end** (the dialog's save gate, the builder's prefill,
whether that input box exists), which a command-line rig cannot reach.

**Ruling: the rig proves the protocol half and names the other half as belonging to the X walkthrough; no merging,
no impersonating.** Specifically:

| Criterion | What the rig proves | What belongs to the X walkthrough |
|---|---|---|
| C1② | the test with wrong credentials really fails, and no row is added afterwards | the dialog's "only a successful test allows saving" gate itself |
| C2② | the column face returns the full set of source column names (the prefill's input) | that `target` in the builder really is prefilled with the source column name |
| C3③ | the task definition refuses raw SQL (`deny_unknown_fields` → 400) | that the UI has no hand-edit SQL control |

**The report gives this boundary its own section.** A report claiming six things proved while really proving five and
a half is more dangerous than one that plainly says "this half belongs to the walkthrough" — the former leaves people
believing the gate reaches there.

### 7. Two owner rulings of 2026-08-19 on usage

1. **No teardown by default after a run.** The two datasources and the differently-named-mapping task built by C1/C2
   are exactly the data more than half of X1–X8 needs; tearing them down means building them by hand again.
   Pass `--clean` to tear down (opt-in, not the default). **The opposite of M3's `M3_KEEP_RIG`** — keeping the rig was
   the exception there and is the norm here, because v1's walkthrough checklist runs right behind the four rigs.
2. **The report opens with a five-row table of "the customer's five requirements → where each is verified → this run's
   result".** C1–C6 are split by technical module and do not map one to one onto the customer's five (requirement 5
   especially: row count and row width are in M1's `wide-100k`, memory shape in C6, split across two places by §1).
   This report is not only for whoever changes the rig; it is also the evidence shown to the customer that v1 is done,
   so the report must answer the mapping itself rather than leave the reader to assemble it from the ADR.

### 8. The range criterion for `swapped_rows` is **a mirrored pair** — both the sink and the source side must be ranges

ADR-0035 §4 changed the `swapped_rows` criterion from equality to the range `[staged, 2 x staged]`: under
`ON DUPLICATE KEY UPDATE` MySQL counts 1 for an insert and 2 for an update, so on any re-run where an existing row's
value really changed, `affected_rows` necessarily exceeds `staged_rows`. The sink side, `mysql_destination.rs`, was
changed at the time; **the mirrored assertion on the source side in `transfer.rs` was missed, and the equality test
survived until today**.

The consequence is not a rig problem, it is **the product blowing up on the v1 main path**: a re-run changing 1 of 5
rows gives `swapped_rows = 6 != staged_rows = 5`, so the task is judged `FAILED`. And "primary-key upsert, a re-run
updates only the changed rows" is the customer's fourth requirement. C4④ is what caught it.

**Ruling: the `swapped_rows` criterion is a two-sided pair; changing either side changes the other at the same time.**
This is not a soft "remember to sync" convention — the two assertions have one MySQL semantic as their subject, and
they are **either both ranges or both equalities; a diverged state is wrong on any given day**, it just waits for a
row to actually change before it blows up. Wherever else one semantic carries two assertions, whoever changes one
owns changing its mirror, and review checks against this clause.

### 9. ADR-0038 §5's third branch also constrains **the swap statement's column set** — the swap may cover only mapped columns

ADR-0038 §5's third branch states the precheck half: **unmapped but the target column has a default → allow**.
The runtime half was empty at the time, so the swap statement took **all** the target's columns — unmapped columns are
NULL in the staging table, and the swap wrote that NULL straight into the target, hitting
`ERROR 1048 Column cannot be null`. **Precheck allows, runtime explodes**: two semantics at odds. C5② is what caught it.

**Ruling: the swap statement's column set = the set of mapped target columns; no unmapped column enters.**
Unmapped columns then take their own `DEFAULT` on INSERT and keep their existing value on UPDATE — both exactly what
"allow" promised. ADR-0038 §5's third branch from now on **governs both halves**: as the precheck judges, so the swap
is written; changing either later means looking at the other.

### 10. The second form of false green: an assertion that gets its result free from a schema error — print status and body verbatim in the report

Item 5 rejects "an assertion that is always true". C1② exposed another member of the same family:
**the assertion itself is correct, but the request never reached the path under test, and the failure came free from
something else.**

Specifically: `test-connection`'s draft fields are `#[serde(flatten)]`-ed in the protocol, the rig wrapped them in an
extra `{draft: …}` layer, and the request was rejected at parse time with `400 missing field name`; the assertion only
required "not 200", got its 400, and judged PASS. **What was tested was the rig's own malformed JSON, not the
rejection of bad credentials.**

**Two rulings:**

1. **Any check asserting that a request fails must also assert a reason for the failure** — C1② now additionally
   requires that the failure body **not match** `JSON 请求体无效|missing field`. After the fix the measured result is
   `502 / ERROR 1045 Access denied for user 'spike'`, which really did reach the credentials path.
2. **Print status and body verbatim in the report.** A false green is invisible from a PASS alone; only with the
   original text in the report does a human scan catch "that 400 looks wrong". Same reasoning as item 6's
   "no impersonating": **a report has to be open to challenge before it can hold back false greens.**

### Expiry

- **Item 1's "four baselines"**: only if sink one day shares one process across both levels (which requires
  overturning §3.5 first) does the baseline revert to "one per process". The trigger is **a change in the process
  model**, not four numbers feeling like too many.
- **Item 6's three boundaries**: if the rig ever grows the ability to drive a browser (it has not, and will not grow
  it for this alone), the UI halves can be merged back in; until then, **the report may not write the walkthrough's
  conclusions into the rig's conclusions.**
- **Items 8 and 9 are rulings on two product defects, and do not expire**: they are not "how it is arranged now" but
  two semantics that should always have agreed and were half-implemented. Unless ADR-0035 §4's range criterion or
  ADR-0038 §5's third branch is itself overturned — in which case these follow — loosening either half alone puts the
  diverged state back.

## Addendum (2026-08-19, while running overall v1 acceptance under #136): how to re-check a walkthrough trigger, the X series re-ruled to run against a live rig, and probes drifting with the UI

### Background

§6 planned v1's walkthroughs (V runs once and is sealed on #133, W lands on #132, X gets its own checklist), but what
it planned was **a plan**. Running the whole thing under #136 made three things visible that only a real run shows:
what makes a "not run" hold up, what the X series uses to produce its states, and whether a probe can itself lie.
Nailed down here. **Not one criterion changed.**

### 1. A "not run" holds up on **the diff since the seal point**, not on a declaration

`CLAUDE.md` rule 3 allows "sealed + zero changes since" as a legitimate "not run", but does not say how that
"zero changes" is proven. #136's method is **to give the seal point and the resulting diff for each, verbatim in the
report**:

- **V1–V25**: seal point `e581056` ("three factual corrections to the design-system README; the whole of V1–V25 run
  once and sealed"). `git log e581056..HEAD -- docs/design-system/ web/src/app.css web/src/` gives **zero commits** —
  neither trigger condition (a design-system change / a `tokens.css` change) holds.
- **W1–W6**: seal point `1348df1`. Two commits after it, `e63c492` and `aa510db`, **did touch the front end**
  (9 files, +1439 lines), so looking only at "did `web/src/` change" would misjudge this as triggered. Checked line by
  line: the strings `.precheck-reports` and `DiagnosticTable` appear **0 times** in the whole diff — the checklist
  names those two things' layout and column structure, not "did the front end change".

**Ruling: a walkthrough's trigger is judged against the object the checklist names, not extrapolated at file
granularity; and the judgement must carry "how it was checked", or the "not run" is only a declaration.**
This is cheaper than "the front end changed, so re-run everything" and stricter than "nothing changed, surely" —
what it saves is pointless full re-runs, what it tightens is that the claim can be challenged.

### 2. X1–X8 re-ruled to run **against a live rig**, with the stub demoted to a fallback

§6.3 did not specify a state source when it created the X checklist, and the implementation ticket used a stub
(`v1-mock.py`). Owner ruling of 2026-08-19 (Q5): **X1–X8 take their observations from the real service left running
by `run-v1-acceptance.sh`.**

The content of these criteria is the reason: X3 judges "wrong credentials cannot be saved", X4 judges "deletion is
refused and names the tasks" — against a stub, the failure body and the referencing task names are made up by the
walkthrough itself, which is **issuing yourself your own certificate**. Against a live rig, X3 gets a real `ORA-01017`
and X4 names the 11 tasks the C series really created.

**Cost and the top-up**: the rig creates only 2 datasources, while X2 judges "five rows recorded". The other 3 are
added via `POST /api/datasources` (the same entry point a human uses), **without requiring that they connect** — the
POST does not test the connection (ADR-0039 §3 puts "only a successful test allows saving" on the dialog), and X2 only
reads the list's columns and values. The seeding script `walkthrough/x-rig-seed.sh` lives in the same tree as the
probes and is committed with them.

**The stub stays**: with `X_RIG` unset to 1 it still runs against the stub. It is a fallback — when some item has no
subject on the live rig (the "no datasources at all" state, say), record honestly "this one fell back to the stub, and
here is the deviation"; **no forcing it, and never let one item block the whole run.**

### 3. A probe's selectors drift with the UI, and **a misreading probe and a broken UI look identical in the report**

`m3-probe.py` was written in the M3 era, and v1's builder changed three forms under its feet; this run hit four traps:

1. The target table moved from a plain input to `<input list>` + `<datalist>` — the old approach targets the
   **`value` attribute React does not reflect**, so it never selects anything.
2. `.column-fetch-section` occurs twice in v1 ("目标表建表 SQL" and the new "目标表列参考" share the class), so a bare
   selector is ambiguous.
3. The "拿建表 SQL" button ended up outside the viewport of the modal's scroll area; while `page.click()` scrolled to
   it, the `/api/builder/sql` call triggered by filling in the target table name re-rendered that section, so
   **the click missed** — not one `/api/columns` request was sent, and the awaited selector never appeared.
4. `.ddl-output` **also occurs twice** in v1 (the builder's "生成的 SQL" above shares the class). A bare selector hits
   that one, so W4 read "the DDL is only 9 lines and does not end in `utf8mb4;`" and W5 read "the DDL block is still
   there" — **both readings look like a UI regression and are in fact the probe's scope being wrong.**

Item 4 is the crux. **The report cannot tell "is this number a probe error or a broken product"**, so two practices:

- **Selectors always target label text or `aria-*`, never position and never attributes React does not reflect**;
  when one class appears more than once on a page, the scope must be narrowed to a unique ancestor
  (`.fetch-ready .ddl-output`, not `.ddl-output`).
- **When an observation does not line up, first compare item by item against the previous walkthrough record, then
  check the branch in the rendering code, and only then write "the UI changed".** That step is how #136 extracted the
  two false regressions.

These two are the two halves of one thing with §6's "walkthrough tooling must be committed" (which lands as
`CLAUDE.md` rule 4): committing solves "can the next machine run it", this section solves "can the numbers it
produces be trusted".

### 4. "Run in passing" is not "triggered" — the report keeps the two apart

W1–W6 really was run this time, but its trigger judgement is still "already met on #132" (see item 1's diff evidence).
The reason for running it is that the probe had to be fixed on this ticket anyway, and fixing it without running it
would be odd (owner ruling Q6).

**Ruling: in a walkthrough record, "trigger judgement" and "was it run this time" are two columns, and neither
substitutes for the other.** Run but not triggered: the observations go into the report as **supporting evidence**.
Triggered but not run: that is a debt, not a record. Conversely, V1–V25 not being run this time is not loosened by
"W happened to be run" — V's seal evidence is its own.

### Expiry

- **Item 1's re-check method**: valid as long as `CLAUDE.md` rule 3 still allows "sealed + zero changes" as a
  legitimate not-run. If it ever becomes "unconditional re-run on every acceptance", item 1 is void along with the rule.
- **Item 2's live-rig ruling**: bound to the fact that the C series does not tear down. If `run-v1-acceptance.sh` ever
  tears down on completion, the X series either carries its own rig orchestration or falls back to the stub —
  **and a fallback must be stated in the record**, never done quietly.
- **Item 3 does not expire**: it is not "how it is arranged now" but an inherent property of walkthrough tooling as code.
