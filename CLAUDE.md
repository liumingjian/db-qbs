# db-qbs

## Agent skills

### Issue tracker

Issues live as GitHub issues in `liumingjian/db-qbs`, managed via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage roles, using their default label strings. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context — `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

## M2 visual gate

Run `docs/spikes/fixtures/local-rig/m2-visual-walkthrough.md` for every M2 acceptance.
Any change to `docs/design-system/README.md` or `docs/design-system/tokens.css` must run the
same walkthrough before merge and record the actual observations, not only a pass claim.

## Commit trailer

Agent-authored commits end with exactly this trailer — no other spelling:

```
Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
```

History before 2026-08-15 also contains `Claude Fable 5`; that variant is retired, old commits stay as they are.
