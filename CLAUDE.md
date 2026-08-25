# db-qbs

## Agent skills

### Issue tracker

Issues live as GitHub issues in `liumingjian/db-qbs`, managed via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage roles, using their default label strings. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context — `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

## Visual gates

Three walkthroughs, one table. Rules 1-4 below apply to all three; they are not
per-milestone concessions.

| Walkthrough | Cases | Run it when |
|---|---|---|
| `docs/spikes/fixtures/local-rig/m2-visual-walkthrough.md` | V1-V25 | every M2 acceptance; **any** change to `docs/design-system/README.md` or `docs/design-system/tokens.css` |
| `docs/spikes/fixtures/local-rig/m3-visual-walkthrough.md` | W1-W6 | every M3 acceptance; any change to the **precheck block** in `web/src/app.css` — the contiguous run of `.precheck-reports` / `.precheck-exit` / `.diagnostic-table` rules at the end of the run-detail region — or to the `DiagnosticTable` column structure |
| `docs/spikes/fixtures/local-rig/v1-visual-walkthrough.md` | X1-X21 | every v1 acceptance; any change to the datasource screen, the target-agent screen, the nav item set, the task-creation screen — the builder in full, including its fetch-mode switch, custom-SQL editor, WHERE textbox, mapping columns and target dropdown — the start / bulk-start / rerun entries, the list filter strips, the client-side pagination, the datasource row-level connection test, the job-center column structure, the checkbox / bulk actions, the migration-progress cell, the run-status column, the run-detail drawer, the sider collapse, or any change to `web/src/app.css` outside the precheck block the W1-W6 row defines. A rule the precheck screen **shares** with other screens — the ligature list, for one — counts as outside, so it fires here |

1. **A trigger fires, you run it — no exemptions.** Whether an edit is "just text"
   costs more to adjudicate than running the walkthrough. Past rulings, each named by the
   screen or file it ruled on; the reasoning behind each sits in its ticket and in git history,
   which is why none of them is addressed by ADR number:
   - **M3's precheck screens** touched no design-system file, so V1-V25 did **not** fire.
   - **The v1 datasource screen** corrected two facts in `docs/design-system/README.md`,
     and that alone fired V1-V25 — taken on the chin, no exemption sought.
   - **Dropping the shape-precheck section** from the run-detail screen changed the
     `.precheck-reports` layout, so W1-W6 fired for v1.
   - **The P2 job center** re-tokenized `docs/design-system/tokens.css` and merged the
     run-history screen into the task list, so **all three** fired; the V-series
     re-judgments are V9 / V14 / V24 / V25, the X-series ones are X1 / X8-X12 plus new
     X13-X18.
   - **The target-agent screen** was added; V1-V25 fired **because the design-system
     README's screen inventory gained a line** — the judgements themselves were
     unchanged — and X1 / X2 were re-judged with new X19; W1-W6 did **not** fire.
2. **Record the actual observations, never a bare pass claim.** Each walkthrough
   file spells out its own record format; a report of "W2 passed" is not a report.
3. **If nothing triggered, say "not run" and why.** An acceptance whose changes touch
   no UI-affecting code (docs/ADR/fixture-only) may skip — but silence is not a skip.
   A walkthrough already run and sealed for this release is a legitimate "not run", but
   only with the seal cited **and** the evidence that nothing has changed since it
   (ADR-0040 §6.1 sealed V1-V25 on #133; §6.2 landed W1-W6 on #132).
4. **The tooling that drives a walkthrough lives in the repo.** Stubs, probes and
   runners belong under `docs/spikes/fixtures/local-rig/walkthrough/`, tracked. A hard
   gate whose tooling only exists in one machine's untracked directory is not a gate —
   it is a gate the next machine silently skips. Nothing new goes into `.playwright/`;
   that path keeps local browser config only.

## Language

**English is the default. Chinese survives only where a human reads it as prose.**

| Chinese | English |
|---|---|
| Strings rendered in the web UI (labels, buttons, error prose, the CJK glosses beside `SWAPPED` / `DISCARDED`) — the product serves Chinese users, and `docs/design-system/README.md` §2 is built on that | Everything else in the repo: `CONTEXT.md`, `docs/**`, ADRs, `README.md`, code comments and identifiers, commit messages |
| `docs/install/*.md` — an on-site operator types these steps by hand | Log `event` names, field names, and internal error strings |

Editing an existing file, **its established language wins: convert it wholesale or leave it, never
half.** A file half-converted is worse than either end — it reads as though someone gave up midway
and leaves the next agent guessing which half is current.

The test is always **who reads it**: a person, as prose → Chinese. An agent, a compiler, or whoever
opens the repo next → English.

## Living docs state the present

`CONTEXT.md` and `docs/design-system/README.md` are what an agent reads to know **what is
true now**. Stale text in either one does not just waste tokens — it gets built.

**Supersede by rewriting in place.** When a decision changes, edit the sentence it
falsified and delete what it replaced. Never append a superseded-by layer: an entry
that states the old form first, then two overturns, makes every reader diff three versions
to recover one fact, and half of them get it wrong. The sequence belongs in git and in the
decision ticket, not in the entry.

Both files say so in their own headers. Keep it that way.

## Record retention

A timestamped record (`*-<ISO8601>Z.md`: acceptance runs, visual walkthroughs, rehearsal
and build logs) is evidence of one run on one day, not a document. **Each series keeps its
newest record only**; delete the older ones in the same commit that adds the new one.
Cite `git log --diff-filter=D -- <path>` when a doc needs a retired record.

Two exceptions, both load-bearing: `docs/install/records/` (the `test-rehearsal-*.sh`
scripts assert those files exist) and the newest `packaging/centos7/records/build-*.md`.

An ADR that leans on a record **carries the verdict in its own text**, so retiring the
record costs nothing.

## Commit trailer

Agent-authored commits end with exactly this trailer — no other spelling:

```
Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
```

History before 2026-08-15 also contains `Claude Fable 5`; that variant is retired, old commits stay as they are.
