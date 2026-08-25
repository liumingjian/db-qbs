# ADR-0044: The target sink becomes a first-class "agent": a registry, per-datasource binding, and the retirement of `sink_base_url`

**Status**: Accepted
**Date**: 2026-08-24
**Origin**: the owner's first piece of field feedback from the POC on 2026-08-24 (in conversation):
"the target MySQL is not actually reached through the sink agent — **I stopped the agent service and
synchronisation still worked**, which makes no sense. In practice source cannot reach MySQL directly,
and managing metadata also has to go through the agent. So once the agent starts there should be a
registration module in the front end, so it can be selected during datasource management."
**Related**: `ADR-0037` (the datasource model; **its §1 "one sink reaches any MySQL" is narrowed by §1
here to "one datasource binds one agent"**, and the comment on `SourceConfig::sink_base_url` — "there
is no compelling reason to bind a particular sink to a particular database" — is voided by §5),
`ADR-0038` (the target metadata surface; the two proxy entry points in its §3 are re-routed per agent
by §4 here), [ADR-0041](0041-v2-scope-trial-readiness.md) §4 (the stunnel tunnel — **direction
unchanged**, see §3), `ADR-0024` (neither `listen` is authenticated; the `GET /v1/agent/info` added
here lands on that same unauthenticated surface, see §2),
[ADR-0043](0043-p2-job-center.md) §2 (three nav items, **raised to four by §6 here**),
[ADR-0039](0039-v1-ui-increments.md) §2/§3 (the datasource screen's columns and "store only what
connects"; §6 here adds one clause to each)

## Background

After installing the product in the field and getting it working, the owner ran the plainest possible
check: **stop the agent on the target host and run the synchronisation again.** It succeeded.

That result is itself the criterion — **the product was lying about this.** From a user's point of
view, the process on the target host is "where this system stands on the target side"; if everything
carries on after stopping it, the data must have taken another route.

The facts in the code are worth recording more than the symptom: **`source` really does open no MySQL
connection** (`crates/source` does not even depend on a MySQL driver), and the writes really were done
by some `sink` process. The actual gap is here —

> The `sink` address is **a process-wide global setting**, `source.toml::sink_base_url`.
> There is **no binding whatsoever** between "which MySQL datasource" and "which sink", and a sink has
> never appeared in the interface at all.

So as long as **any** sink capable of reaching the target database still stands behind that global
address — one installed on the source host, one on another machine, one left running from a previous
rehearsal — stopping the one on the target host has no consequence. The "agent" the user sees and the
sink the product actually uses are two different things, and the product neither notices nor shows it.

The same gap has a second consequence, which the owner named in the same sentence: **metadata must go
through the agent too** (table listing, column fetch, connection test), and today those also hit the
global address.

## Decision

### 1. The sink becomes the first-class concept "target agent"; MySQL datasources bind one each; there is no global fallback

> **One agent = the `sink` process on the target side.** **Every** path to the target database —
> connection test, table listing, column fetch, and the writes of a run — must go through
> **the one agent that this datasource is bound to**.
> **`source.toml::sink_base_url` is retired** (migration in §5).

Three hard rules follow:

1. **`agent_id` is mandatory on a MySQL datasource** and must name an agent that really exists in the
   registry. An empty binding does not mean "use the default one" — it **cannot be stored**.
2. **There is no fallback path.** Agent unresolvable, agent offline, agent identity mismatched — all
   three **fail on the spot**. There is no "well, try the process-wide address" tier. That is the whole
   point of this ADR: leaving a fallback leaves the field incident exactly as it was.
3. **Oracle datasources bind no agent.** The source database is reached directly by source (the
   deployment shape in `CONTEXT.md` is unchanged), and giving it an agent field would only suggest
   something must be installed on the source side too.

**This narrows ADR-0037 §1**, which said one sink process can reach any MySQL and that "which sink" and
"which database" had no compelling reason to be bound in v1. There is a reason now, and it was found in
the field: without the binding, "swap an agent" and "stop an agent" are **both unrepresented** in the
product. The rest of ADR-0037 (credentials stored at source, crossing the wire with a run, never read
back) is **untouched** — this ADR changes routing, not credentials.

### 2. An agent has an identity stable across restarts, and `GET /v1/agent/info` is how it introduces itself

A new endpoint on the sink side, with no request body:

```
GET /v1/agent/info → { "agent_id": "...", "name": "...", "version": "..." }
```

- `agent_id` is **stable across restarts**: it lives in an `agent-id` file (0600) beside `sink.toml`,
  generated and written on first use if absent. **Deployers are not asked to prepare it** — the value
  itself carries no meaning, only its constancy does, and making someone copy a uuid by hand is just
  one more step to get wrong.
- `name` comes from `sink.toml::agent_name`, defaulting to the hostname. **It is never a criterion**;
  it only reaches the interface.
- `version` is the build version, used during troubleshooting to tell whether both ends are the same
  batch of binaries.

**Why an identity rather than a mere liveness probe**: liveness alone cannot catch "a different agent is
answering at the same address" — which is precisely the general form of the field incident (a wrong
address, another sink standing in, a tunnel pointing elsewhere). The identity is pinned into the
source-side record **at the moment of registration**, and compared on every probe and every run start.
A mismatch is judged `mismatch`, **not** `online`.

**It lands on the unauthenticated surface of ADR-0024**, so this endpoint carries **no credential field
and never may**: anyone who can reach the sink port could already read it. The payload shape is pinned
by `crates/shared/tests/protocol_golden.rs`.

### 3. Registration is "source dials out", not "the agent registers itself" — the network direction does not change by one character

The owner's words were "once the agent starts there should be a registration module in the front end".
**That is delivered as a front-end screen, but the handshake direction stays source → agent**: you enter
a name and address on the "Target Agent" screen, source immediately issues one `GET /v1/agent/info`, and
**it is stored only if the probe succeeds**. If it fails, the answer is "there is no live agent at that
address" and nothing is left in the database.

**Why agent-initiated registration (agent → source) is rejected**: the stunnel tunnel of ADR-0041 §4 is
**one-way** — the source side is the client and the target side only `accept`s. Letting the agent
register itself would require either a reverse tunnel or a channel from target to source, i.e. changing
the entire deployment and both installation manuals for a preference about who speaks first. The pull
model buys exactly the same things: registration requires the other side to be alive, status follows it,
and it is visible in the interface.

Liveness is maintained in two places, and **both are required**:

- **Background probe**: source polls every agent in the registry every 15 seconds, updating `status` /
  `last_seen_at` / `last_error`. This is the half that makes "stop the agent and the list shows it".
- **An immediate probe before use**: connection test, table listing, column fetch, and run submission
  each resolve and probe the agent before starting work. This is the half that makes "stop the agent and
  **this very action** cannot be done". With only the background probe there is a window of up to 15
  seconds; with only the immediate probe, the list would forever show the previous state.

### 4. All four target-side paths resolve the agent from the datasource; the run child process re-checks the identity

| Path | Entry point | Routing |
|---|---|---|
| Draft connection test | `POST /api/datasources/test-connection` | The agent currently selected in the form |
| By-id connection test | `POST /api/datasources/{id}/test-connection` | The agent bound to that datasource |
| Table listing | `POST /api/target/tables` | Same |
| Column fetch | `POST /api/target/columns` | Same |
| Run submission | `POST /api/runs` | The agent bound to the task's **target** datasource |

Run submission takes one extra step: the resolved endpoint (`agent_id` / `name` / `base_url` /
`instance_id`) is **pinned into the temporary task file handed to the run child process**
(`TaskConfig::agent`), and the child **re-checks the identity** before starting.

**Why check twice**: between "submit" and "actually start writing" lie the seconds-long jobs of opening
an Oracle connection, describing, and running `COUNT(*)`. If the agent is stopped or replaced during
that window, checking only the first time is the same as not checking. The child process **reads no
address from `source.toml`** — that field is retired, so the two-sources-of-truth problem is gone at the
root.

Failure classification reuses the existing closed set (ADR-0029, **no new values**): an unreachable agent
classifies as `NETWORK`.

### 5. Retiring `sink_base_url`, with a one-time migration

The shape copies the Oracle credential migration of ADR-0037 §10, with the same criterion of "the table
is empty", so it can only happen once:

- The field becomes **optional**, so an existing deployment's `source.toml` still parses.
- On first start with an empty agent table, it migrates into a single agent named **「默认」**
  (`instance_id` empty, status "not yet probed" — the network should not be touched that early in
  startup, and the first probe fills it in), and every **MySQL datasource not yet bound to an agent** is
  pointed at it.
- A `warn` is logged telling the deployer to delete the field and confirm on the "Target Agent" screen
  that it is online.

**Why backfill the datasources too**: without it, the first run submission after upgrading would report
"this datasource has no agent bound" — an error the user never configured and could not have
anticipated. With the backfill, behaviour matches the pre-upgrade state, and **the only difference is
that this route now has a name, a status, and visibility when it stops.**

### 6. Interface: a new first nav item "Target Agent", plus one column and one field on the datasource screen

- **The nav goes from three items to four** (rewriting ADR-0043 §2): **Target Agent · Job Center ·
  Datasources · Settings**. It comes first not out of layout preference — a MySQL datasource cannot be
  created without a registered agent, so this screen is the first stop when setting up a new machine.
- **The agent screen has a "status" column**, an **explicit exception** to ADR-0039 §2's "no connection
  status column", because the two differ in kind: that rule guards against the cost of background-polling
  every **business database** and against the lie of a stale green dot. Probing an agent is one `GET`
  against our own process — it touches no business database and consumes no database connection — and
  "is it alive right now?" is the entire reason this screen exists.
  Three states: `online` / `offline` / `mismatch` — **`mismatch` in red, `offline` in grey.** "It did not
  start" and "the thing standing at that address is not the one you think" are two different incidents,
  and merging them erases the clue.
- **The registration dialog has no "test connection" button**: submitting *is* a connection (§3), so an
  extra button only adds a click and buys no new information.
- **The datasource screen gains a "Target Agent" column**: MySQL rows show the agent name, followed by a
  tag when it is offline or mismatched — at that moment the datasource simply cannot be used, and hiding
  that only defers the discovery to submission time. **Oracle rows are empty**, not "not applicable"
  (which would suggest something was left unconfigured).
- **The agent is a required field in the form**, and it **joins the connection fingerprint of "store only
  what connects"** (ADR-0039 §3): swapping the agent means swapping the route to the target database, and
  the previous test result says nothing about the new one. When the registry holds exactly one agent it is
  **preselected** — most deployments in the field have exactly one.

## Consequences

- **The first start after upgrading automatically migrates one 「默认」 agent** and binds existing MySQL
  datasources to it. Behaviour matches the pre-upgrade state, but from now on this route is visible,
  stoppable, and swappable in the interface.
- **The agent on the target host is now a hard dependency**: stop it and connection tests, metadata
  fetches, and run submissions **all** fail. That is exactly what this ticket buys — but it also means
  **one operational slip (forgetting to start the agent) surfaces as a screen-wide failure** rather than
  quietly taking another route, as it does today. That is a deliberate trade.
- **One more network round trip**: every target-side path probes the agent before starting (5s connect /
  5s read timeouts). The cost is tens of milliseconds in the interface, in exchange for the assertion
  "it is genuinely alive right now".
- **The `agent-id` file becomes a piece of target-side state.** Deleting it by accident = that agent has
  changed identity, source judges `mismatch` and stops every path, and the remedy is to save it again on
  the agent screen (re-pinning the identity). This belongs in the installation manual.
- **This ADR adds no authentication.** Agent registration has no token and `/v1/agent/info` is
  unauthenticated — ADR-0024's "reachability equals privilege" stands verbatim. This ticket neither
  improves nor worsens it.

## Validity

**Reopening signals**: a deployment where one source manages more than ten agents (at which point the
list, filtering, and grouping all need redoing), or a hard requirement for **agent-initiated reporting**
(the target sitting behind NAT so source cannot dial in). The latter would directly overturn the
direction ruling of §3, and the tunnel shape of ADR-0041 §4 would have to change with it.

## Walkthrough triggers

The datasource screen's column structure, the datasource form, and the shell's nav items all changed;
per the table in `CLAUDE.md`:

- **X series (v1 walkthrough) fires**: **X1 re-judged** (nav goes from three items to four, Target Agent
  first, datasources third), **X2 re-judged** (the datasource screen gains a "Target Agent" column), and
  **X19 added**: "Target Agent screen: registration / probing / the three states / deletion refused / the
  required dropdown in the datasource form". X3's criterion is unchanged, but **reaching it takes one more
  step** (a MySQL draft must select an agent first). The numbering rule holds as always: nothing is
  renumbered, no line is deleted.
- **V series (design system) fires**: `tokens.css` and `app.css` changed by not one character and there is
  **not one new component**, but the screen inventory in `docs/design-system/README.md` §7 gained the
  "Target Agent screen" entry (a new screen missing from the inventory makes the inventory wrong). Per
  `CLAUDE.md` rule 1, **changing it means running V1–V25, with no exemption** — "it is only some text" is
  exactly what that rule forbids saying. No criterion changed; this is a regression run.
- **W series (M3)**: `.precheck-reports` and `DiagnosticTable` are unchanged, so it **does not fire**.
