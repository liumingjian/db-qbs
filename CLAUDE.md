# db-qbs

## Agent skills

### Issue tracker

Issues live as GitHub issues in `liumingjian/db-qbs`, managed via the `gh` CLI.

### Triage labels

Five canonical triage roles, each using its default label string: `needs-triage` (maintainer
needs to evaluate), `needs-info` (waiting on the reporter), `ready-for-agent` (fully specified,
ready for an AFK agent), `ready-for-human` (requires human implementation), `wontfix`.

### Domain docs

Single-context — `CONTEXT.md` at the repo root is the one domain document. There is no
`docs/adr/`: the decision record lives in GitHub issues and in git history.

## Visual gates

The three walkthrough checklists (V1-V25, W1-W6, X1-X21) were **deleted** along with the rest
of `docs/`. What survives is the tooling that drove them, under
`docs/spikes/fixtures/local-rig/walkthrough/` — the runners, stubs and probes still run, but
the case lists they were judged against no longer exist in this repo. **The visual gates are
therefore not enforceable from the repo as it stands.** Re-establishing one means re-authoring
its case list first; retrieve a deleted checklist with
`git log --diff-filter=D -- docs/spikes/fixtures/local-rig/`.

Three rules still hold for any walkthrough that gets re-established:

1. **A trigger fires, you run it — no exemptions.** Whether an edit is "just text" costs more
   to adjudicate than running the walkthrough.
2. **Record the actual observations, never a bare pass claim.** A report of "W2 passed" is not
   a report.
3. **The tooling that drives a walkthrough lives in the repo**, tracked, under
   `docs/spikes/fixtures/local-rig/walkthrough/`. A hard gate whose tooling only exists in one
   machine's untracked directory is not a gate — it is a gate the next machine silently skips.
   Nothing new goes into `.playwright/`; that path keeps local browser config only.

## Language

**English is the default. Chinese survives only where a human reads it as prose.**

| Chinese | English |
|---|---|
| Strings rendered in the web UI (labels, buttons, error prose, the CJK glosses beside `SWAPPED` / `DISCARDED`) — the product serves Chinese users | Everything else in the repo: `CONTEXT.md`, `README.md`, code comments and identifiers, commit messages |
| | Log `event` names, field names, and internal error strings |

Editing an existing file, **its established language wins: convert it wholesale or leave it, never
half.** A file half-converted is worse than either end — it reads as though someone gave up midway
and leaves the next agent guessing which half is current.

The test is always **who reads it**: a person, as prose → Chinese. An agent, a compiler, or whoever
opens the repo next → English.

## Living docs state the present

`CONTEXT.md` is what an agent reads to know **what is true now**. Stale text in it does not just
waste tokens — it gets built.

**Supersede by rewriting in place.** When a decision changes, edit the sentence it
falsified and delete what it replaced. Never append a superseded-by layer: an entry
that states the old form first, then two overturns, makes every reader diff three versions
to recover one fact, and half of them get it wrong. The sequence belongs in git and in the
decision ticket, not in the entry.

The file says so in its own header. Keep it that way.

## Record retention

A timestamped record (`*-<ISO8601>Z.md`: acceptance runs, visual walkthroughs, rehearsal
and build logs) is evidence of one run on one day, not a document. **Each series keeps its
newest record only**; delete the older ones in the same commit that adds the new one.
Cite `git log --diff-filter=D -- <path>` when something needs a retired record.

One exception, load-bearing: the newest `packaging/centos7/records/build-*.md`.

## Commit trailer

Agent-authored commits end with exactly this trailer — no other spelling:

```
Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
```

History before 2026-08-15 also contains `Claude Fable 5`; that variant is retired, old commits stay as they are.
