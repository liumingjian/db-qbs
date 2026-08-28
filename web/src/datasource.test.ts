import { describe, expect, it } from "vitest";

import type { Agent, Datasource, Task } from "./api";
import {
  agentLabel,
  agentStatusOf,
  canSaveDatasource,
  connectionFingerprint,
  connectionSummary,
  deleteRefusalMessage,
  draftFrom,
  qualifiedTargetTable,
  referenceCounts,
} from "./datasource";
import {
  referencedDatasourcesFrom,
  referencedTasksFrom,
  ApiError,
} from "./api";

const agent: Agent = {
  agent_id: "agent-a",
  name: "目标端 A",
  base_url: "http://127.0.0.1:8080",
  instance_id: "6f1a9c2d",
  version: "0.1.0",
  last_seen_at: "2026-08-24T00:00:00Z",
  status: "online",
  last_error: null,
  mysql_version: "8.0.36",
  mysql_collation: "utf8mb4_0900_ai_ci",
};

const oracle: Datasource = {
  datasource_id: "ds-1",
  name: "生产核心库",
  kind: "oracle",
  connect_string: "//oracle:1521/ORCLPDB",
  username: "app",
  has_password: true,
};

const mysql: Datasource = {
  datasource_id: "ds-2",
  name: "数仓 MySQL",
  kind: "mysql",
  agent_id: "agent-a",
  host: "10.0.0.12",
  port: 3306,
  username: "sink",
  database: "dw_stage",
  has_password: false,
};

function taskWith(sourceId: string, targetId: string, name = "t"): Task {
  return {
    task_id: name,
    name,
    source_datasource_id: sourceId,
    target_datasource_id: targetId,
    spec: {
      owner: "SPIKE",
      table: "T",
      target_table: "T",
      write_mode: "APPEND",
      primary_key: ["ID"],
      columns: [{ source: "ID", target: "ID" }],
      where_clause: "",
    },
  };
}

describe("connectionSummary", () => {
  it("各显各的，不拼一个假的统一连接串", () => {
    expect(connectionSummary(oracle)).toBe("//oracle:1521/ORCLPDB");
    expect(connectionSummary(mysql)).toBe("10.0.0.12:3306 / dw_stage");
  });
});

describe("qualifiedTargetTable", () => {
  it("MySQL 目标端把库名补全，删除对话框里那张表因此是可核对的", () => {
    expect(qualifiedTargetTable(mysql, "ORDER_ITEM")).toBe("dw_stage.ORDER_ITEM");
  });

  it("认不出目标端时只给表名，不编一个库名出来", () => {
    expect(qualifiedTargetTable(undefined, "ORDER_ITEM")).toBe("ORDER_ITEM");
    expect(qualifiedTargetTable(oracle, "ORDER_ITEM")).toBe("ORDER_ITEM");
  });
});

describe("draftFrom", () => {
  it("编辑态口令永远是空的——界面不回读口令，连密文都不回", () => {
    expect(draftFrom(oracle).password).toBe("");
    expect(draftFrom(mysql).password).toBe("");
  });

  it("新建态默认是 Oracle 的空表单", () => {
    expect(draftFrom(null)).toEqual({
      name: "",
      kind: "oracle",
      connect_string: "",
      username: "",
      password: "",
    });
  });
});

describe("canSaveDatasource（测通才让存）", () => {
  it("没测过就不放行", () => {
    const draft = draftFrom(null);
    expect(canSaveDatasource(draft, null, null)).toBe(false);
  });

  it("测通的那组值就是表单里这组时放行", () => {
    const draft = { ...draftFrom(null), connect_string: "//a:1521/X", username: "u" };
    expect(canSaveDatasource(draft, null, connectionFingerprint(draft))).toBe(true);
  });

  it("连接字段一改，上一次的结果当场作废", () => {
    const tested = { ...draftFrom(null), connect_string: "//a:1521/X", username: "u" };
    const fingerprint = connectionFingerprint(tested);
    const changed = { ...tested, connect_string: "//b:1521/X" };
    expect(canSaveDatasource(changed, null, fingerprint)).toBe(false);
  });

  it("编辑态只改名称免测连——名称不进指纹", () => {
    const initial = draftFrom(oracle);
    const renamed = { ...initial, name: "改了个名字" };
    expect(canSaveDatasource(renamed, initial, null)).toBe(true);
  });

  it("编辑态改了口令就得重测，哪怕别的字段没动", () => {
    const initial = draftFrom(oracle);
    const changed = { ...initial, password: "新口令" };
    expect(canSaveDatasource(changed, initial, null)).toBe(false);
  });

  it("新建态没有「未改动」这条路：初值为 null 时只认指纹", () => {
    const draft = draftFrom(null);
    expect(canSaveDatasource(draft, null, connectionFingerprint(draft))).toBe(true);
    expect(canSaveDatasource(draft, null, "别的指纹")).toBe(false);
  });
});

