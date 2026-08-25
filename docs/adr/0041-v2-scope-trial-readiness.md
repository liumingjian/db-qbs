# ADR-0041: v2's scope is set by "ready for the trial next week" — no new features, only the delivery path; the two-host topology and the public-internet channel are the first real settlement of the deployment premises

**Status**: Accepted
**Date**: 2026-08-19
**Ticket**: [#140](https://github.com/liumingjian/db-qbs/issues/140) (the v2 map). **The spec and implementation tickets do not exist yet** — see addendum 3
**Precedent**: `ADR-0034` (v1's scope set by the customer's five requirements) — this ADR is its **successor**, not its fulfilment

## Background

v1 closed on 2026-08-19 ([#136](https://github.com/liumingjian/db-qbs/issues/136) accepted with all four rigs green;
the loose ends [#137](https://github.com/liumingjian/db-qbs/issues/137) /
[#139](https://github.com/liumingjian/db-qbs/issues/139) are cleared).

Where the scope comes from has no ready answer this time. ADR-0034 set v1's scope as "by this one customer's five
requirements", and wrote down its own retirement signals: **a second deployment appears, or the customer asks for
something shaped unlike the existing requirements**. The situation when the map was opened is that
**v1 has not yet been handed to the customer**, so neither signal has fired.
So "copy v1's way of setting scope" does not apply, and neither does "work the backlog in the ADRs": without
exception those backlog items are triggered by "some kind of deployment appears / a real need is voiced", and
ordering them at zero deployments orders the implementer's technical preferences, not what the customer wants.

### Owner rulings of 2026-08-19 (five rounds of grilling; the input to this ADR)

| # | Ruling |
|---|---|
| 1 | **v2 is set by "make the delivery actually happen"**, not by the backlog |
| 2 | **The on-site trial is next week**; the customer is in a hurry |
| 3 | **The owner installs it in person**, the customer supplies the server, the owner brings the environment; **no over-packaging** |
| 4 | The trial's goal is **basic functions usable**, one business scenario running end to end |
| 5 | The customer side is a **test environment**; target tables may be altered directly |
| 6 | **The customer's data contains no Chinese**; the data volume is capped at about **100MB** and will not grow |
| 7 | If something goes wrong during the trial **the customer contacts the owner**; the product does not notify proactively |
| 8 | **Two hosts**: `source` on the source side (Oracle), `sink` on the target side (MySQL); there is no network between the two databases, only `source → sink` over the public internet |
| 9 | That public path **runs on a whitelisted port**; access control is the network layer's job |
| 10 | Servers: **CentOS 7**, outbound access, root, ample resources |

## Decision

### 1. Where v2's scope comes from: **the delivery path, not a feature list**

**v2 adds no transfer capability at all.** It buys exactly one thing: **that this can be installed on the customer's
machines and run once.**

The basis is rulings 1 and 2 together. Not having delivered is itself the biggest unknown right now — which backlog
item to do will surface once the customer actually uses it, and doing it before then trades hours for guesses.
And the trip is next week, so this version's budget is **one week**, which does not hold a second thing.

**Consequence**: ADR-0034's expiry clause "re-assess generalisation in v2" **does not happen in this version**.
Its trigger (a second deployment) still has not fired, and this version does not create it.

### 2. Four deliverables, not one line of new function

1. **A two-host installation rehearsal** — from zero on clean CentOS 7: `source` + Oracle Instant Client 19c on the
   source side (against the customer's Oracle 11g), `sink` on the target side (against MySQL 8.0).
   **The rehearsal is the means of producing the manuals, not a means of verification**: the manuals must be a record
   of a walk actually taken, not written from imagination.
2. **Two installation manuals** — one per side, **written for the owner himself** (mostly commands). A customer-facing
   version waits until the trial is over and it is known what customers actually ask (ruling 3, "no over-packaging").
3. **One environment self-check per side** — the first thing run on the machine; whatever is missing, whichever
   database is unreachable, whether the tunnel is up, **listed in full on the spot**, rather than blowing up halfway
   through the install. The criterion is "on site, once the self-check says OK there should be no further
   environment-class failure".
4. **A double-ended `stunnel` tunnel** — configuration and installation go into the manuals; see §4 for why.

### 3. Explicitly out of scope, with reasons

| Backlog item | This version | Reason |
|---|---|---|
| Delete propagation (ADR-0035 expiry 1) | no | the trigger is "a deployment with physical source-side deletes appears"; not fired |
| Filter expressiveness `IN`/`BETWEEN`/`sysdate-1` (ADR-0035 expiry 2) | no | the trigger is "a real need is voiced"; not fired. What the trial scenario needs is filtering by date, and the existing form is enough |
| `purged_rows` → `updated_rows` (ADR-0035 expiry 3) | no | it is queued behind "do it in passing next time ADR-0010/0017/0020 are touched", and this version touches none of them |
| Business date derived from the schedule (ADR-0035 expiry 4) | no | it drags in scheduled runs, an explicit non-goal |
| True character-length semantics (ADR-0033 expiry) | **no** | **the customer's data contains no Chinese** (ruling 6), so the 4x lower bound of `VARCHAR2(n CHAR)` will never be seen in this deployment. **Note this is a fact about one deployment, not a general judgement** — ADR-0033's item stays on the backlog with its trigger unchanged |
| Generalisation: type surface / multiple shapes / authentication (ADR-0034 expiry) | no | see §1 |
| Splitting `TaskFormDialog` | no | pure internal tidying, zero customer-visible benefit, and this version's budget does not hold it |

### 4. The public channel: **whitelist + `stunnel` tunnel, zero product changes**

Rulings 8/9 make two long-standing items in the deployment premises real for the first time:

- **The MySQL password crosses `source → sink` in cleartext** (`ADR-0037` §4).
  The original premise was "the channel must be trusted — same host, trusted intranet, or TLS/tunnel built by the
  deployer". It is now **the internet**.
- **`sink` has no authentication** (`ADR-0024`), and the design's fallback was "bind loopback only".
  Now `sink` must be reachable by a `source` on the public internet.

**Ruling: access control is the customer's whitelisted port (ruling 9), confidentiality is a double-ended `stunnel`
tunnel, and not one line of product code changes.**
`source` connects to the local tunnel port, encryption crosses the internet inside the tunnel, and the target-side
tunnel lands on a loopback `sink` — **so `sink` still binds loopback only, and ADR-0024's fallback shape holds as
written**.

**Why not TLS inside the product** (the most expensive restraint in this version):

- `source` sends HTTP with **`ureq` compiled without TLS** (`Cargo.toml`: `default-features = false, features = ["json"]`);
- `crates/source/src/protocol.rs:100` **hard-rejects any `sink_base_url` that is not `http`**, so `https://` fails at startup.

In other words TLS inside the product = adding the `rustls` feature + relaxing the scheme check + terminating TLS in
`sink` + re-running all four rigs. Within a one-week budget, what that displaces is the installation rehearsal itself.
**The tunnel gives equivalent confidentiality, at the cost of two extra installation steps.**

**Backlog (re-assess after the trial)**: TLS inside the product, and credential checking in `sink`. Triggers: a
deployment **not installed by the owner in person**, or the customer's security review demanding the product prove
the channel secure by itself — at which point the tunnel, as "built by the deployer", is the thing that retires.

### 5. Build target: CentOS 7's glibc 2.17 is a hard constraint, **and musl is not a way around it**

- The customer machine is CentOS 7 (glibc **2.17**). Binaries built on a newer Linux or on macOS **will not run there**,
  failing at startup with `GLIBC_2.xx not found` — **this must be hit in the rehearsal, not on site**.
- **Static linking against musl is not an option**: `source` loads `libclntsh.so` through Oracle's OCI, and Instant
  Client is a glibc dynamic library that a musl target cannot load.
- **Ruling: build inside a `centos:7` container**, both binaries. Two side effects are handled in the rehearsal as
  well — CentOS 7 is EOL and its default yum repos are dead (use `vault.centos.org`), and `rusqlite` uses bundled
  SQLite so a C compiler is needed at build time.

### 6. Acceptance criteria: **install by the manual and transfer once**, no new rig entry point

- **No fifth rig entry point.** ADR-0040 §2's letters partition **entry points for transfer semantics** (M2=A, M3=B,
  v1=C), and this version changes zero transfer semantics; a new letter would only make "entry point" drift as a concept.
- **This version's criteria are procedural and live in the rehearsal record**: on a clean CentOS 7,
  **by the manual only** (no improvised commands), install both sides, all self-checks green, one transfer end to end.
  **Any "the manual did not say, so it was solved on the spot" counts as the criterion not met** — that is exactly
  what will stall the owner on site, and it must be written back into the manual and walked again.
- **The four existing rigs stay green as before** (this version does not change transfer semantics, so nothing about
  them should change).
- **The three walkthroughs**: this version does not touch the UI, so per `CLAUDE.md` rule 3 write "not run and why",
  with seal-point diff evidence attached.

### 7. The trial scenario is **an assumption**, not a settled fact — **voided by addendum 2 below**

The business scenario used for acceptance is: **filter yesterday's business detail rows by date and upsert them by
primary key into the corresponding target table.**

**This was an assumption the customer had not confirmed** (the owner agreed to proceed on it while asking for the
real scenario). **Voided the same day, 2026-08-19**: the owner supplied the customer's real words; see addendum 2.
This section is kept for the record — do not cite it as current.
**No later decision may cite it as a settled fact**; once the customer gave the real scenario it was replaced in
place, and that replacement is not rework.

### 8. Factual premises (this version's inputs; get one wrong and come back to change this section)

| Surface | Fact |
|---|---|
| Topology | two hosts: `source` + Instant Client on the source side, `sink` on the target side; **no network** between the two databases |
| Channel | `source → sink` only, over the public internet on a whitelisted port; confidentiality by `stunnel` |
| System | CentOS 7, outbound access, root, ample resources |
| Source | Oracle **11g** |
| Target | MySQL **8.0**, `utf8mb4`; the target tables **already exist**, and being a test environment they **may be altered directly** |
| Data | capped at about **100MB**; **no Chinese** |
| People | the owner installs in person; during the trial the customer uses it alone and contacts the owner on trouble; the product does not notify proactively |

## Consequences

- **Good**: the whole one-week budget goes to the only thing certainly worth doing — turning "not delivered", the
  biggest unknown, into a known. After the trial, which backlog item comes first will be answered by the customer's
  actual use rather than ordered by guesswork.
- **Bad**: this version is a zero-visible increment **to the customer** — what the owner installs is, functionally,
  v1. If the customer expects "some new features again", this version has no answer, **and the owner knows and
  accepts that**.
- **Risk**: the target-side entry (public IP / port / whitelist) is supplied by the customer, and **the product
  cannot prove it from its side**. Not having it means installation day stops dead — it is this version's only
  external blocker, and is the top tracked item on map #140.

## Expiry

**Re-assess as soon as the trial is over.** Three triggers; any one of them retires §1 of this ADR:

1. **The customer actually uses it** — scope then returns to "set by real usage feedback", and the backlog is
   reordered by what the customer hit;
2. **A second deployment appears** — ADR-0034's expiry clause finally fires and the generalisation debt starts being paid;
3. **A deployment not installed by the owner in person appears** — §4's "tunnel built by the deployer" loses its
   premise, and TLS inside the product plus credential checking in `sink` become mandatory.

## Addendum (2026-08-19, two further owner inputs)

### 1. The rehearsal environment becomes **Docker containers on the mac**; the customer's real machines are adapted on site by the owner

Owner ruling: **prefer simulating the customer environment with Docker on the mac**, and let the owner absorb the
real machines' differences on the day.

- The rehearsal rig becomes two `centos:7` containers acting as "source host" and "target host", in the same compose
  file as the existing local rig's `qbs-oracle11` / `qbs-mysql8`; the cross-container hop simulates the public
  internet, with `stunnel` running on both ends as designed.
- **This does not lower the manual's criteria** (§6 unchanged): the manual must still be a record of "install by
  following this alone". Differences between containers and real machines (kernel, SELinux, firewall, packages
  already installed) are absorbed on site by the owner — **the manual must mark the steps that may differ on a real
  machine**, rather than pretend there is no difference.
- **The cost, stated plainly**: in a container root is the default, the package manager is clean and the network
  works. Those three are exactly what most often stalls people on a real machine. This version accepts that cost,
  because the person installing is the person who wrote the manual.

### 2. The trial scenario is no longer an assumption — **the customer's key features are in**, and one of them is currently missing

§7's "an assumption, not to be cited as settled before the customer confirms" **is void as of now**. The owner's
report of the customer's actual words:

> 可以查数据、加过滤条件、导入目标数据库；能看到执行的进度；失败了可以重试。
>
> (Query data, add filter conditions, import into the target database; see execution progress; retry after a failure.)

Item by item against the current state:

| What the customer said | Current state | This version |
|---|---|---|
| query data | builder + column fetch + generated SQL (ADR-0027/0038) | already there |
| add filter conditions | constant conditions / fill at run time (ADR-0036 §1) | already there |
| import into the target database | primary-key upsert (ADR-0035) | already there |
| see execution progress | the phase string + an **indeterminate progress bar** + four real numbers (rows pushed / batch number / elapsed / cumulative bytes) | already there, **but no percentage** |
| retry after a failure | **missing** — the only way is to go back to the task screen and start that task again | **add a one-click re-run** |

**No percentage, and not this time either.** ADR-0017 C4 and `ADR-0026` §3 both rejected it explicitly: a
denominator requires a `COUNT` up front, that denominator sits at a different SCN from the numerator's
single-snapshot cursor, so the percentage would overshoot 100% or stall forever at 97% — and would pay an
`ORA-01555` risk for decoration. **If the customer insists on a percentage during the trial, that is a re-assessment
trigger for after the trial** (recorded into the signals of ADR-0026's expiry clause); nothing changes this week.

**The one-click re-run is in this version**, because the customer named it as a key feature and it costs almost
nothing: the history record already stores `run_params` (`api.ts:231`), the start entry point already exists as
`startRun(taskId, runParams)` (`api.ts:446`), and all that is needed is **prefilling the failed run's parameters into
the start dialog**.

**This does not overturn §1**: §1 says no new **transfer capability**. A one-click re-run changes no transfer
semantics, touches no part of the path, and moves none of the four rigs' criteria; it turns "the action the customer
named" from two steps into one. **It is this version's only functional increment, and no second exception is opened.**

### 3. Spec and implementation tickets are generated by `/to-spec` and `/to-tickets`, **never handwritten**

On 2026-08-19 a spec (#141) and seven implementation tickets (#142–#148) were handwritten and **have all been deleted**.

**The rule**: specs in this repo go through `mattpocock-skills`' **`/to-spec`**, and implementation tickets through
**`/to-tickets`**. Both skills have `disable-model-invocation: true` in their frontmatter — **the model cannot invoke
them; only the owner can.** Handwritten output does not match the templates (`to-spec` wants a long list of User
Stories and forbids file paths in Implementation Decisions; `to-tickets` wants What to build plus checkbox Acceptance
criteria, must quiz the owner on the split and get approval before publishing, and must use GitHub's native blocking
relationships), and more importantly it bypasses the "owner approves the split" step.

**Therefore: this ADR's rulings stand, but it has no accompanying spec or tickets; both are generated by the owner
running `/to-spec` and `/to-tickets`.** The "customer's five key features" table in addendum 2 and the four
deliverables in §2 are inputs to generating the spec, not the spec itself.

### 4. The rehearsal rig's numbering, the narrowing of criterion 1, and what the "cross-container reachability" negative criterion was really worth (2026-08-20, two-axis review after #152 landed)

Once #152 stood the rehearsal rig up, the two-axis `/code-review` caught three things to nail down.

**(a) `R0–R10` is not a fifth rig letter.** ADR-0040 §2's A/B/C number **acceptance scenarios for transfer
semantics**; R numbers the rehearsal rig's own topology self-check and touches no transfer semantics at all.
v2's acceptance criteria are procedural and live in the rehearsal record (§6); no new acceptance entry point is
opened. This ruling previously lived only in the body of `local-rig/README.md`, and is nailed here now.

**(b) #152's criterion 1, "starts and stops with the existing orchestration", yields to criterion 3 and narrows to
"stops with the existing orchestration".** The two hosts live under compose's `rehearsal` profile: `down.sh` carries
`--profile rehearsal` so teardown takes them with it (without it, all three networks fail to delete as "still in
use", leaving a half-torn rig); the **start** half is not folded into `up.sh` — folding it in would add two centos:7
containers and two networks to the four existing rigs' start/stop, breaking the same ticket's criterion 3 ("the
existing rigs are unaffected") on the spot. When two criteria conflict, yield to criterion 3; starting uses
`rehearsal-up.sh`.

**(c) "Cross-container reachability is severed" was a falsely green negative criterion on mac Docker.**
The original R6 connected to the target side **by container name**: the source host is not on the target's network,
so the failure happens first at name resolution rather than routing — and "DNS cannot find it" is precisely the
false-green cause the criteria script itself names. Changed to connect directly to the target's IP on
`qbs-dst-side`, with a positive control alongside (the target reaches itself through the same IP and gets a token),
R6 **FAILed immediately**: Docker Desktop forwards between two bridge networks, and `172.30.0.3 → 172.29.0.3:15443`
got a token straight away. R3/R5 (the two databases being mutually unreachable) were judged by container name too,
the same cause over a larger area.

**Ruling: negative criteria are always judged by IP, and every one carries a same-address positive control; the
severance is imposed by the rig explicitly, never left to Docker's default behaviour.** It is imposed **from
outside**, by blackholing routes in the two hosts' network namespaces (a temporary helper container with
`--network container:` and `--cap-add NET_ADMIN`); the host under rehearsal installs nothing itself and does not even
have `ip` — so the "clean machine" premise holds as written, and the severance is an external fact imposed by the
rig, just as a firewall would be at the customer site. Deleting the container resets it; `rehearsal-up.sh` re-imposes
it on every start, and `rehearsal-reset.sh` goes through the same path.

### 5. "No network between the two databases" still owed one path: the one through the host gateway (2026-08-20, second two-axis review of #152)

After addendum 4 changed the negative criteria to IP-based and had the rig impose blackhole routes explicitly,
`R3/R3b/R5/R5b` all went green. **But that topology still did not hold**: each database publishes a port on the host
(`1521:1521` / `3306:3306`), and the host is exactly where the "one public hop" lands — the source host reaching
MySQL `3306` via `host.docker.internal` **was measured as reachable** (Docker Desktop supplies an IPv6 gateway
`fdc4:f303:9324::254`, unrelated to those IPv4 blackholes). `README.md`, meanwhile, stated
"cross-container reachability is severed / an unpublished port is a port outside the whitelist" as a universal conclusion.

**This path cannot be closed by removing the published ports**: the source in the four existing rigs runs on the mac
host and depends on exactly those two published ports, so removing them breaks the same ticket's criterion 3
("the existing rigs are unaffected") on the spot. Nor can the gateway be blackholed wholesale: `15443` on that same
gateway is the whitelisted hop, and the two hosts still need outbound access (`centos:7` is EOL, so installing
packages requires switching repos to `vault.centos.org` first — the manual's very first step, and the rehearsal rig
should not be more lenient than a real machine on that point).

**Ruling: severance has two layers, and the second is judged per port.**

1. **Blackhole routes** — the other side's network plus the default network's subnet. This blocks "every port on that
   machine and that database".
2. **Port-level DROP, in both the IPv4 and IPv6 tables** — the source host drops all outbound `3306`, the target host
   drops all outbound `1521`. This blocks "the path that bypasses routing". Each host's own database uses the other
   port and is unaffected; `15443` on the gateway and everything needed for outbound access are untouched.

It is imposed through the same path as addendum 4: a one-shot alpine sharing the rehearsed host's network namespace
(`--cap-add NET_ADMIN`), with not a byte changed on the machine under rehearsal, and deleting the container resets it.

**Three criteria follow**: `R3c` (source → host `3306` unreachable), `R5c` (target → host `1521` unreachable), and
`R5d` (target → host `15443` reachable) as `R5c`'s same-address positive control; `R3c`'s positive control is the
existing `R7`. **Without that control, R3c would go green for the wrong reason whenever the gateway path went down
entirely** — the very false green addendum 4 ruled on, replayed at a different address.

**Two more nailed down at the same time**: (a) `R0` (same architecture, same glibc) is #151's build target, not
#152's topology criterion — keep the two ledgers separate and do not pass another ticket's evidence off as this
one's; (b) the rehearsal rig's three scripts gain a static self-check
`scripts/test-rehearsal-topology.sh`, consistent with the four existing rig entry points — a criteria script is a
gate, and if the gate has no gate of its own, deleting a negative criterion is only discovered on the next real run.

### 6. The tunnel's concrete shape, the boundary of the stub sink, and the ordering of two sets of criteria (2026-08-20, #153 landed)

§4 only ruled "confidentiality by a double-ended stunnel tunnel, not one line of product code changed". Landing it
raised five things to nail down.

**(a) The tunnel both encrypts and authenticates, and what it authenticates is the pinned certificate.**
Each end has a self-signed certificate, and each puts **the other's** into its `CAfile` with `verify = 2`; no CA is
built. CentOS 7's stunnel is **4.56**, which has no `checkHost` — **pinning the certificate file itself is the
identity check.** Encryption without authentication would let a man in the middle present another self-signed
certificate and be accepted, and a third party on the public internet is exactly what §4 must block. The reason for
not building a CA: there are two identities in total, and the only benefit of an issuing chain is "a third one can be
signed later" — but a third one appearing (a deployment not installed by the owner) is precisely the trigger for the
tunnel approach in §4's backlog to **retire**. On that day the scheme changes, not the number of layers.
The protocol floor is TLS 1.2 (CentOS 7's OpenSSL 1.0.2 reaches 1.2, not 1.3); the measured negotiation was
`TLSv1.2 / ECDHE-RSA-AES256-GCM-SHA384`. Private keys are generated in place and carried by hand, **never entering
version control**, and never crossing the tunnel itself.

**(b) Where the ports land: not one character of `sink_base_url` changes.** The source-side stunnel client binds only
`127.0.0.1:8080`; the target-side server accepts on the whitelisted port `15443` and lands on the `sink` at
`127.0.0.1:8080`. `8080` is exactly the current value in `config/source.toml.example` and `config/sink.toml.example` —
**so "zero product changes" means more than "no code changed": even that value in the sample configs is untouched.**
`sink` still binds loopback only, and ADR-0024's fallback shape holds as written.

**(c) What #153 got working is the tunnel, and it lands on a stub sink, not the real one.** The real `sink` arrives
with #156. This ticket proves the tunnel segment, and whether the landing point is the real product does not affect
the criteria — **but "loopback only" must be identical**, and the stub binds `127.0.0.1:8080` for exactly that
reason; criterion `T4` (the target cannot reach `8080` through its own side-network IP) is what judges it.
This is written down so that nobody later reads #153's green as "sink is installed and working".

**(d) The two sets of criteria have an order: topology first, tunnel second.** `rehearsal-topology-check.sh`'s
`R7a/R8a` need the target to start its own probe listener occupying `15443`, and after the tunnel is installed that
port belongs to stunnel. Measured on 2026-08-20: running the topology criteria while the tunnel is up turns
`R6a/R7/R7a/R10` **red together**, and all four share one cause, none of which is a topology problem.
**Ruling: run the topology criteria first, then install the tunnel; to re-check topology after installation, tear
down and rebuild with `rehearsal-reset.sh` first.** The topology script gains an occupancy check before starting the
probe, so it says that cause out loud — a negative criterion going red for the wrong reason is the other face of a
false green.

**(e) `T0–T11`, like `R0–R10`, is not a fifth rig letter** (the same ruling as addendum 4(a)): it numbers the
rehearsal rig's self-check for the tunnel segment and touches no transfer semantics. The two tunnel scripts likewise
get a static self-check `scripts/test-rehearsal-tunnel.sh` — **and this ticket's fourth criterion, "zero product code
changes", is judged there**: it is a static fact, and the rig can only see what running looks like, not whether the
scheme check was quietly relaxed to make it run.

That gate **judges by content, not by a branch diff.** Judging by diff breaks in two places: a shared static
self-check would go red innocently on a **sibling ticket's** branch (one touching `web/` for the one-click re-run,
say); and once this ticket merges into `main`, `merge-base` is `HEAD`, the diff is always empty and the criterion is
permanently green — **a permanently green gate is not a gate**. So what it asserts is the three pieces of content the
tunnel approach rests on: `protocol.rs` still hard-rejects a non-`http` `sink_base_url`, `source.toml.example`'s
`sink_base_url` is still `http://127.0.0.1:8080`, and `sink.toml.example`'s `listen` still binds loopback only.
Change any one of the three and "zero changes" stops being true and the gate goes red on the spot —
**red after the merge too**.

### 7. Two landing rulings for the source-side installation manual, and one false green in self-check S4 (2026-08-20, #155 landed)

§6 only ruled "install by the manual and transfer once". Landing the source-side half raised three things to nail down.

**(a) In a rehearsal the rig prepares the far end; the near end must be typed by a human following the manual.**
`rehearsal-tunnel-up.sh` gains `--side both|source|target`: #155 used `--side target` to prepare only the target end,
and walked the source end through the manual line by line. If the script does the near end, "the manual is a record
of a walk actually taken" is void on the spot — and that is the whole of §6.
Likewise `--sink stub|real` is added: **source-side self-check S8 judges the product's own error code `RUN_UNKNOWN`**
(#154), which #153's stub sink cannot return, so #155's far end is the real `db-qbs-sink` built by #151.
The cost, stated plainly: under `--sink real`, `rehearsal-tunnel-check.sh`'s T3/T5/T7 judge by the stub's marker and
go red on the marker; **the encryption evidence for the tunnel segment remains #153's record, and is not redone in #155.**

**(b) Self-check S4 was a false green, because it added `LD_LIBRARY_PATH` on the product's behalf.**
In the source-side rehearsal of 2026-08-20, after `S1–S8` all went green, "test connection" immediately gave
`DPI-1047 ... libnnz19.so: cannot open shared object file` — **#154's criterion "after the self-check says OK, no
environment-class failure should appear on site" broke on the spot.** The cause: ODPI-C `dlopen`s only `libclntsh.so`
by full path, while its siblings `libnnz19.so` / `libclntshcore.so` are found by the dynamic linker on **its own**
search path; without the Instant Client directory in `ldconfig` they are not found. And S4 pushed that directory into
`LD_LIBRARY_PATH` before checking, so what it checked was not the path the product will take.

**Ruling: a self-check always judges by the product's own search path, and never paves the road before checking it.**
S4 becomes `env -u LD_LIBRARY_PATH ldd` — **"this script does not add it" is not enough, the inherited one must be
wiped explicitly**: putting `export LD_LIBRARY_PATH=/opt/oracle/instantclient` into root's profile is the commonest
habit on machines like these, and the `db-qbs-source` started by systemd inherits no profile, so leaving it in checks
"can this shell load it right now" rather than "can the service process load it". If what is missing would "all be
there once the Instant Client directory is added", the remedy gives the two `ldconfig` commands directly, kept
separate from the "the package is not installed" cause, and **when both hold, list them together in one pass**
(script-header discipline 1: reporting only one means making people clear one and hit the next).
Step 4 of the manual gains `ld.so.conf.d` + `ldconfig` accordingly.
**Negative control** (having changed it, prove it really catches things): on an installed machine, remove that conf,
run `ldconfig`, and S4 goes red on the spot with the `ldconfig` remedy; restore it and it returns to 8/0.

> An incidental observation, do not misread it: after removal, an **already running** source process still returns
> `ok:true` on test connection — ODPI-C initialises once per process. This fault only appears when a new process
> connects for the first time, so the self-check is stricter than a running process, and that is correct.

**(c) `S` is not a fifth rig letter** (the same ruling as addendum 4(a), cited for the fourth time): the source-side
installation criteria live in the record under `docs/install/records/` and touch no transfer semantics. The manual
and the record share one documentation area (spec #149 E.17); the rig side holds only the "walk it on the rehearsal
rig" half, plus a static self-check that starts no rig, `test-rehearsal-source-install.sh` — which guards
**that the manual and the replay script say the same thing**: every one of the self-check's S1–S8 appears in the
manual, the real-machine difference markers number at least the five classes the spec names, ports and paths match on
both sides, and the vault repo-switch section points at the same archive as the build image (there are now four
implementations of that section).

### 8. Landing rulings for the target-side installation manual, and the tunnel criteria holding on a real sink for the first time (2026-08-20, #156 landed)

§6 only ruled "install by the manual and transfer once", and addendum 7 landed the source-side half. Landing the
target-side half raised five things to nail down.

**(a) The far end swaps direction, so the manual order is fixed: target side first, source side second.** #155 had
the rig prepare the target (`rehearsal-tunnel-up.sh --side target --sink real`) and a human type the source; #156 has
the rig prepare the source (`--side source`: stunnel client + certificates + `openssl`) and a human type the target.
The target's own self-checks D1–D9 **do not depend on the source**, which is used only at step 10 of the manual
("check once from the public side") — whereas the source's self-check S8 must reach the sink on the target's
loopback, so in the reverse order the last step of the source's self-check cannot go green. Both manuals and
`docs/install/README.md` say this.

**(b) The tunnel criteria hold on a real sink for the first time: `rehearsal-tunnel-check.sh --sink stub|real`.**
The cost stated in addendum 7(a) (under `--sink real`, T3/T5/T7 judge by the stub's marker and go red on it) is no
longer acceptable once the real sink is this ticket's deliverable — a criterion guaranteed to be red on the
deliverable is not a criterion. Ruling: **the criteria recognise a fingerprint per landing-point kind**; the stub is
recognised by the marker line it returns, the real sink by the product error code `RUN_UNKNOWN` inside the 404 it
returns for a nonexistent run (the same fingerprint as source self-check S8 and target self-check D2; "somebody
answered" does not count, since a tunnel landing on some other service also produces an answer). The real sink has no
health endpoint (its whole route set is `/v1/runs*` and `/v1/target/*`), so the probe hits
`/v1/runs/__tunnel-probe__`. The landing-point kind is declared by the caller and must match
`rehearsal-tunnel-up.sh --sink`; a mismatch turns three criteria red on a false cause — the same class of problem as
addendum 4's "a negative criterion going red for the wrong reason". #153's stub-sink record remains the first piece of
encryption evidence for the tunnel; this ticket re-walking T0–T11 on a real sink is the second.

**(c) The three MySQL premises are judged by sink, and no MySQL client is installed on the target host.** The client
in CentOS 7's base repo is the 5.x generation, which cannot authenticate against MySQL 8.0's default
`caching_sha2_password`, so installing it produces a fake fault (already ruled in #154; here it becomes a "do not
install" note in the manual). So step 7 of the manual **has no command to type on this machine**; it is a note for
the customer's DBA: the account's `GRANT` (`SELECT, INSERT, UPDATE, CREATE, DROP` — `information_schema` metadata,
the `INSERT … SELECT` swap segment, creating and dropping the staging table; no `DELETE`, since ADR-0035's upsert
deletes no rows), two `my.cnf` lines (`character-set-server = utf8mb4`, `max_allowed_packet = 64M`), and that
`init_connect` / any middle layer must not rewrite the session `sql_mode`. The same number appears in three places
(sink's `MIN_PACKET`, the self-check's `MIN_PACKET=67108864`, and the 67108864 given to the DBA in the manual) and is
pinned by the static self-check. D4–D7 **go green on a real sink for the first time** on this ticket — until now the
rig had only the stub's nine-way classification (C1–C9), and every level beyond "cannot connect" had never been
walked on a real sink. The rehearsal rig's MySQL satisfies all three premises naturally (compose brings it up as
`utf8mb4`, and 8.0's default `max_allowed_packet` happens to be 64M) — **this is one place where the rehearsal rig is
more lenient than a real machine**, marked in the manual as real-machine difference ⑥, with item 11 of the packing
list naming the DBA note as something to bring.

**(d) Two target-only real-machine differences are added, raising the static self-check's bar to seven markers.**
Of the five classes named in spec #149 User Story 4, **root is a top-level premise** (the manual opens with
"root throughout", handled word for word as in #155's source manual — it is not a step-by-step divergence but a
whole-document assumption: root is present by default in a container and available on a real machine, so the two do
not diverge), and the other four (firewall / SELinux / packages already installed / yum repos) each carry a
`⚠ 真机差异` marker. The target side has two more that cannot be hit on the rehearsal rig and certainly will be on a
real machine, each marked: the whitelisted port and public IP are supplied by the customer (the sole external blocker
in §8's "Risk"; this end needs only the port, with `accept` bound to `0.0.0.0`), and MySQL being the customer's
database (see (c)). `test-rehearsal-target-install.sh` judges by a marker count ≥ 7 plus keywords for those six
classes (root goes to the top-level premise and is not counted, as in #155).

**(e) `D` is not a fifth rig letter** (the same ruling as addendum 4(a), cited for the fifth time): the target-side
installation criteria live in the record under `docs/install/records/` and touch no transfer semantics. The manual and
the record share one documentation area (spec #149 E.17); the rig side holds only the "walk it on the rehearsal rig"
half: `rehearsal-target-install.sh` is an executable replay of the manual (not a second way to install), and
`test-rehearsal-target-install.sh` guards "the manual and the replay say the same thing".

### 9. The final rehearsal's orchestration layer, the assertion surface of the real transfer segment, and the negative side of "the filter took effect" (2026-08-20, #157 landed)

§6's criteria are only fully walked at this ticket: addendum 7 landed the source half, addendum 8 the target half, and
this ticket runs both manuals **joined up**, plus the thing neither earlier ticket did — **one real transfer through
the tunnel** (spec #149 User Story 14). Landing it raised five things to nail down.

**(a) The orchestration layer installs nothing itself; there is only one source for how to install.**
`rehearsal-final.sh` orchestrates: tear down and rebuild → check the packing list item by item → topology criteria →
the target-side walk → the source-side walk → tunnel criteria → one real transfer. Not one installation command is in
it; the target side comes from `rehearsal-target-install.sh` (following the target manual) and the source side from
`rehearsal-source-install.sh` (following the source manual). The reason is the same as addendum 7(a)/8(a): the moment
the script types installation commands itself, the record no longer evidences the manual, and §6's "the manual did
not say it and it was solved on the spot counts as the criterion not met" loses its hold. `test-rehearsal-final.sh`
item 2 judges by content — `yum -y install` / `ldconfig` / `unzip -oq` / placeholder substitution / starting a
preflight appearing in the orchestration script all go red.

**(b) Step 10 of the target manual is deferred, so that replay gets two switches.** Those four commands are
**typed on the source host** (the manual says so), and in the final rehearsal the source is installed from its own
manual too — while the target is being installed the source is still a clean machine and cannot type them. Ruling:
`--defer-step10` installs steps 1–9 and leaves step 10 for `--only-step10` after the source is installed.
**Both switches only subtract, never add**: with no arguments that script is word for word what it was when #156
landed (addendum 8's record still holds); and **nothing unwalked is listed in the ledger** — recording an unrun step
as green or as red both disguise "not run" as a judgement, the same class as addendum 4's "a negative criterion going
red for the wrong reason".

**(c) The real transfer's assertion surface is the product's own `/api/*`, hit with `curl` inside the source-host
container.** This is the first application on the rehearsal rig of the same discipline as ADR-0028 §1 (the assertion
surface is the API, not the DOM): that path is **the same one** the owner uses when clicking the UI through
`ssh -L 8088:127.0.0.1:8088`, and the UI half belongs to the three visual walkthroughs and is not re-judged here.
Each of the six things in a completed transfer has its criterion: two datasources (the target one's connection test
goes **through the tunnel to sink, and from sink to MySQL**), the builder listing tables and fetching columns, a task
definition with one **fill-at-run-time** filter condition, starting the run, polling the phase and rows pushed, and
value-by-value verification in the target database. **The target table is created from the DDL the product
generates** (v1 creates tables by hand, ADR-0039) — the rig does not write its own `CREATE TABLE`, which would bypass
the DDL generator and leave that on-site step un-rehearsed; item 6 of the static self-check watches for it.

**(d) "The filter took effect" must have a negative side.** The fixture `acceptance/oracle-v2-final.sql` deliberately
gives its two business dates **unequal row counts** (five rows on 08-20, two on 08-19), and verification asserts not
only the row count and value-by-value equality but also **that the two rows outside the filter were not transferred**.
Counting rows alone would record the coincidence "the whole table was transferred and happens to have exactly five
rows" as green — the other face of addendum 4's "every negative criterion carries a same-address positive control":
a positive needs a negative, and a negative needs a positive control.

**(e) Item 8 of the packing list (offline rpms) is recorded as "not applicable" on the rehearsal rig, neither OK nor
missing.** The containers have network and use the three vault repos, so the offline path is the manual's
real-machine difference ②, and **it cannot be verified** on the rehearsal rig. Recording OK is a false green (those
rpms were never used), and recording it missing would block the whole run. A third state, "not applicable + why", is
the only honest record, and it is the same discipline as `CLAUDE.md` visual-gate rule 3 (if it was not run, say "not
run and why", but silence is not a skip). The other ten items are checked one by one, and one missing stops
everything on the spot — installation day has no second chance.

**(e2) "Seeing progress" cannot be shown end to end on the rehearsal rig.** The 2026-08-20 transfer was five rows,
and a one-second polling interval caught only a single `PREPARING` frame before it reached a final state. Ruling:
**the record states the one frame honestly and does not pass it off as "progress was watched"** — a long-running
in-progress state already has homes (the `hang-streaming` rig that M2's walkthrough V1/V16/V17 obtain via
`M2_KEEP_RIG`, and M1's 100k-row level), and building another at this entry point only adds a second source of truth
that will drift (ADR-0040 §1's reasoning). Seeing it end to end in the final rehearsal would require a fixture at the
100k-row scale, which is exactly what M1's level already does.

**(e3) Criterion 4, "solved on the spot must be written back and walked again", applies equally to the orchestration
script.** This ticket ran three times on 2026-08-20: the first was green throughout, then code review reported seven
issues (item 3 always OK, a create-table error swallowed, polling treating a non-200 as a final state, and others);
once the script changed, **that record no longer evidenced the version of the script in the repo** — voided and
re-walked. The second run tripped over `jq`'s `//` treating `false` as "missing" (`.live // empty` returned an empty
string at the moment of reaching a final state, so the transfer actually succeeded while the script reported "stuck");
fixed, and re-walked as the third run. Ruling: **the manual gets re-walked, and so does the orchestration script when
it changes**, with the record noting how many runs there were and why each re-run happened — the criteria are
procedural, so the reason for a re-run is itself part of the record.

**(f) This ticket opens no new rig letter** (the same ruling as addendum 4(a), cited for the sixth time): the final
rehearsal's criteria still live in the record under `docs/install/records/`, in the same documentation area as the two
manuals (spec #149 E.17). The four existing rigs (M1/M2/M3/v1) are untouched to the byte.

### 10. "Running the M2 rig" is not "M2 acceptance", and the three code gates must keep their raw output (2026-08-20, two-axis review after #158's acceptance)

Both review axes independently reported the same thing: `CLAUDE.md`'s visual-gate trigger table gives V1–V25's first
trigger as "every M2 acceptance", while every overall acceptance **re-runs the M2 rig** — read literally, V1–V25 runs
in every version, colliding head-on with ADR-0040 §6.1's "V1–V25 runs once and is sealed". #136 (v1's acceptance)
already handled it as "no collision", but that was **a precedent, not a written ruling**, so #158 proved it again.
The ruling, so it need not be proved a third time:

**(a) "M2 acceptance" means the acceptance of the M2 milestone, not any execution of the `run-m2-acceptance.sh`
script.** The latter runs in M3's, v1's and v2's overall acceptances as a **regression re-run** — proving "this
version did not break M2's criteria", not "the M2 milestone is being accepted". A milestone is accepted once in its
life (M2's was completed under #72), so this trigger **never fires again after M2**. V1–V25 is thereafter driven only
by the second trigger: any change to `docs/design-system/README.md` or `tokens.css`.

**(b) This is not an exemption from rule 1.** Rule 1 says "a trigger fires, you run it, no exemptions", and governs
what may not be second-guessed **once a trigger holds**; this clause rules on **what the trigger refers to** — a
reading, not an exemption. The test is hard: the second trigger (whether the two design-system files changed) is
**still checked item by item every version, with git evidence attached**, and if a change is found the whole of
V1–V25 runs, with no second option. What #158 found was zero commits in `e581056..d2bf782`.

**(c) W1–W6 does not take this clause.** Its two triggers (the `.precheck-reports` layout, the `DiagnosticTable`
column structure) were always "run it if it changed", with no "every such-and-such acceptance" tier, so checking git
evidence per version suffices (what #158 found: zero commits to `app.css`, and the two keywords appearing 0 times in
the whole front-end diff).

**(d) The three code gates (`cargo test --workspace` / `npm run typecheck` / `npm test`) must keep their raw output,
not just a one-line conclusion.** All four rig reports carry the raw payload of each assertion; only these three
gates had until now left just "all passed" in the acceptance ledger — the next machine cannot re-check that, and it
is the same discipline as `CLAUDE.md` rule 2 ("record actual observations, never a bare pass claim"), which had only
been written for the visual walkthroughs. Ruling: **the overall acceptance ledger attaches the three gates' raw
output** (test target names, each `test result` line, exit codes), along with which machine it ran on and when.
