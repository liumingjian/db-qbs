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
the rigs that accepted them live in `docs/spikes/fixtures/local-rig/`. M4 has not started, though
failure classification landed ahead of it. Column-name mapping is explicitly out of scope — renaming uses a
SQL `AS` alias. **Nothing has been deployed to production yet.**

Both ends are Rust (`crates/`); the web UI is React + Vite (`web/`), bundled at build time by
`crates/source/build.rs` calling `npm run build` and embedded into the `db-qbs-source` binary.
See `CONTEXT.md` for the architecture; the decision record lives in GitHub issues and git history.

## Quick start

Three binaries:

| Binary | Side | Role |
| --- | --- | --- |
| `db-qbs-sink` | target | Resident service, writes MySQL. It *is* the "target agent"; the source registers it from the UI (ADR-0044) |
| `db-qbs-source` | source | Resident service, web UI plus task orchestration |
| `db-qbs-source-run` | source | One-shot process running a single import (spawned by `db-qbs-source`, also runnable alone) |

Prerequisites: the source machine has the **Oracle Instant Client 19c Basic** bundle installed
(`oracle_client_lib_dir` points at it) and the target has **MySQL 5.7 or 8.0** (on 5.7,
`max_allowed_packet` must be raised to at least 64 MiB — its stock 4 MiB fails the Connection
Ritual). The build machine needs Rust
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

The UI asks for a login. The account is **`admin` / `admin`** and there is only one — no
registration, and the password changes only when you change it, from the menu at the top right.
Forgot it? On the source host:

```sh
db-qbs-source reset-password --config source.toml   # back to admin / admin, every session voided
```

**That login covers `source`'s HTTP face and nothing else.** `sink` is still unauthenticated and
holds `DELETE` on the target database, both `listen` addresses bind loopback by default, and the
password travels in cleartext unless you put TLS in front. Multi-user or off-host access still
requires your own reverse proxy doing TLS. To run an import without the UI:

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

Rig acceptance (M1's 9 cases, M2's A1–A14, M3's B1–B6, v1's C1–C6) is a **manual gate deliberately
kept out of CI**; the scripts live in `docs/spikes/fixtures/local-rig/scripts/` and the
visual-walkthrough runners in `docs/spikes/fixtures/local-rig/walkthrough/`. The checklists those
runners were judged against have been deleted — see `CLAUDE.md` under **Visual gates** for what that
leaves standing.

## Agent configuration

`CLAUDE.md` is the single agent instruction entry point: it carries the issue-tracker convention,
the triage label vocabulary, the language rule and the commit trailer. `CONTEXT.md` beside it is
the one domain document.
