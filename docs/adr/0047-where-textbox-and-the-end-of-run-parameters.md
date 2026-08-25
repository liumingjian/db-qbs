# ADR-0047: Filtering becomes a free-form WHERE textbox, and the whole run-parameter chain is deleted

**Status**: Accepted
**Date**: 2026-08-25
**Origin**: [#180](https://github.com/liumingjian/db-qbs/issues/180), the first ticket under the
new-task wizard rework ([#168](https://github.com/liumingjian/db-qbs/issues/168)).
**Related**: [ADR-0045](0045-custom-sql-as-wrapped-subquery.md) §1 (custom SQL is wrapped verbatim as
a subquery — **the same "do not parse it, do not rewrite it" stance now applies to the WHERE text**),
[ADR-0045](0045-custom-sql-as-wrapped-subquery.md) §6 and [ADR-0046](0046-qa-round-editor-nav-and-dead-column.md) §1
(the highlighted input; **this ADR makes that widget shared rather than custom-SQL-only**),
[ADR-0043](0043-p2-job-center.md) §6 (bulk start; its "tasks with runtime parameters fail on the spot"
branch is **deleted** by §3 here), [ADR-0043](0043-p2-job-center.md) §4 (the detail drawer; its "run
parameters and identifiers" panel loses half its name), [ADR-0041](0041-v2-scope-trial-readiness.md)
addendum 2 (one-click re-run; **the entry stays, the prefill is deleted** — see §4).

## Background

Filtering was "pick a column + one of three comparators + a value", the comparators being `>` `<` `=`
— **not even `>=`**. Each condition also declared where its value came from: written into the task
definition (constant) or typed at launch (runtime). That second choice is the root of everything else
being deleted here: **run parameters existed only as a property of a structured condition.** They
brought with them a launch dialog, a set of prefill rules for re-run, a column in run history, a
"which parameters does this spec declare" derived surface, the second half of the concurrency
mutual-exclusion key, and a special branch in bulk start.

The four-slot form could not express what these users actually filter on: `>=`, `IN`, `BETWEEN`,
`LIKE`, `OR`, a sub-select, `TRUNC(...)`. The only way out was the custom-SQL path — where you have
to hand-write the whole query, table and all, just to add one predicate. **The people who use this
platform write SQL.** For them the form is not a simplification; it is a smaller expression language
wrapped in more clicks.

## Decision

### 1. `TaskSpec` carries `where_clause: Option<String>` and nothing else about filtering

`conditions` and `order_by` are gone. `where_clause` means exactly one thing: **a fragment spliced
verbatim after `WHERE`**, not including the word `WHERE` itself. Blank (or absent) means no `WHERE`
at all, i.e. read the whole table.

**It is not parsed, not rewritten, not reversed.** Whether it runs is Oracle's call, reported at the
moment it runs. This is the same stance ADR-0045 §1 took for custom SQL, and for the same reason:
the moment this code starts understanding the text, it owns an analyser that will drift away from
the database's own.

**"Verbatim" is literal — the generator does not even re-indent the continuation lines.** Lining a
multi-line fragment up under `WHERE` reads better, and it looked like a whitespace-only change; it is
not. Doing it correctly requires knowing which newlines fall **inside a string literal**, because
padding inserted into `'a\nb'` changes that literal's value, and therefore the rows that get moved.
Knowing that requires a lexer, and not owning one is this field's whole premise. Only the leading and
trailing whitespace of the whole fragment is trimmed.

**Anything that wraps the generated statement must therefore put its closing parenthesis on its own
line.** A hand-written fragment may end in a `--` line comment, which swallows the rest of *its
line*; `build_range_check_query` used to splice `... FROM ({sql}) RANGE_CHECK_SOURCE` on one line, so
the comment would have eaten the closing paren — and only at the moment the run reached the range
check. `precount` already wrapped with newlines.

`ORDER BY` goes with it and does not come back as its own field. It was never load-bearing — the
transfer is an upsert keyed on the primary key (ADR-0035), so row order changes nothing about the
result — and anyone who wants it can write it inside a custom SQL.

### 2. The only rule the WHERE text must pass is "no semicolon"

Not injection defence: the text **is** the user's own SQL, and whoever can type it here can already
run it against that database with the credentials this task holds. What the rule blocks is
**statement splicing** — everything after a `;` would be stitched into a statement that is supposed
to contain exactly one `SELECT`, so the query previewed and the query executed stop being the same
one. `validate_source_sql` has always held custom SQL to the same line.

Everything else is allowed through: unbalanced parentheses, unknown columns, unknown functions. Each
is decided by Oracle on the spot, and re-deciding them here means keeping a parser that drifts.

**Identifiers are still whitelisted.** Table and column names come from the interface's own
selections and should never contain a hand-typed character, so `validate_identifier` stays exactly as
it was. The WHERE fragment is the deliberate exception, and that asymmetry is the whole point: one
side is generated, the other is authored.

### 3. Starting a run is just running

`POST /api/runs` accepts the task identity and nothing else. Because `StartRunInput` is
`deny_unknown_fields`, an old client (or an old script) that still sends `run_params` is **rejected
outright** rather than silently ignored — being ignored is how someone ends up believing a parameter
took effect.

Clicking start runs the task; bulk start is "tick a few rows, click run". No dialog on either path.
This deletes ADR-0043 §6's branch that failed parameterised tasks locally with "needs run parameters,
start it individually", and with it that ADR's §Validity note about "a dialog at the top of bulk
start". There is nothing left to fill in.

**Where the errors go.** The launch dialog used to host two things besides the form: the "a run with
these parameters may already be in flight" warning, and the failure message. Both now land in a
banner at the top of the screen (`.notice.is-error`). The warning itself is deleted — it was a
client-side guess that raced the real gate; what remains is the server's own 409, reported when it
actually happens.

### 4. The mutual-exclusion key degrades to the task

It was "task + this run's parameter set". With no parameter set, it is the task: **one task may not
have two runs in flight.** The 409 message degrades in step, from "this task already has a run in
flight with the same parameters" to "this task already has a run in flight".

`RunHistory` drops `run_params` from the model and from every response. **The SQLite column is not
dropped and the table is not discarded**: nothing reads it any more, it was declared
`NOT NULL DEFAULT '{}'`, so inserts on an existing database keep working. Throwing away a machine's
whole run history to remove one column of dead data is wildly out of proportion — unlike the task
table, where the old shape genuinely cannot be read.

**Re-run keeps its entry and loses its prefill.** `rerunAction` — which rows offer a re-run, and why
a present-but-disabled button is better than a vanished one — is unchanged. `rerunPrefill` is
deleted: the previous run left no values to carry over, so re-run and start are now literally the
same call.

### 5. The task table is dropped wholesale on the old shape

`TaskSpec` is `deny_unknown_fields`, so a stored spec carrying `conditions` / `order_by` fails to
deserialize, which is already the third criterion in `drop_incompatible_task_table`. No migration
script, no in-place translation, no legacy coexistence — the same route the `columns` reshape took.
The harness gains one case that both drops such a row and then creates a new task, because "dropped"
has to mean **the machine still works afterwards**, not "`list()` now returns an error nobody can
clear from the interface".

### 6. The textbox is the highlighted input, made shared

The widget ADR-0046 §1 built for custom SQL — transparent-text `textarea` over a coloured `<pre>`,
driven by the in-house lexer in `web/src/sql.ts` — is extracted as `HighlightedSqlInput` and used by
both fields. Still no third-party editor, for the reason ADR-0046 §1 already gave. The pixel-lock
constraint moves with it: the merged CSS selector is now `.sql-highlight, .sql-text-input`, and
`.sql-text-input` takes over `.source-sql-editor textarea`'s place in the ligature list — `>=` must
not render as `≥` in the one field where the characters shown are the characters executed.

**No formatting button on the WHERE box.** `formatSql` lays out whole statements by clause; a bare
predicate has no clauses to lay out.

**The placeholder is a real, editable example**, not "enter a condition". The note underneath says
the two things a first-time user gets wrong: do not write the word `WHERE`, do not write a semicolon.
The column list of the selected table sits beside it as a reference — **no completion**: completion
takes over the keyboard, and this field is too short to be worth it.

### 7. The frontend `spec` module keeps its role and loses vocabulary

`web/src/spec.ts` is still the single presentation vocabulary shared by the builder, the start
surface, and run history. It simply has less to say: every comparator / value-type / parameter-name /
run-parameter export is gone, and `conditionSummary` becomes `whereSummary` — whitespace collapsed to
one line, **not one character rewritten**, `整表` when nothing was written. Keeping the module while
shrinking it is deliberate: the three screens still need one answer to "how do we say this", and the
day something new is added, there must be one place for it rather than three.

## Consequences

- **A malformed filter now fails at run time, not at save time.** The four-slot form could not
  produce a syntactically invalid predicate; a textbox can. The failure is an ordinary Oracle error
  on the first statement — which is also where "column does not exist" and "wrong type" have always
  surfaced — but it does move the discovery point later. This is the price of not owning a parser,
  and it is paid knowingly.
- **Bind variables disappear from the read path entirely.** Values used to go through named binds for
  escaping correctness (ADR-0011 §2, "do not invent a second escaping scheme"); with the values now
  written by hand inside the fragment, there are no values left for the generator to escape. The
  binding plumbing is deleted from `oracle_source.rs` rather than left carrying empty maps. Escaping
  is now the author's problem inside their own text — which is exactly the deal custom SQL already
  had.
- **"Import yesterday's rows each day" is still out of reach, but for a smaller reason.** It used to
  need a caller passing a value; now it needs a relative-time expression, and the user can simply
  write `TRUNC(SYSDATE) - 1`. ADR-0004's ban on relative time was enforced structurally by "values
  are bind variables"; that structure is gone, so the ban is gone with it. This is a real widening
  and is recorded as such — a run is triggered by hand, so "when did it run" is already the operator's
  own decision.
- **Run history rows written before this change keep a `run_params` column nobody reads.** Stated so
  the next person does not mistake it for a live field.

## Validity

**Reopening signals**:

1. Someone needs the same task run against different filters on a schedule or from a caller — at
   which point parameterisation returns, but as a **template + arguments** design applied to the text,
   not as the old per-condition value-source flag.
2. Malformed filters turn out to be a real, repeated field failure — the answer is a "test this
   filter" button that runs a cheap `COUNT(*)` against the real database, **not** a client-side
   parser.
3. Two runs of one task in flight becomes something the field actually wants (different filters, same
   definition) — the mutual-exclusion key needs recomputing, and §4 is the sentence to overturn.

## Walkthrough triggers

Per the table in `CLAUDE.md`:

- **X series (v1 walkthrough) fires** — the task-creation screen's builder changed, the start
  dialog is gone, the re-run entry changed, and `web/src/app.css` changed outside the precheck block.
  **X21 added** (the WHERE textbox: card, placeholder, highlighting, pixel-lock, and the absence of
  every old control). **X9 re-judged** (re-run opens no dialog and lands on the run detail; the
  concurrency warning moves from inside the dialog to the top-of-screen 409 banner). Nothing is
  renumbered and no line is deleted.
- **V series (design system) fires and was run** — both design-system files are edited, and per
  `CLAUDE.md` rule 1 that alone is the trigger, with no exemption sought (the precedent is the v1
  datasource screen, which fired V1–V25 by correcting two facts in the README). What changed:
  README §7's component entry becomes **"highlighted SQL input" `.sql-text-input` + `.sql-highlight`**,
  because §6 above makes the widget shared rather than custom-SQL-only, and the drawer entry drops
  "run parameters" from its contents list; `tokens.css` keeps all four highlight values **unchanged**
  and only widens the comment's "appears only in the custom SQL input" to name both places.
  Re-judgements: **V14 / V15 re-anchor** (the drawer panel is renamed from "run parameters and
  identifiers" to "run identifiers"; the two-ids criterion is untouched) and **V16 is re-judged** —
  its subject, the warning inside the launch dialog, no longer exists; the same thing is now the
  server's own 409 in the top-of-screen banner, judged on the same terms (one plain sentence, says
  which task, dismissable), with the old "only warns you, does not stop you" half **reversed**.
- **W series (M3) does not fire** — neither `.precheck-reports` / `.precheck-exit` /
  `.diagnostic-table` nor the `DiagnosticTable` column structure changed by one character.

**The tooling for all three was updated regardless of what fired**, because the product shape it
drives moved: the stubs' specs now carry `where_clause`, `v-mock.py`'s state factory switches from
"the date typed into the dialog" to "which row you click", and `m3-probe.py` stops filling a form
that no longer exists. Per `CLAUDE.md` rule 4, a gate whose tooling no longer drives is a gate the
next machine silently skips, so all three were run to `exit=0`. **X and V are judgements**, recorded
in `v1-visual-walkthrough-20260825T054145Z.md` and `m2-visual-walkthrough-20260825T053720Z.md`;
the W run is a smoke test that proves its tooling drives, and nothing more.

Running V also turned up **a defect in the stub, not the product**: `v-mock.py`'s live-run state was
missing `total_rows` / `precount_ms` entirely, so the "total rows" cell rendered `NaN` — the product
branches on `=== null`, and an absent field is `undefined`. Fixed in the stub, because a gate whose
fixtures do not match the interface's own contract judges the wrong screen.
