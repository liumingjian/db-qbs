import { describe, expect, it } from "vitest";

import type { Agent, Datasource } from "./api";
import {
  continueBlockReason,
  evaluateEdit,
  evaluateEntry,
  gateFix,
  entryNeedsDialog,
  gateReason,
  preselect,
} from "./entry";

function oracle(id: string, name: string): Datasource {
  return {
    datasource_id: id,
    name,
    kind: "oracle",
    connect_string: `//host:1521/${id.toUpperCase()}`,
    username: "reader",
    has_password: true,
  };
}

function mysql(id: string, name: string, agentId: string): Datasource {
  return {
    datasource_id: id,
    name,
    kind: "mysql",
    agent_id: agentId,
    host: "10.0.0.12",
    port: 3306,
    username: "sink",
    database: "dw_stage",
    has_password: true,
  };
}

function agent(id: string, name: string, status: Agent["status"]): Agent {
  return {
    agent_id: id,
    name,
    base_url: "http://127.0.0.1:8080",
    instance_id: "6f1a9c2d",
    version: "0.1.0",
    last_seen_at: "2026-08-25T02:00:00Z",
    status,
    last_error: null,
    mysql_version: "8.0.36",
    mysql_collation: "utf8mb4_0900_ai_ci",
  };
}

const online = agent("agent-a", "目标端 A", "online");
const offline = agent("agent-b", "目标端 B", "offline");
const mismatch = agent("agent-c", "目标端 C", "mismatch");

describe("evaluateEntry", () => {
  it("reports loading before the datasource list has arrived", () => {
    expect(evaluateEntry([], [], true)).toEqual({ kind: "loading" });
  });

  it("blocks on the missing source datasource first", () => {
    expect(evaluateEntry([], [], false)).toEqual({
      kind: "blocked",
      gate: "no-source",
    });
  });

  it("blocks on the source link even when the target links are also broken", () => {
    // The chain is sequential: one dialog names one gate, and the first broken
    // link is the one the user has to fix before anything downstream matters.
    const guard = evaluateEntry([mysql("m1", "数仓", "agent-b")], [offline], false);
    expect(guard).toEqual({ kind: "blocked", gate: "no-source" });
  });

  it("blocks when no target datasource exists", () => {
    expect(evaluateEntry([oracle("o1", "核心库")], [online], false)).toEqual({
      kind: "blocked",
      gate: "no-target",
    });
  });

  it("blocks when every target's agent is offline", () => {
    const guard = evaluateEntry(
      [oracle("o1", "核心库"), mysql("m1", "数仓", "agent-b")],
      [online, offline],
      false,
    );
    expect(guard).toEqual({ kind: "blocked", gate: "target-agent-offline" });
  });

  it("counts an identity mismatch as not online", () => {
    const guard = evaluateEntry(
      [oracle("o1", "核心库"), mysql("m1", "数仓", "agent-c")],
      [mismatch],
      false,
    );
    expect(guard).toEqual({ kind: "blocked", gate: "target-agent-offline" });
  });

  it("counts a datasource bound to an unregistered agent as not online", () => {
    const guard = evaluateEntry(
      [oracle("o1", "核心库"), mysql("m1", "数仓", "agent-gone")],
      [online],
      false,
    );
    expect(guard).toEqual({ kind: "blocked", gate: "target-agent-offline" });
  });

  it("opens once one online target exists, and keeps the offline ones on the list", () => {
    const guard = evaluateEntry(
      [
        oracle("o1", "核心库"),
        mysql("m1", "数仓", "agent-a"),
        mysql("m2", "集市", "agent-b"),
      ],
      [online, offline],
      false,
    );
    expect(guard.kind).toBe("open");
    if (guard.kind !== "open") {
      return;
    }
    expect(guard.sources).toEqual([
      {
        datasource_id: "o1",
        name: "核心库",
        connection: "//host:1521/O1",
        agentName: "",
        agentStatus: null,
      },
    ]);
    expect(guard.targets).toEqual([
      {
        datasource_id: "m1",
        name: "数仓",
        connection: "10.0.0.12:3306 / dw_stage",
        agentName: "目标端 A",
        agentStatus: "online",
      },
      {
        datasource_id: "m2",
        name: "集市",
        connection: "10.0.0.12:3306 / dw_stage",
        agentName: "目标端 B",
        agentStatus: "offline",
      },
    ]);
  });
});

describe("gateFix", () => {
  it("sends both datasource gates to the datasource screen", () => {
    expect(gateFix("no-source")).toBe("datasources");
    expect(gateFix("no-target")).toBe("datasources");
  });

  it("sends the liveness gate to the target-agent screen", () => {
    expect(gateFix("target-agent-offline")).toBe("agents");
  });
});

