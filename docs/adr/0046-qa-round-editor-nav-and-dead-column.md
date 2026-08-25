# ADR-0046: One QA round — syntax highlighting and formatting for custom SQL, nav reordered by revisit frequency, and the always-true "password" column dropped from the datasource screen

**Status**: Accepted
**Date**: 2026-08-24
**Origin**: a round of **manual QA** by the owner after [ADR-0045](0045-custom-sql-as-wrapped-subquery.md)
landed (stub backend `v1-mock.py` plus `web/dist`), raising three points: custom SQL needs
highlighting and formatting; Target Agent should not be first in the nav; the "password" column on
the datasource screen carries no meaning.
All three touch criteria an ADR had already pinned (the ordering in [ADR-0044](0044-target-agent-registry.md) §6,
the column structure in the X2 walkthrough criterion, the SQL input box in ADR-0045 §6), so per
`ADR-0034` §1 they are recorded together here.
**Related**: [ADR-0045](0045-custom-sql-as-wrapped-subquery.md) (its §6 input box is amended by §1
here; **§1–§5 are untouched**), [ADR-0044](0044-target-agent-registry.md) §6 (**its ordering is
rewritten by §2 here; every other clause in that section stands verbatim**),
[ADR-0043](0043-p2-job-center.md) §2 (the nav item set),
[ADR-0039](0039-v1-ui-increments.md) §3 (store only what connects — the entire basis of §3 here),
`ADR-0037` §5 (a password is never read back — **untouched**),
`ADR-0025` (the single source of design-system tokens; §1 adds four colours to it)

## Background

The three points come from one QA session and are technically unrelated, but they share a shape:
**none of them is "the implementation is wrong"; each is "the original judgement stopped holding once
it was actually used".**
So none can be slipped in as a quiet bugfix — each must state which sentence it overturns and why that
sentence looked right at the time.

## Decision

### 1. Syntax highlighting and one-click formatting for the custom SQL input

ADR-0045 §6 gave a bare `textarea`. What actually gets pasted in the field is "an existing 70-column
query with a dblink" — precisely the cost ADR-0036 §1 wrote down and the thing ADR-0045 bought back.
In a bare `textarea` that is one undifferentiated block: you cannot see where `FROM` is or which quote
is unclosed. **This field is the only place the query is visible in that mode** (ADR-0045 §6 says so
itself), so it has to be legible.

**One lexer, two uses.** `web/src/sql.ts` exports only `tokenize` and `formatSql`; highlighting and
formatting share a single scan. Two independent character walks would eventually drift apart on string
literals, line/block comments, and double-quoted identifiers — exactly the three places where
misreading one character changes the meaning.

**No third-party editor** (CodeMirror, Monaco). This field needs "see the structure", not completion,
folding, or multiple cursors; switching would carry hundreds of extra kilobytes and require reconciling
its styling system against this repo's tokens. Highlighting takes the familiar route of a
**transparent-text `textarea` over a coloured `<pre>`**, with the two layers' shared-box constraints
pinned in one merged selector.

**Formatting touches whitespace only and changes not one character.** The invariant:
`tokenize(formatSql(s))` with whitespace tokens removed is **character-for-character equal** to
`tokenize(s)` with whitespace tokens removed, guarded by `sql.test.ts`.

That rules out "uppercase the keywords", which looks like amputating a formatter's basic job, but:

- On Oracle, changing case is fatal for **quoted identifiers**, and deciding "which words are ordinary
  words outside quotes" needs an analyser this does not have.
- More importantly, once characters may change, **"the formatter did not break my SQL" drops from
  provable to manually verifiable.** A button that quietly rewrites the user's SQL is self-contradictory
  in a system whose SQL is wrapped verbatim as a subquery (ADR-0045 §1).

By the same reasoning, **anything inside parentheses is collapsed onto one line**: with no analyser
there is no way to tell a subquery from `NVL(a, b)`, and guessing wrong produces a mess. Between
"pretty layout" and "predictable layout", take the latter.

**Formatting does not clear already-fetched result columns.** Editing the SQL does (the columns may no
longer be the same set); formatting does not — since the invariant guarantees the meaning is unchanged,
the columns are the same set, and clearing them would punish someone for tidying their layout. This is
the entire reason `SqlEditor` has both `onChange` and `onFormat` rather than one callback.

**The highlight colours form their own small group and do not reuse the four semantic colours.**
`--crit` / `--warn` / `--ok` each have a job in the three-axis language (ADR-0025 §4); tinting a string
literal with `--crit`'s red would conjure the sentence "something is wrong here" out of nowhere.
Four new tokens — `--sql-keyword` / `--sql-string` / `--sql-number` / `--sql-quoted` — appear only in
this field and never in a state, tag, or chart. Comments and punctuation deliberately fall back to the
existing `--mute` / `--dim`: they want to recede, not to be yet another colour.

**Double-quoted identifiers and single-quoted literals must differ in colour**: on Oracle the two are
worlds apart, yet they look nearly identical in a bare `textarea`. This is the most concrete thing
highlighting buys in this system.

**Ligatures must still be off, and off in both layers.** The constraint inherited from ADR-0045 holds
verbatim (`>=` must never render as `≥`); disabling only one layer would misalign not just glyphs but
the cursor position.

### 2. Nav ordered by revisit frequency: Job Center · Datasources · Target Agent · Settings

