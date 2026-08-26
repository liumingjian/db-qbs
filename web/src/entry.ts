// The door in front of the task builder: the guard chain, and the datasource
// choice the whole flow is then carried out under.
//
// These are rules, not rendering, which is why they sit outside the component.
// Getting one wrong costs the user a round trip they cannot diagnose: walking
// three steps into the builder before discovering there is no datasource to
// read from, or configuring an entire task against a target agent that has been
// down the whole time and only says so when the run fails.

import type { Agent, AgentStatus, Datasource, Task } from "./api";
import { agentLabel, agentStatusOf, connectionSummary } from "./datasource";

/**
 * Which link of the chain broke. The order below is the order they are checked
 * in, and it is also the order a person has to fix them in.
 */
export type EntryGate =
  | "no-source"
  | "no-target"
  | "target-agent-offline"
  | "source-deleted"
  | "target-deleted";

/** The screen that fixes a gate. Both values are also `Page` values in `App`. */
export type EntryFix = "datasources" | "agents";

/**
 * One line in a datasource selector.
 *
 * It carries the connection summary and the agent's liveness because that is
 * the whole point of choosing here rather than inside the form: nobody should
 * have to back out to another screen to cross-check before daring to pick.
 */
export interface DatasourceOption {
  datasource_id: string;
  name: string;
  /** `connect_string` for Oracle, `host:port / database` for MySQL. */
  connection: string;
  /** The bound agent's name. Empty on Oracle, which `source` reaches directly. */
  agentName: string;
  /** Agent liveness. `null` on Oracle — the field has no meaning there. */
  agentStatus: AgentStatus | null;
}

export type EntryGuard =
  | { kind: "loading" }
  | { kind: "blocked"; gate: EntryGate }
  | {
      kind: "open";
      sources: DatasourceOption[];
      /** Offline targets stay on this list — see `selectable`. */
      targets: DatasourceOption[];
    };

/**
 * A target whose agent is not online cannot be picked, but it is still listed.
 *
 * Filtering it out would answer "why is my datasource missing?" with silence;
 * listing it with its status answers it on the spot, and the disabled continue
 * button says what to do about it.
 */
export function selectable(option: DatasourceOption): boolean {
  return option.agentStatus === null || option.agentStatus === "online";
}

/**
 * The guard chain. **First broken link wins** — one dialog names one gate.
 *
 * Reporting every failure at once reads well on a fresh install and badly
 * everywhere else: gate 3 asks about the agent bound to a target datasource, so
 * it is not even a question until gate 2 has passed.
 */
export function evaluateEntry(
  datasources: readonly Datasource[],
  agents: readonly Agent[],
  loading: boolean,
): EntryGuard {
  if (loading) {
    return { kind: "loading" };
  }
  const sources = optionsOf(datasources, agents, "oracle");
  if (sources.length === 0) {
    return { kind: "blocked", gate: "no-source" };
  }
  const targets = optionsOf(datasources, agents, "mysql");
  if (targets.length === 0) {
    return { kind: "blocked", gate: "no-target" };
  }
  if (!targets.some(selectable)) {
    return { kind: "blocked", gate: "target-agent-offline" };
  }
  return { kind: "open", sources, targets };
}

function optionsOf(
  datasources: readonly Datasource[],
  agents: readonly Agent[],
  kind: Datasource["kind"],
): DatasourceOption[] {
  return datasources
    .filter((datasource) => datasource.kind === kind)
    .map((datasource) => ({
      datasource_id: datasource.datasource_id,
      name: datasource.name,
      connection: connectionSummary(datasource),
      agentName: agentLabel(datasource, agents as Agent[]),
      agentStatus: agentStatusOf(datasource, agents as Agent[]),
    }));
}

/** Where the dialog's one button goes. */
export function gateFix(gate: EntryGate): EntryFix {
  return gate === "target-agent-offline" ? "agents" : "datasources";
}

