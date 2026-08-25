# ADR-0045: Custom SQL returns as v1's second data-retrieval path — the SQL is a wrapped subquery, and the structured spec remains the source of truth

**Status**: Accepted
**Date**: 2026-08-24
**Origin**: **recorded after the fact.** On this branch, `codex/p1-handoff-screenshots`,
[`71e09b0`](../../commit/71e09b0) already landed custom SQL in the code (`TaskSpec::source_sql`,
`/api/builder/sql-columns`, a second tab in the builder), and `eae232e` deepened it (the outer
projection, column selection, prose guidance). A two-axis code review on 2026-08-24 found that
**it conflicts head-on with `ADR-0036` §1 and was never recorded.**
Per `ADR-0034` §1 — a change to a capability boundary must land as an explicit ADR and may not take
effect quietly inside an implementation ticket — this ADR is that entry.
**The bookkeeping is late, and that fact is written here rather than smoothed over.**
**Related**: `ADR-0036` (**its §1 clause "v1 offers no editing entry point" is revoked by §1 here;
its §3 is voided by §5; the disposition of §2, §4, §5, §6 and §7 is in the table in §5**; **its
validity signal 1 is this ADR**), `ADR-0023` (**the reasoning by which its §2 rejected option C is
unchanged**, see §2 item 2), `ADR-0035` §3 (condition shapes and run-time parameters — **this ADR does
not reopen it**, it only declares that on the custom-SQL path it has no subject),
`ADR-0038` (§1 the projection alias *is* the mapping, §2 column mappings store the target name, §8
column names are case-insensitive — **all three untouched**; §3 here is how they land on custom SQL),
`ADR-0011` §2 (do not invent a second escaping scheme, see §4),
`ADR-0009` (the mapping precheck is **untouched**),
`ADR-0034` §1 (capability changes are recorded explicitly)

## Background

`ADR-0036` §1 ruled that the structured spec is the single source of truth and that v1 does not
support hand-edited SQL, and — following ADR-0034 §1 — **wrote the cost down explicitly** in its body:

> **Cost, stated plainly**: that existing production query with 70-plus columns and a dblink
> **cannot simply be pasted in and run** in v1; it must be rebuilt in the builder. Queries the builder
> cannot express (multi-table joins, subqueries, expression columns) have **no way out** in v1.

The same ADR's validity signal 1 named the condition for reopening:

> **Hand-edited SQL — explicitly not in v1.** Re-evaluate when a real query appears that the builder
> cannot express but the business must move.

**That signal fired** — the code has landed, and the builder now has two data-retrieval paths. The
problem is not whether it should have been done, but that it **bypassed the bookkeeping**: ADR-0036 §1's
sentence "SQL is displayed read-only in the interface; v1 offers no editing entry point" still hangs in
the repository unchanged, `CONTEXT.md` restates it in two places, and the product's behaviour is now the
opposite. **A ruling that tells lies is worse than no ruling**: the next person to read it will make
judgements from it.

**The shape is what matters.** If what landed were exactly the shape ADR-0036 §1 rejected word for word,
the correct response would be **to remove the code**, not to file an ADR ratifying it. So this ADR's
first job is to put the shape on the table for comparison — and it is genuinely not the same one.

## Decision

### 1. Adopt shape D: the SQL is an **input**, not an authority

Reusing the taxonomy of `ADR-0023` §2 and `ADR-0036` §1, with one row added:

