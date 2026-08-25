# db-qbs

A database query-and-import service: connect to heterogeneous databases, run a query, and import the
result into a target. Only **MySQL** and **Oracle** need to be supported for now.

## Scope

- **Databases**: MySQL and Oracle. Others are out of scope.
- **Core capability**: connect to a datasource → run a query → export/import the result.
- **Shape**: a service (long-running, exposing an interface that triggers import tasks).

## Status

M1 (a one-shot process completing a single import), M2 (`source` as a resident service plus a web UI)
and M3 (the nine-row type surface, the mapping precheck, and value-domain checks) are **implemented**;
acceptance records live in `docs/spikes/fixtures/local-rig/`. M4 has not started, though failure
classification landed ahead of it. Column-name mapping is explicitly out of scope — renaming uses a
SQL `AS` alias. **Nothing has been deployed to production yet.**

Both ends are Rust (`crates/`); the web UI is React + Vite (`web/`), bundled at build time by
`crates/source/build.rs` calling `npm run build` and embedded into the `db-qbs-source` binary.
See `CONTEXT.md` for the architecture and `docs/adr/` for the decisions still in force.

## Quick start

Three binaries:

| Binary | Side | Role |
| --- | --- | --- |
| `db-qbs-sink` | target | Resident service, writes MySQL. It *is* the "target agent"; the source registers it from the UI (ADR-0044) |
| `db-qbs-source` | source | Resident service, web UI plus task orchestration |
| `db-qbs-source-run` | source | One-shot process running a single import (spawned by `db-qbs-source`, also runnable alone) |

Prerequisites: the source machine has the **Oracle Instant Client 19c Basic** bundle installed
(`oracle_client_lib_dir` points at it) and the target has **MySQL 8.0**. The build machine needs Rust
1.85+ and Node.js 22+ (`zeroize` in `Cargo.lock` requires edition2024, which Cargo below 1.85 cannot
resolve; Node 16 cannot build `npm run build`).

Artifacts destined for **CentOS 7 (glibc 2.17)** cannot be compiled directly on this build machine —
installing them there fails at startup with `GLIBC_2.xx not found`. That path goes through
`packaging/centos7/build.sh`, which builds both `linux/amd64` and `linux/arm64` in one command and
verifies each one starts on a clean same-architecture `centos:7`. See `packaging/centos7/README.md`.

Three more things under `packaging/` serve the path onto a customer machine: the **packing list**
checked off item by item before departure (`packaging/PACKING-LIST.md`), the **preflight self-check
for both ends** to run first thing on arrival (`packaging/preflight/`, which lists everything missing
in one pass), and the **stunnel templates for both sides** that encrypt the `source → sink` hop
(`packaging/stunnel/`).

```sh
cargo build --release

# target side
cp config/sink.toml.example sink.toml && chmod 0600 sink.toml   # set listen only
./target/release/db-qbs-sink --config sink.toml                 # first start writes agent-id next to sink.toml

# source side
cp config/source.toml.example source.toml && chmod 0600 source.toml
./target/release/db-qbs-source --config source.toml             # open the configured listen in a browser
```

The first stop in the UI is **Target Agent**: register one by entering the sink address above (it is
stored only if the probe succeeds), then select it when creating a MySQL datasource. **The target
database is reachable only through an agent**; there is no global fallback address.

**Neither `listen` is authenticated**, and both bind loopback by default. Multi-user access requires
your own reverse proxy in front doing auth and TLS. To run an import without the UI:

```sh
db-qbs-source-run --config source.toml --task task.toml --biz-date 2026-08-14
```

## Run logs

`db-qbs-source-run` and `db-qbs-sink` emit JSON Lines to stdout only. Failure records may contain
business column values, so tighten permissions *before* creating the file when redirecting:

```sh
umask 077
db-qbs-source-run --config source.toml --task task.toml --biz-date 2026-08-14 > run.jsonl
chmod 0600 run.jsonl
```

Log files must not be loosened to 0644, and must not be collected or forwarded beyond the target.
The full field contract is in `CONTEXT.md` under **Run Log**.

## Development

```sh
cargo test --workspace   # Rust unit and integration tests
npm install              # first time
npm run typecheck        # tsc --noEmit
npm test                 # vitest run
npm run dev              # front-end only (vite dev server)
```

Rig acceptance (M1's 9 cases, M2's A1–A14, M3's B1–B6) and the M2/M3 visual walkthroughs are
**manual gates with trigger conditions, deliberately kept out of CI**: the scripts live in
`docs/spikes/fixtures/local-rig/scripts/` and the checklists in
`docs/spikes/fixtures/local-rig/m2-visual-walkthrough.md` and
`docs/spikes/fixtures/local-rig/m3-visual-walkthrough.md`. Changing `docs/design-system/` requires
re-running the M2 walkthrough; changing the M3 failure-state layout or the diagnostic table's column
structure requires re-running the M3 walkthrough — and recording the actual observations. See
`CLAUDE.md` for the full gate table.

## Agent configuration

This repository follows the `mattpocock/skills` conventions:

- `CLAUDE.md` — the agent instruction entry point, containing the `## Agent skills` block
- `docs/agents/issue-tracker.md` — issues go through GitHub Issues (`gh` CLI)
- `docs/agents/triage-labels.md` — the triage label vocabulary
- `docs/agents/domain.md` — domain doc layout (single-context: `CONTEXT.md` at the root plus `docs/adr/`)