**This rewrites the ordering judgement of ADR-0044 §6.** Every other clause in that section — the agent
screen has a status column, the registration dialog has no test button, the datasource screen gains a
column, the form requires an agent and folds it into the connection fingerprint — **stands verbatim**.

ADR-0044 §6 put agents first, reasoning that "a MySQL datasource cannot be created without a registered
agent, so this screen is the first stop when setting up a new machine". That sentence is not wrong, but
**it holds only on the day of first installation** — afterwards the agent screen is an ops screen you
return to when something breaks. Using a one-time dependency chain to fix an order someone clicks daily
mistakes the installation manual's sequence for the everyday path.

There was also an inconsistency from the start: **the landing page has always been the job center**
(the fallback in `pageFromHash`), while the first nav item was agents — so on opening the app the
highlighted item and the expanded screen disagreed. The new order removes that mismatch too.

**The criterion becomes one reusable sentence**: the nav is ordered by **revisit frequency**,
descending, not by dependency chain.
The position of `agents` in the `Page` union type moves accordingly — the order in which a type is
written does not affect behaviour, but it is the first statement of ordering the next person reads, so
it should match the interface.

### 3. Drop the "password" column from the datasource screen

**That column is always 「已设置」.** ADR-0039 §3 established "store only what connects": a stored
datasource necessarily has a password that was set and did connect. So the column has only ever had one
value — **a column of constants occupying a cell's width while answering nothing.** It was written
because `has_password` happened to be in the API (ADR-0037 §5 lets the interface see only that boolean),
and **"there is a field" was mistaken for "there is something to say".**

**The badge in the form stays, untouched.** The `已设置 · 留空 = 不改` beside the password box when
editing a datasource (`.field-badge.is-neutral`) answers an entirely different question:
**"what happens if I leave this blank this time?"** That is a sentence about the consequence of this
action, not about a property of the record.

**The API does not change.** `has_password` remains in `DatasourceView`, is still computed by
`datasource.rs`, and the Rust-side assertion (`"has_password":true`) stays as it is. What is withdrawn
is one column of display, not a piece of data — the form still needs it, and changing the API to delete
one column of display would turn a reversible act into an irreversible one.

**ADR-0037 §5, "a password is never read back, not even the ciphertext", is untouched.** This ADR does
not touch the credential boundary at all.

## Consequences

- **The format button is a control that rewrites user input**, the first in this repository. The
  invariant in §1 is the sole condition under which it is allowed to exist; the day someone wants to add
  "uppercase keywords" or "unify the indentation style", they come back and overturn that invariant
  rather than adding a switch in the implementation.
- **The lexer is hand-written and will certainly misread some inputs.** The cost is fenced into a very
  small area: a misread affects **colouring only**, and colouring does not affect execution — what
  actually crosses the wire is the wrapped statement in `crates/source` (ADR-0045 §1). On the formatting
  side the invariant holds the line, so a misread costs at worst an ugly layout, never a changed meaning.
- **Nav ordering now has a criterion instead of a case-by-case argument**: revisit frequency. Where the
  next new screen goes is answered by that sentence.
- **`has_password` becomes a field used only by the form.** It now has exactly one consumer; if that half
  of the form ever changes too, the API side should be retired with it — recorded here so it does not
  become an unclaimed field.
- **There is now a token group that appears in exactly one place** (the four SQL highlight colours). It is
  the design system's first set of colours that **carry no state semantics**, so README §7 gains one
  component entry. It risks misuse (someone tinting something else with `--sql-keyword`'s purple), so the
  token comments pin down "appears only in the custom SQL input".

## Validity

**Reopening signals**:

1. Something appears that "requires understanding the SQL's structure" — inferring columns from SQL, real
   syntax validation, pointing at an error's position. A hand-written lexer will not do; that needs an
   analyser, and §1's "no third-party" conclusion must be recomputed.
   Note this signal **excludes** "parse SQL back into a structured spec", which ADR-0023 §2 rejected
   permanently and ADR-0045 §2 restated; it is unrelated to this ADR.
2. The nav exceeds six or seven items, or grouping / second-level navigation is needed — at which point
   "ordered by revisit frequency" is no longer sufficient.
3. A datasource shape appears that can be stored without a connection test (importing a configuration
   offline, say) — §3's premise disappears and the "password" column may need to return.

## Walkthrough triggers

All three changed the interface; per the table in `CLAUDE.md`:

- **X series (v1 walkthrough) fires**: **X1 re-judged** (nav order becomes Job Center · Datasources ·
  Target Agent · Settings; the item set, the dark sider, and the collapse criteria are unchanged),
  **X2 re-judged** (the datasource screen drops the "password" column, so the header goes from eight
  columns back to seven; the rest of that criterion — no search box, no connection status column, a
  "referenced by" column, connection strings shown per kind, plus the "Target Agent" column — is
  untouched), **X20 added** (the custom SQL field gains a highlight layer and a format button).
  The numbering rule holds as always: nothing is renumbered, no line is deleted.
  **X19's criterion is unchanged**, but **the path to reach it changed** (the agent screen is now the
  third nav item).
- **V series (design system) fires**: `docs/design-system/tokens.css` gained four tokens and README §7's
  component inventory gained the "custom SQL input" entry. Per `CLAUDE.md` rule 1, **changing them means
  running V1–V25, with no exemption.** The only new criterion is that §7 component; the rest is regression.
- **W series (M3)**: neither `.precheck-reports` nor `DiagnosticTable` changed by one character, so it
  **does not fire**.