describe("preselect", () => {
  const a = {
    datasource_id: "m1",
    name: "数仓",
    connection: "10.0.0.12:3306 / dw_stage",
    agentName: "目标端 A",
    agentStatus: "online" as const,
  };
  const b = { ...a, datasource_id: "m2", name: "集市", agentStatus: "offline" as const };
  const c = { ...a, datasource_id: "m3", name: "备用" };

  it("picks the only selectable option", () => {
    expect(preselect([a, b])).toBe("m1");
  });

  it("picks nothing when two are selectable", () => {
    expect(preselect([a, b, c])).toBe("");
  });

  it("picks nothing when none is selectable", () => {
    expect(preselect([b])).toBe("");
  });

  it("keeps a still-selectable current value instead of overriding it", () => {
    expect(preselect([a, b, c], "m3")).toBe("m3");
  });

  it("drops a current value that is no longer selectable", () => {
    expect(preselect([a, b, c], "m2")).toBe("");
  });
});

describe("continueBlockReason", () => {
  const guard = evaluateEntry(
    [
      oracle("o1", "核心库"),
      oracle("o2", "财务库"),
      mysql("m1", "数仓", "agent-a"),
      mysql("m2", "集市", "agent-b"),
    ],
    [online, offline],
    false,
  );
  if (guard.kind !== "open") {
    throw new Error("fixture must open the door");
  }

  it("asks for the source first", () => {
    expect(continueBlockReason(guard, "", "m1")).toBe("请选择源端数据源");
  });

  it("asks for the target next", () => {
    expect(continueBlockReason(guard, "o1", "")).toBe("请选择目标端数据源");
  });

  it("names the offline agent that blocks the chosen target", () => {
    expect(continueBlockReason(guard, "o1", "m2")).toBe(
      "「集市」的目标端 Agent「目标端 B」不在线，目标库只能经它访问",
    );
  });

  it("distinguishes an identity mismatch from an offline target agent", () => {
    const mismatchGuard = evaluateEntry(
      [
        oracle("o1", "核心库"),
        mysql("m1", "数仓", "agent-a"),
        mysql("m3", "错接库", "agent-c"),
      ],
      [online, mismatch],
      false,
    );
    if (mismatchGuard.kind !== "open") {
      throw new Error("fixture must open the door");
    }
    expect(continueBlockReason(mismatchGuard, "o1", "m3")).toBe(
      "「错接库」的目标端 Agent「目标端 C」身份不符，目标库只能经它访问",
    );
  });

  it("lets a live pair through", () => {
    expect(continueBlockReason(guard, "o1", "m1")).toBeNull();
  });
});

describe("evaluateEdit", () => {
  const task = { source_datasource_id: "o1", target_datasource_id: "m1" };
  const datasources = [oracle("o1", "核心库"), mysql("m1", "数仓", "agent-b")];

  it("reports loading before the lists have arrived", () => {
    expect(evaluateEdit(task, [], [], true)).toEqual({ kind: "loading" });
  });

  it("lets an offline agent through", () => {
    // Editing ends in 保存, not in a run. Blocking here would make "the target
    // agent is down" mean "you may not change one line of WHERE".
    const guard = evaluateEdit(task, datasources, [offline], false);
    expect(guard.kind).toBe("open");
  });

  it("blocks when the source datasource has been deleted", () => {
    // Not an inconvenience: without it the table list and the column dictionary
    // cannot be read, so what opens is an empty screen with no way to fill it.
    expect(evaluateEdit(task, [mysql("m1", "数仓", "agent-b")], [offline], false)).toEqual({
      kind: "blocked",
      gate: "source-deleted",
    });
  });

  it("blocks when the target datasource has been deleted", () => {
    expect(evaluateEdit(task, [oracle("o1", "核心库")], [offline], false)).toEqual({
      kind: "blocked",
      gate: "target-deleted",
    });
  });

  it("names the source link first when both are gone", () => {
    expect(evaluateEdit(task, [], [], false)).toEqual({
      kind: "blocked",
      gate: "source-deleted",
    });
  });

  it("sends both deletions to the datasource screen", () => {
    expect(gateFix("source-deleted")).toBe("datasources");
    expect(gateFix("target-deleted")).toBe("datasources");
  });

  it("gives every gate a sentence that names the link that broke", () => {
    const gates = [
      "no-source",
      "no-target",
      "target-agent-offline",
      "source-deleted",
      "target-deleted",
    ] as const;
    for (const gate of gates) {
      expect(gateReason(gate).length).toBeGreaterThan(0);
    }
    expect(gateReason("source-deleted")).toContain("源端数据源");
  });
});

describe("entryNeedsDialog", () => {
  const source = oracle("ds-oracle", "生产 Oracle");
  const target = mysql("ds-mysql", "报表 MySQL", "agent-a");
  const online = agent("agent-a", "目标端 A", "online");

  it("does not interrupt when the gate passes and there is nothing to choose", () => {
    const guard = evaluateEntry([source, target], [online], false);
    expect(guard.kind).toBe("open");
    expect(entryNeedsDialog(guard)).toBe(false);
  });

  it("still asks when there is a real choice on either side", () => {
    const guard = evaluateEntry(
      [source, oracle("ds-oracle-2", "灾备 Oracle"), target],
      [online],
      false,
    );
    expect(entryNeedsDialog(guard)).toBe(true);
  });

  it("always shows the door when it is refusing entry", () => {
    expect(entryNeedsDialog(evaluateEntry([], [], false))).toBe(true);
    expect(entryNeedsDialog({ kind: "loading" })).toBe(true);
  });
});