/** What the dialog says. One gate, one sentence, naming the link that broke. */
export function gateReason(gate: EntryGate): string {
  switch (gate) {
    case "no-source":
      return "还没有 Oracle 数据源，没有可以取数的库";
    case "no-target":
      return "还没有 MySQL 数据源，没有可以写入的库";
    case "target-agent-offline":
      return "所有 MySQL 数据源的目标端 Agent 都不在线，目标库只能经它访问";
    case "source-deleted":
      return "这个任务的源端数据源已经不在了，表清单和列都读不出来";
    case "target-deleted":
      return "这个任务的目标端数据源已经不在了，字段映射无从对照";
  }
}

/**
 * The same door, for editing a saved task.
 *
 * Two links differ from the create path, both because editing usually means
 * changing one line of a filter rather than starting a run:
 *
 * - A **deleted** datasource blocks. It is not an inconvenience: without it the
 *   table list and the column dictionary cannot be read, so what opens would be
 *   an empty screen with no way to fill it.
 * - An **offline agent** does not block. Editing ends in 保存, not in a run, and
 *   turning "the target agent is down" into "you may not change one line of
 *   WHERE" is a toll with nothing on the other side of it. The step that needs
 *   the agent says so itself, and running waits for it to come back.
 */
export function evaluateEdit(
  task: Pick<Task, "source_datasource_id" | "target_datasource_id">,
  datasources: readonly Datasource[],
  agents: readonly Agent[],
  loading: boolean,
): EntryGuard {
  if (loading) {
    return { kind: "loading" };
  }
  const sources = optionsOf(datasources, agents, "oracle");
  const targets = optionsOf(datasources, agents, "mysql");
  const has = (options: DatasourceOption[], id: string) =>
    options.some((option) => option.datasource_id === id);
  if (!has(sources, task.source_datasource_id)) {
    return { kind: "blocked", gate: "source-deleted" };
  }
  if (!has(targets, task.target_datasource_id)) {
    return { kind: "blocked", gate: "target-deleted" };
  }
  return { kind: "open", sources, targets };
}

/**
 * What a selector starts on: **only ever an unambiguous answer**.
 *
 * One selectable option means there is no decision to make, and making someone
 * confirm it is the kind of click this whole door exists to remove. Two or more
 * and the field stays empty — a silent default would be picked for them.
 *
 * `current` survives a refresh of the underlying lists as long as it is still
 * selectable; an agent going offline under an already-chosen target clears it,
 * because that choice is no longer one the door would let through.
 */
export function preselect(
  options: readonly DatasourceOption[],
  current = "",
): string {
  const usable = options.filter(selectable);
  if (usable.some((option) => option.datasource_id === current)) {
    return current;
  }
  return usable.length === 1 ? usable[0].datasource_id : "";
}

/**
 * Whether the door has to be shown at all (UX review P1-10).
 *
 * The dialog exists to do two things: refuse entry with a reason, and take a
 * choice. When the gate passes **and** `preselect` answers both sides on its
 * own, it is doing neither — it is a modal whose only content is a button that
 * says 进入向导. And now that both pickers live inside the wizard, changing
 * one's mind afterwards costs nothing.
 *
 * The gate evaluation itself is untouched. It still runs, and it still stops
 * people at the door when a link in the chain is broken; it just stops
 * interrupting when it has nothing to say.
 */
export function entryNeedsDialog(guard: EntryGuard): boolean {
  if (guard.kind !== "open") {
    return true;
  }
  return preselect(guard.sources) === "" || preselect(guard.targets) === "";
}

/**
 * Why the door will not open yet, in the user's own language, or `null`.
 *
 * The offline case names the datasource **and** the agent: "the agent is
 * offline" on its own leaves the reader to work out which of the two names on
 * screen it is talking about.
 */
export function continueBlockReason(
  guard: Extract<EntryGuard, { kind: "open" }>,
  sourceDatasourceId: string,
  targetDatasourceId: string,
): string | null {
  if (
    !guard.sources.some(
      (option) => option.datasource_id === sourceDatasourceId,
    )
  ) {
    return "请选择源端数据源";
  }
  const target = guard.targets.find(
    (option) => option.datasource_id === targetDatasourceId,
  );
  if (target === undefined) {
    return "请选择目标端数据源";
  }
  if (!selectable(target)) {
    return `「${target.name}」的目标端 Agent「${target.agentName}」${
      target.agentStatus === "mismatch" ? "身份不符" : "不在线"
    }，目标库只能经它访问`;
  }
  return null;
}
