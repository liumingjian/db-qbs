# db-qbs

## Agent skills

### Issue tracker

Issues live as GitHub issues in `liumingjian/db-qbs`, managed via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage roles, using their default label strings. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context — `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

## Visual gates

Three walkthroughs, one table. Rules 1-3 below apply to all three; they are not
per-milestone concessions.

| Walkthrough | Cases | Run it when |
|---|---|---|
| `docs/spikes/fixtures/local-rig/m2-visual-walkthrough.md` | V1-V25 | every M2 acceptance; **any** change to `docs/design-system/README.md` or `docs/design-system/tokens.css` |
| `docs/spikes/fixtures/local-rig/m3-visual-walkthrough.md` | W1-W6 | every M3 acceptance; any change to the `.precheck-reports` layout in `web/src/app.css` or to the `DiagnosticTable` column structure |
| `docs/spikes/fixtures/local-rig/v1-visual-walkthrough.md` | X1-X8 | every v1 acceptance; any change to the datasource screen, the builder mapping columns / target dropdown, or the four `app.css` rules in ADR-0039 §9 |

1. **A trigger fires, you run it — no exemptions.** Whether an edit is "just text"
   costs more to adjudicate than running the walkthrough. Rulings: ADR-0032 §8
   (M3 was a zero design-system change, so V1-V25 did **not** fire), ADR-0039 §9
   (v1 **does** fire V1-V25 — the README correction is a trigger, taken on the chin),
   ADR-0040 §6.2 (ADR-0036 §5 removed the shape-precheck section, so W1-W6 fires for v1).
2. **Record the actual observations, never a bare pass claim.** Each walkthrough
   file spells out its own record format; a report of "W2 passed" is not a report.
3. **If nothing triggered, say "not run" and why.** An acceptance whose changes touch
   no UI-affecting code (docs/ADR/fixture-only) may skip — but silence is not a skip.

## Commit trailer

Agent-authored commits end with exactly this trailer — no other spelling:

```
Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
```

History before 2026-08-15 also contains `Claude Fable 5`; that variant is retired, old commits stay as they are.