describe("目标端 agent 绑定（ADR-0044 §6）", () => {
  it("Oracle 那一栏是空的——源库由 source 直连，这一栏对它没有含义", () => {
    expect(agentLabel(oracle, [agent])).toBe("");
    expect(agentStatusOf(oracle, [agent])).toBeNull();
  });

  it("MySQL 显示 agent 的名字与它此刻的状态", () => {
    expect(agentLabel(mysql, [agent])).toBe("目标端 A");
    expect(agentStatusOf(mysql, [agent])).toBe("online");
    expect(agentStatusOf(mysql, [{ ...agent, status: "mismatch" }])).toBe("mismatch");
  });

  it("绑的 agent 已经不在注册表里就点名说出来，并按不在线算", () => {
    // 含糊成一个 id 片段会让人以为只是显示问题；这条数据源此刻是真的不能用。
    expect(agentLabel(mysql, [])).toBe("已失效的 agent");
    expect(agentStatusOf(mysql, [])).toBe("offline");
  });

  it("换一台 agent 就得重测：agent 进指纹", () => {
    // 换 agent 等于换了一条到目标库的路，上一次的测连结果与新路没关系。
    const initial = draftFrom(mysql);
    const moved = { ...initial, agent_id: "agent-b" };
    expect(canSaveDatasource(moved, initial, connectionFingerprint(initial))).toBe(false);
    expect(connectionFingerprint(moved)).not.toBe(connectionFingerprint(initial));
  });

  it("编辑态带出原来绑的那台 agent", () => {
    expect(draftFrom(mysql)).toMatchObject({ kind: "mysql", agent_id: "agent-a" });
  });
});

describe("referenceCounts", () => {
  it("数的是任务不是绑定：一个任务两端都指着它也只算一个", () => {
    const counts = referenceCounts([
      taskWith("ds-1", "ds-1", "两端同一条"),
      taskWith("ds-1", "ds-2", "另一个"),
    ]);
    expect(counts.get("ds-1")).toBe(2);
    expect(counts.get("ds-2")).toBe(1);
  });

  it("没被引用的数据源不在计数里，空绑定不计入", () => {
    const counts = referenceCounts([taskWith("", "", "还没绑")]);
    expect(counts.size).toBe(0);
  });
});

describe("referencedTasksFrom", () => {
  it("409 报文里点名的任务原样取出来", () => {
    const error = new ApiError("数据源仍被 2 个任务引用", 409, {
      error: { message: "…", tasks: ["日销明细", "客户主数据"] },
    });
    expect(referencedTasksFrom(error)).toEqual(["日销明细", "客户主数据"]);
  });

  it("不是 409、或报文里没有 tasks 时给空数组——正文那句话本来就带着同一件事", () => {
    expect(referencedTasksFrom(new ApiError("坏了", 500, { error: { message: "x" } }))).toEqual([]);
    expect(referencedTasksFrom(new ApiError("坏了", 409, { error: { message: "x" } }))).toEqual([]);
    expect(referencedTasksFrom(new Error("网络断了"))).toEqual([]);
  });
});

describe("referencedDatasourcesFrom", () => {
  it("删 agent 被拒时，409 报文里点名的数据源原样取出来", () => {
    const error = new ApiError("这台 agent 仍被 2 条数据源引用", 409, {
      error: { message: "…", datasources: ["数仓 MySQL", "报表库"] },
    });
    expect(referencedDatasourcesFrom(error)).toEqual(["数仓 MySQL", "报表库"]);
  });

  it("两把钥匙各开各的门：tasks 那份取不出 datasources", () => {
    const error = new ApiError("x", 409, { error: { message: "…", tasks: ["甲"] } });
    expect(referencedDatasourcesFrom(error)).toEqual([]);
    expect(referencedTasksFrom(error)).toEqual(["甲"]);
  });
});

describe("deleteRefusalMessage", () => {
  it("点名的任务交给列表，红底那段话只说数量与动作——名字不列两遍", () => {
    const server =
      "数据源仍被 2 个任务引用：日销明细、客户主数据；请先改这些任务的数据源";
    const shown = deleteRefusalMessage(server, ["日销明细", "客户主数据"]);
    expect(shown).toBe("数据源仍被 2 个任务引用；请先改这些任务的数据源");
    expect(shown).not.toContain("日销明细");
    expect(shown).not.toContain("客户主数据");
  });

  it("数量取的是列表里真有几条，不是从服务端那句话里抠出来的数", () => {
    expect(deleteRefusalMessage("随便什么", ["甲", "乙", "丙"])).toBe(
      "数据源仍被 3 个任务引用；请先改这些任务的数据源",
    );
  });

  it("拿不到 tasks 时原样退回服务端那句话——那时一遍也没有比一遍都不点名强", () => {
    const server = "数据源仍被 1 个任务引用：日销明细；请先改这些任务的数据源";
    expect(deleteRefusalMessage(server, [])).toBe(server);
    expect(deleteRefusalMessage("删除失败", [])).toBe("删除失败");
  });
});