| | Shape | Verdict |
|---|---|---|
| A | One-shot scaffolding: a wizard emits SQL text, the selection state is not kept, the text is authoritative | **Retired** (ADR-0023 §2's original ruling; not reopened here) |
| B | The structured spec is authoritative: the task definition stores table / columns / conditions / ordering / primary key, and the SQL is generated from it | **Adopted** (ADR-0036 §1, **still in force**, the default path) |
| C | Two-way sync: hand edits are parsed back into the structured model | **No** (it needs an Oracle SQL parser; ADR-0023 §2's reasoning is unchanged) |
| **D** | **The spec remains authoritative, and the user's SELECT is embedded as an opaque subquery**: the outer projection is generated from the spec | **Adopted** (this ADR, the second path) |

The generated shape:

```
SELECT q.<source column> AS <target field>,
       q.<source column> AS <target field>
  FROM (
         <the user's SELECT, verbatim>
       ) q
```

**The user's SQL is never executed verbatim.** The transfer chain recognises only result column names
(`transfer.rs` hands `source.columns()` of the executed statement straight to sink, which is why "the
result column name *is* the target column name"). Without this outer projection, column selection has
nowhere to land — unselected columns would cross the wire anyway, and renaming a target field would be
silently ignored.

**Why this is not the middle road ADR-0036 §1 rejected.** What that rejected was "store a piece of SQL
text but mark it read-only", and its reasoning ran:

> It keeps the shape in which an unreproducible piece of SQL lies inside the task definition, so the
> parameter list still cannot be derived and the condition shapes are still unknowable — **it keeps
> every cost of hand editing while granting no freedom at all.**

D changes both ends of that sentence. **The freedom is granted in full**: any SELECT — multi-table joins,
subqueries, expression columns, dblinks — all of which are exactly what ADR-0036 §1 recorded as having
"no way out". **The costs are then dissolved structurally by the three constraints in §2**, not by
parsing — which is the boundary between D and C, and the whole reason D can stand.

### 2. Three constraints, dissolving one by one the three reasons ADR-0036 §1 adopted B

**Constraint 1: a custom-SQL task has no filter conditions and no ordering, and therefore no run
parameters.** `TaskSpec::validate` hard-rejects them (`crates/source/src/task_spec.rs`):

> `自定义 SQL 模式不能再配置过滤条件或排序，请直接写入 SQL`

ADR-0036 §1's first reason was that ADR-0035 §3's "fill at run time" requires enumerating parameters and
collecting values at submission, which only a declared parameter list can do. On D it **does not apply**,
because there are no parameters to enumerate on this path — this is not "we cannot derive them so we
fudge it", it is **declaring that they do not exist**, enforced by validation.

**Cost, stated plainly**: a custom-SQL task **cannot carry run-time parameters**, so "import yesterday's
rows each day" is out of reach. Changing the condition means changing the SQL text itself, i.e. changing
the task definition. This is a real capability gap; see Validity 1.

**Constraint 2: never parse back, never sync two ways.**
The spec's authority over **the projection, column mapping, target fields, and the primary key** is
**always and only** one copy; `source_sql` is an opaque piece of text, and the system never reads its
contents to infer anything. ADR-0036 §1's second reason — "once hand editing is allowed, structure and
text will inevitably drift, and 'which is authoritative' must be re-asked at every decision point" —
does not hold on D: **there is no second authority**, so there is no surface to drift on.
`ADR-0023` §2's reasoning for rejecting option C is **unchanged**.

**Constraint 3: every result column of the inner query must have an unquoted identifier name.**
Expression columns must supply their own alias (`COUNT(*) AS CNT`). This is not a new rule but the
existing surface of `validate_identifier` — the **only** thing standing between an identifier and being
concatenated into SQL (values go through bind variables; identifiers cannot). An expression column
without an alias cannot land in the spec at the read-result-columns step, so it **cannot be stored**, and
never becomes a problem that explodes at run time.

### 3. Quoting rules for inner aliases: quote only when necessary

Oracle folds **unquoted** references to upper case. So when the inner query writes
`SELECT id AS "id"`, the described result column name comes back as lower-case `id`, while the generated
outer `q.id` folds to `Q.ID` — **a miss, ORA-00904, exploding only at run time.**

Ruling: **when the column name stored in the spec is not all upper case, quote the outer reference; when
it is all upper case, leave it unquoted.**

- All upper case is the overwhelming majority (Oracle's default folding), and leaving it unquoted means
  **the SQL text generated by existing tasks does not change by one character.**
- "Always quote" is rejected: it would alter the generated text of every existing task (and with it the
  comparability of the snapshot in run history) for exactly the same correctness as this rule —
  **zero benefit, maximum blast radius.**

The target-field side is unchanged: `AS <target field>` folds to upper case, and sink compares
case-insensitively per `ADR-0038` §8, so it hits regardless.

### 4. The shape-validation surface: three rules, all structural

**None** of the six rules cancelled by `ADR-0036` §5 is restored. Custom SQL passes only the three rules
of `validate_source_sql`:

| # | Rule | Nature |
|---|---|---|
| 1 | Non-empty | Structural |
| 2 | Exactly one statement (after stripping one trailing `;`, no `;` may remain) | Structural |
| 3 | The first keyword must be `SELECT` | Structural |

**What it explicitly does not do**: parse SQL, judge precision, judge expression columns, judge magnitude,
or judge whether it will run at all. The debt from ADR-0036 §5 item 6 (columns of indeterminate precision
are no longer intercepted) **stands as it was**; this ADR does not repay it.

**Its relation to `ADR-0011` §2, "do not invent a second escaping scheme"**: no conflict. The system
**concatenates nothing into the user's SQL text** — the outer projection concatenates identifiers only,
each passed through `validate_identifier`. Literal values inside the user's SQL were written by the user,
who is responsible for their own statement; the system never touches them, so no "second escaping scheme"
arises.

**Note that judgement has not disappeared**: `ADR-0009`'s **mapping precheck** runs on the sink side via
cursor describe, and types outside the whitelist, `DECIMAL`s with insufficient precision, and `NUMBER`s
without a declared precision are still **hard-rejected** by it. A custom SQL's result columns travel the
same describe path and are therefore governed by the same gate, with nothing loosened.

### 5. Disposition of `ADR-0036`, clause by clause

| ADR-0036 | Original ruling | Disposition here |
|---|---|---|
| §1, first half | The structured spec is the single source of truth | **Untouched**; D still obeys it |
| §1 "SQL is read-only in the interface; v1 offers no editing entry point" | No editing entry point | **Revoked.** A second retrieval path exists and it has an editing entry point |
| §1's "cost, stated plainly" paragraph | Joins / subqueries / expression columns have no way out in v1 | **The cost is gone**, which is precisely what this ADR delivers |
| §2 | SQL does not enter the task definition, it is recomputed; history pins a snapshot | **Still holds, once distinguished**; see below |
| §3 | Reserve no structural slot for "restoring hand editing later" | **Voided**; see below |
| §4 | Discard old task definitions and swap the schema | **Untouched**, already carried out |
| §5 | The SQL shape precheck is cancelled entirely | **Untouched**; the three rules restored in §4 here are structural validation, not any of those six |
| §6 | There are exactly three derived surfaces | **Untouched.** `source_sql` is an **input field** of the spec, not a fourth derivative |
| §7 | The mutual-exclusion key is `task_id` + the run-parameter set | **Untouched.** A custom-SQL task has no parameters and degrades, per that text, to "the same task may not run concurrently with itself" |
| Validity 1 | Re-evaluate when a real query appears that the builder cannot express | **This ADR is its fulfilment** |

**Why §2 still holds.** It ruled that the **derived** SQL does not enter the task definition — that copy
is still not stored, still recomputed, and every run record still pins a snapshot of what was executed.
The `source_sql` that does enter the task definition is **a different thing**: an input to the spec,
peer to `owner` / `table` / `columns`, answering "where does this task get its data from" rather than
"what will be generated now". The distinction between the two kinds of SQL (that paragraph of §2) is not
muddied at all; there are simply now three: **the input subquery** (written by the user), **the derived
statement** (recomputed), and **the history snapshot** (what actually ran).

**§2's open question is answered here.** Its validity signal 1 asked: "after hand editing, what is the
relation between the SQL snapshot in history and the spec?" Answer: **the snapshot stores the complete
wrapped statement**, exactly isomorphic to the table mode — it does not change when the spec changes; it
is an audit fact. Custom SQL introduces no new shape into that relation.

**Why §3 is voided.** What it forbade is precisely the field `source_sql: Option<String>`. Its argument:

> A field with only one possible value **neither prevents** the data shape from having to change again
> when hand editing is genuinely built, **nor stops** a reader of the code from believing two task shapes
> exist in the system.

That field now has **two real values**, and two task shapes **do** exist in the system — the premise is
gone and the argument falls with it. This is a premise disappearing, **not** an argument being refuted;
if custom SQL is ever withdrawn again, §3 automatically comes back into force.

### 6. Interface: two paths, one fork

- **The fork lives on the header of the card it governs** (two tabs, source table / custom SQL), not in
  the footer of the datasource card.
- **Switching clears** the source table, result columns, field mappings, primary key, and filter
  conditions — with a confirmation shown **only when there is genuinely something to lose**.
- **The filter-condition card does not disappear in custom-SQL mode**; a sentence explains where filtering
  has gone. Removing the whole card would suggest filtering was gone, when in fact it has just moved.
- **The "build SQL" preview renders in both modes.** This one is hard: the user's SQL **is not executed
  verbatim**, and the preview is the **only** place the final statement can be checked. Not rendering it
  in SQL mode would ask someone to judge a piece of text that will never be executed.
- **The interface promises nothing about ordering.** An inner `ORDER BY` **does not bind** the outer query
  — Oracle does not guarantee an inline view's ordering propagates outward. The transfer semantics do not
  depend on ordering anyway (upsert deduplicates on the primary key and is idempotent), so **this is not
  fixed** — but no promise such as "put the ordering in your SQL" may appear in the prose.

## Consequences

- **Three places in `CONTEXT.md` must change**:
  1. In the glossary under "task definition": "**v1 offers no entry point for hand-editing SQL** (§1, an
     explicit narrowing of capability: an existing production query cannot be pasted in)" — contrary to
     fact; rewrite.
  2. Under "V1 scope": "**the spec is the single source of truth, the SQL is read-only and cannot be hand
     edited**, see ADR-0036" — the first half still holds, the second must change.
  3. **Add a "custom SQL" glossary entry.** It is already a first-class concept (the badge in the job
     center's source column, the discriminant of `SourceSummary.kind`), and its absence from the glossary
     is a gap.
- **`ADR-0036`'s title stays as it is** (including the "v1 does not support hand-edited SQL" half).
  Changing the title would break every anchor referencing it, and the disposition is already carried
  clause by clause by the table in §5 here; anyone reading ADR-0036 should read its "Related" line first.
- **The interface prose cites the wrong ADR** where it says "the comparison operators are only `>` `<` `=`
  (ADR-0035 §3)". `ADR-0035` §3 actually says "no `IN` / `BETWEEN` / `LIKE` / expressions" — it excludes
  **shapes**, and `>=` **is not on that list**. This ADR adds no comparison operator (that is a separate
  product decision), but **ADR-0035 §3 may no longer be used as cover**; the prose is changed to state the
  present rather than cite a ruling.
- **Columns of indeterminate precision are still not intercepted** (the debt of ADR-0036 §5 item 6).
  Custom SQL makes such columns more likely (an expression column's precision is especially unknowable),
  so **the debt grows heavier**, but its disposition is unchanged; see Validity 2.
- **Where it lands**: `crates/source/src/task_spec.rs` (`source_sql()` / `projection()` / `validate()` /
  `validate_source_sql()`), `crates/source/src/server_main.rs` (`/api/builder/sql-columns`),
  `web/src/App.tsx` (the two-path builder), `web/src/spec.ts` (`sourceSummary()`).

## Validity

1. **Run parameters — a custom-SQL task has none.** Re-evaluate when a real requirement appears that "this
   existing SQL must run daily with a parameter".
   **The answer is most likely not to go back and write a parser**, but to let the user write named
   placeholders in the SQL and **enumerate the parameter list themselves** (a user declaration, the same
   in kind as `value_type` — the same logic as ADR-0036 §6's "described types may not enter the task
   definition"). ADR-0023 §2's reasoning against C would still hold, unaffected.
2. **Precision determinacy** — the debt of ADR-0036 §5 stands as it was and is made heavier by this ADR.
   The answer is still to move it into the mapping precheck (that end already holds the described
   precision information), not to bring the shape precheck back.
3. **Parse-back / two-way sync (shape C) — permanently rejected**, unless ADR-0023 §2's argument is itself
   overturned. Adopting D here is **not** a step toward C: the boundary between D and C is precisely
   "does it read that text", and D never reads it.

## Walkthrough triggers

This ADR is a retrospective record and changes no code itself; but **the same batch of commits did change
code** (the source-table rendering in the run detail drawer, the builder's save-disable condition, probe
coverage of custom-SQL mode, and the outer projection's quoting rule). Per the table in `CLAUDE.md`:

- **X series (v1 walkthrough) fires**: the builder's mapping and save conditions changed, as did the
  reading of the job center's source-table column (`RunDrawer` filled in).
  **X18 re-judged** — the "task definition" panel in the run detail drawer previously rendered a bare dot
  for custom-SQL tasks, and the criterion must state its present shape. **X20 added**: "custom SQL mode:
  the tab-switch confirmation, the filter-condition card's explanation, the build-SQL preview rendering,
  result column selection taking effect" — **this state had never been reached by the probe before**, so
  the sentence in the previous record (`20260824T113748Z`) claiming "this card no longer disappears
  entirely in SQL mode" was **unsupported by observation**, and per `CLAUDE.md` rule 2 that is owned here.
  The numbering rule holds as always: nothing is renumbered, no line is deleted.
- **V series (design system)**: neither `docs/design-system/README.md` nor `tokens.css` changed, so it
  **does not fire**.
- **W series (M3)**: neither `.precheck-reports` nor `DiagnosticTable` changed, so it **does not fire**.
