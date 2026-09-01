# db-qbs

## Agent skills

### Issue tracker

Issues live as GitHub issues in `liumingjian/db-qbs`, managed via the `gh` CLI.

### Triage labels

Five canonical triage roles, each using its default label string: `needs-triage` (maintainer
needs to evaluate), `needs-info` (waiting on the reporter), `ready-for-agent` (fully specified,
ready for an AFK agent), `ready-for-human` (requires human implementation), `wontfix`.

### Domain docs

Single-context — `CONTEXT.md` at the repo root is the one present-tense domain document.
Accepted decision records may live in `docs/adr/`; GitHub issues and git remain the history.

## POC deployment standard

When a task targets the POC environment, read `packaging/poc/README.md` before changing
deployment, packaging, or environment assumptions. That document is the canonical POC
standard for hosts, paths, credentials, topology, ports, and acceptance checks. Database
connection values come only from `config/database.toml`; do not create a second database
configuration. `AGENT.md` is a link to this file, so edit `CLAUDE.md` only.
