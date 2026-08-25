import { describe, expect, it } from "vitest";

import type { BuilderColumn, TargetColumn, TargetKey, Task } from "./api";
import {
  apply,
  canAdvance,
  checkIsFresh,
  confirm,
  historyDivider,
  leaving,
  openExisting,
  openNew,
  previewIsFresh,
  taskName,
  toSpec,
  view,
} from "./wizard";
import type { Applied, Change, Draft, PreviewResult, TargetCheckResult } from "./wizard";

const SOURCE = { datasource_id: "ds-oracle", name: "生产 Oracle" };
const TARGET = { datasource_id: "ds-mysql", name: "报表 MySQL" };

function sourceColumn(name: string): BuilderColumn {
  return { name, data_type: "VARCHAR2", precision: null, scale: null, length: 30, nullable: true };
}

function targetColumn(name: string, ordinal = 1): TargetColumn {
  return {
    name,
    column_type: "varchar(30)",
    data_type: "varchar",
    precision: null,
    scale: null,
    length: 30,
    datetime_precision: null,
    nullable: true,
    character_set: "utf8mb4",
    ordinal,
    default_value: null,
    extra: "",
  };
}

/** Apply a change that is expected to go through without asking. */
function done(result: Applied): Draft {
  if (result.kind !== "done") {
    throw new Error(`expected no confirmation, got: ${result.loses.lines.join(" / ")}`);
  }
  return result.draft;
}

/** Apply a change that is expected to ask first, then agree to it. */
function agreed(draft: Draft, change: Change): { draft: Draft; lines: string[] } {
  const result = apply(draft, change);
  if (result.kind !== "needs-confirm") {
    throw new Error("expected a confirmation");
  }
  return { draft: confirm(draft, result.intent), lines: result.loses.lines };
}

/**
 * A draft someone has actually worked in: a table picked by hand, columns read
 * and then touched, a primary key ticked, a filter written.
 */
function workedDraft(): Draft {
  let draft = done(apply(openNew(SOURCE, TARGET), { type: "target-table", table: "t_customer" }));
  draft = done(apply(draft, { type: "source-table", owner: "APP", table: "T_CUSTOMER" }));
  draft = done(
    apply(draft, {
      type: "source-columns-arrived",
      columns: [sourceColumn("ID"), sourceColumn("C_NAME"), sourceColumn("D_BIZ")],
    }),
  );
  draft = done(apply(draft, { type: "toggle-column", source: "D_BIZ" }));
  draft = done(apply(draft, { type: "toggle-primary-key", target: "ID" }));
  draft = done(apply(draft, { type: "where", clause: "STATUS = 1" }));
  return draft;
}

describe("the clearing rules", () => {
  it("changing the source datasource clears the source side and keeps the target table", () => {
    // 换源端等于换一个库：表清单与列字典都是上一个库的。目标表名是用户自己写的。
    const { draft } = agreed(workedDraft(), {
      type: "source-datasource",
      datasource: { datasource_id: "ds-other", name: "灾备 Oracle" },
    });
    expect(draft.spec.owner).toBe("");
    expect(draft.spec.table).toBe("");
    expect(draft.spec.columns).toEqual([]);
    expect(draft.spec.primary_key).toEqual([]);
    expect(draft.spec.where_clause).toBe("");
    expect(draft.spec.target_table).toBe("t_customer");
    expect(draft.step).toBe(1);
  });

  it("changing the dblink clears the same list but leaves the custom SQL alone", () => {
    let draft = done(apply(openNew(SOURCE, TARGET), { type: "fetch-mode", fetchMode: "sql" }));
    draft = done(apply(draft, { type: "source-sql", sql: "SELECT ID FROM APP.T" }));
    draft = confirm(draft, { type: "dblink", dblink: "POC_LINK_A" });
    expect(draft.spec.source_sql).toBe("SELECT ID FROM APP.T");
    expect(draft.spec.dblink).toBe("POC_LINK_A");
  });

  it("switching the fetch mode discards both paths' inputs", () => {
    const { draft } = agreed(workedDraft(), { type: "fetch-mode", fetchMode: "sql" });
    expect(draft.fetchMode).toBe("sql");
    expect(draft.spec.table).toBe("");
    expect(draft.spec.columns).toEqual([]);
    expect(draft.spec.where_clause).toBe("");
    expect(draft.spec.source_sql).toBe("");
  });

  it("changing the source table clears columns, primary key and filter, not the target table", () => {
    const { draft } = agreed(workedDraft(), {
      type: "source-table",
      owner: "APP",
      table: "T_ORDER",
    });
    expect(draft.spec.table).toBe("T_ORDER");
    expect(draft.spec.columns).toEqual([]);
    expect(draft.spec.primary_key).toEqual([]);
    expect(draft.spec.where_clause).toBe("");
    expect(draft.spec.target_table).toBe("t_customer");
  });

  it("editing the SQL clears the read columns; formatting it clears nothing", () => {
    // 既有不变式：`formatSql` 只动空白，结果列还是同一批——把已读的列和已勾的主键
    // 清掉等于罚人排一次版。重构不许把这一条改坏。
    let draft = done(apply(openNew(SOURCE, TARGET), { type: "fetch-mode", fetchMode: "sql" }));
    draft = done(apply(draft, { type: "target-table", table: "t_customer" }));
    draft = done(apply(draft, { type: "source-sql", sql: "SELECT ID FROM APP.T" }));
    draft = done(apply(draft, { type: "source-columns-arrived", columns: [sourceColumn("ID")] }));
    draft = done(apply(draft, { type: "toggle-primary-key", target: "ID" }));

    const formatted = done(apply(draft, { type: "format-sql", sql: "SELECT ID\nFROM APP.T" }));
    expect(formatted.spec.columns).toHaveLength(1);
    expect(formatted.spec.primary_key).toEqual(["ID"]);

    const { draft: edited } = agreed(draft, { type: "source-sql", sql: "SELECT ID, C_NAME FROM APP.T" });
    expect(edited.spec.columns).toEqual([]);
    expect(edited.spec.primary_key).toEqual([]);
  });

  it("changing the target datasource clears the target table, the mapping targets and the key", () => {
    const draft = withTargetColumns(workedDraft());
    const { draft: next } = agreed(draft, {
      type: "target-datasource",
      datasource: { datasource_id: "ds-mysql-2", name: "另一台 MySQL" },
      online: true,
    });
    expect(next.spec.target_table).toBe("");
    expect(next.spec.columns.every((mapping) => mapping.target === "")).toBe(true);
    expect(next.spec.primary_key).toEqual([]);
    expect(next.targetColumns).toEqual([]);
  });

  it("changing the target table preserves decisions and invalidates only old metadata", () => {
    // #173 的缺陷：目标表输入框每次按键都清空全部映射与主键。
    const draft = passingCheck(withTargetColumns(workedDraft()));
    const next = done(apply(draft, { type: "target-table", table: "t_customer_v2" }));
    expect(next.spec.columns).toEqual(draft.spec.columns);
    expect(next.spec.primary_key).toEqual(draft.spec.primary_key);
    expect(next.targetColumns).toEqual([]);
    expect(next.targetKeys).toEqual([]);
    expect(next.check).toBeNull();
  });

  it("going back keeps everything on the step just left", () => {
    const draft = { ...workedDraft(), step: 3 as const };
    const back = done(apply(draft, { type: "back" }));
    expect(back.step).toBe(2);
    expect(back.spec).toEqual(draft.spec);
  });
});

describe("when a change is worth asking about", () => {
  it("says nothing on an empty draft", () => {
    // 空草稿没有「换来的状态」；照字面一律确认会在头三十秒里连弹四次。
    const draft = openNew(SOURCE, TARGET);
    expect(apply(draft, { type: "source-datasource", datasource: { datasource_id: "x", name: "X" } }).kind).toBe("done");
    expect(apply(draft, { type: "fetch-mode", fetchMode: "sql" }).kind).toBe("done");
    expect(leaving(draft)).toBeNull();
  });

  it("does not ask about values the machine put there", () => {
    // 自动全选的列、同名匹配出的映射、预填的目标表名都不算「手改过」。
    let draft = done(apply(openNew(SOURCE, TARGET), { type: "source-table", owner: "APP", table: "T_CUSTOMER" }));
    draft = done(apply(draft, { type: "source-columns-arrived", columns: [sourceColumn("ID")] }));
    expect(draft.spec.columns).toHaveLength(1);
    expect(draft.spec.target_table).toBe("T_CUSTOMER");
    const result = apply(draft, { type: "fetch-mode", fetchMode: "sql" });
    if (result.kind !== "needs-confirm") {
      throw new Error("the hand-picked source table is worth asking about");
    }
    expect(result.loses.lines).toEqual(["已选的源表 APP.T_CUSTOMER"]);
  });

  it("lists exactly what a change clears, and nothing it keeps", () => {
    const result = apply(workedDraft(), {
      type: "source-datasource",
      datasource: { datasource_id: "ds-other", name: "灾备 Oracle" },
    });
    if (result.kind !== "needs-confirm") {
      throw new Error("expected a confirmation");
    }
    expect(result.loses.lines).toEqual([
      "已选的源表 APP.T_CUSTOMER",
      "已选的 2 列",
      "已勾的 1 个主键列",
      "手写的过滤条件",
    ]);
    expect(result.loses.lines.join()).not.toContain("目标表");
  });

  it("asks before deleting a column even though nothing else is cleared", () => {
    const { lines } = agreed(workedDraft(), { type: "remove-column", source: "ID" });
    expect(lines).toEqual(["已选的 2 列"]);
  });

  it("asks before a refresh only when there is hand work to overwrite", () => {
    let draft = withTargetColumns(workedDraft());
    expect(apply(draft, { type: "refresh-target-columns" }).kind).toBe("done");

    draft = done(apply(draft, { type: "rename-target", source: "C_NAME", target: "cust_name" }));
    const { draft: refreshed, lines } = agreed(draft, { type: "refresh-target-columns" });
    expect(lines).toEqual(["手动改过的 1 行字段映射"]);
    // 刷新 = 恢复自动匹配，一件简单可预期的事。
    expect(refreshed.spec.columns.find((mapping) => mapping.source === "C_NAME")?.target).toBe("C_NAME");
  });

  it("treats a saved task's contents as hand-made the moment it is opened", () => {
    // 屏幕上那些东西在人眼里跟自己刚填的没有区别；而且编辑模式清空后一保存就永久了。
    const draft = openExisting(savedTask(), SOURCE, TARGET);
    const result = apply(draft, {
      type: "source-datasource",
      datasource: { datasource_id: "ds-other", name: "灾备 Oracle" },
    });
    expect(result.kind).toBe("needs-confirm");
    expect(leaving(draft)).not.toBeNull();
  });

  it("protects leaving with the same predicate", () => {
    const loss = leaving(workedDraft());
    expect(loss?.lines).toContain("已选的 2 列");
    expect(loss?.headline).toContain("离开");
  });
});

describe("the advance gate", () => {
  it("locates a duplicate target field to both rows", () => {
    let draft = withTargetColumns(workedDraft());
    draft = done(apply(draft, { type: "rename-target", source: "C_NAME", target: "ID" }));
    const blockers = canAdvance(draft, 1).filter((blocker) => blocker.message.includes("重复"));
    expect(blockers.map((blocker) => blocker.column).sort()).toEqual(["C_NAME", "ID"]);
  });

  it("blocks on a missing primary key and says so", () => {
    let draft = done(apply(openNew(SOURCE, TARGET), { type: "target-table", table: "t" }));
    draft = done(apply(draft, { type: "source-table", owner: "APP", table: "T_CUSTOMER" }));
    draft = done(apply(draft, { type: "source-columns-arrived", columns: [sourceColumn("ID")] }));
    expect(canAdvance(draft, 1).map((blocker) => blocker.message)).toContain(
      "主键必选：至少要勾一列作为 upsert 的去重键",
    );
  });

  it("catches the target field shapes the server would reject", () => {
    // `specComplete` 曾是 `validate()` 的真子集：这几条它一条都不查，于是「保存」
    // 亮着，点下去被后端打回。
    let draft = withTargetColumns(workedDraft());
    draft = done(apply(draft, { type: "rename-target", source: "C_NAME", target: "1st_col" }));
    expect(canAdvance(draft, 1).some((blocker) => blocker.message.includes("未加引号"))).toBe(true);
  });

  it("marks a mapping the target table has no column for", () => {
    let draft = withTargetColumns(workedDraft());
    draft = done(apply(draft, { type: "rename-target", source: "C_NAME", target: "no_such" }));
    const blocker = canAdvance(draft, 1).find((entry) => entry.column === "C_NAME");
    expect(blocker?.message).toBe("目标表里没有 no_such 这一列");
    expect(view(draft, 1).step).toMatchObject({ orphans: ["C_NAME"] });
  });

  it("clears the orphaned rows in one gesture", () => {
    let draft = withTargetColumns(workedDraft());
    draft = done(apply(draft, { type: "rename-target", source: "C_NAME", target: "no_such" }));
    draft = done(apply(draft, { type: "drop-orphan-mappings" }));
    expect(draft.spec.columns.find((mapping) => mapping.source === "C_NAME")?.target).toBe("");
  });

  it("blocks step 1 until a target table is chosen", () => {
    let draft = done(apply(openNew(SOURCE, TARGET), { type: "source-table", owner: "APP", table: "" }));
    draft = { ...draft, spec: { ...draft.spec, target_table: "" }, hand: { ...draft.hand, targetTable: true } };
    expect(canAdvance(draft, 1).some((blocker) => blocker.message.includes("目标表"))).toBe(true);
  });

  it("has no completeness gate of its own on the filter step", () => {
    // 分号那条规则只有一份实现，在 Rust 侧；这里不再养第二份。
    let draft = withTargetColumns(workedDraft());
    draft = done(apply(draft, { type: "where", clause: "STATUS = 1; DROP TABLE X" }));
    expect(canAdvance(draft, 2)).toEqual([]);
  });

  it("will not accept a target-table check taken against a different column set", () => {
    let draft = passingCheck(withTargetColumns(workedDraft()));
    expect(canAdvance(draft, 3)).toEqual([]);
    draft = done(apply(draft, { type: "toggle-column", source: "D_BIZ" }));
    expect(canAdvance(draft, 3).map((blocker) => blocker.message)).toEqual(["请先运行目标表检查"]);
  });

  it("excuses the check when editing against an offline agent", () => {
    // 编辑以「保存」收尾，不跑；拦在这里等于「先去把 Agent 弄活，才准改一行 WHERE」。
    const editing = { ...openExisting(savedTask(), SOURCE, TARGET, false), step: 3 as const };
    expect(canAdvance(editing, 3)).toEqual([]);
    const creating = { ...withTargetColumns(workedDraft()), targetAgentOnline: false };
    expect(canAdvance(creating, 3)).not.toEqual([]);
  });
});

describe("staleness follows the inputs, not the clock", () => {
  it("keeps the check across a filter-clause change and drops it on a column change", () => {
    // 回退保状态：改个 WHERE 不该赔上一次目标端往返。
    let draft = passingCheck(withTargetColumns(workedDraft()));
    draft = done(apply(draft, { type: "where", clause: "STATUS = 2" }));
    expect(checkIsFresh(draft)).toBe(true);
    draft = done(apply(draft, { type: "toggle-primary-key", target: "C_NAME" }));
    expect(checkIsFresh(draft)).toBe(false);
  });

  it("drops the preview on a filter-clause change and keeps it across a target-table change", () => {
    let draft = withTargetColumns(workedDraft());
    draft = done(apply(draft, { type: "preview-arrived", preview: preview() }));
    expect(previewIsFresh(draft)).toBe(true);
    draft = done(apply(draft, { type: "target-table", table: "t_customer_v2" }));
    expect(previewIsFresh(draft)).toBe(true);
    draft = done(apply(draft, { type: "where", clause: "STATUS = 2" }));
    expect(previewIsFresh(draft)).toBe(false);
  });

  it("hides a stale result rather than showing it with a caveat", () => {
    let draft = passingCheck(withTargetColumns(workedDraft()));
    draft = done(apply(draft, { type: "toggle-column", source: "D_BIZ" }));
    expect(view(draft, 3).step).toMatchObject({ check: { state: "stale", value: null } });
  });
});

describe("derived values", () => {
  it("generates the task name and stops the moment one is typed", () => {
    let draft = withTargetColumns(workedDraft());
    expect(taskName(draft)).toBe("APP.T_CUSTOMER → t_customer");
    draft = done(apply(draft, { type: "task-name", name: "客户主档日更" }));
    draft = confirm(draft, { type: "source-table", owner: "APP", table: "T_ORDER" });
    expect(taskName(draft)).toBe("客户主档日更");
  });

  it("infers the primary key from the target table and says why it is locked", () => {
    // 任何被禁用的控件都配一句「为什么」，否则用户只会以为它坏了。
    let draft = done(apply(openNew(SOURCE, TARGET), { type: "target-table", table: "t_customer" }));
    draft = done(apply(draft, { type: "source-table", owner: "APP", table: "T_CUSTOMER" }));
    draft = done(
      apply(draft, {
        type: "source-columns-arrived",
        columns: [sourceColumn("ID"), sourceColumn("C_NAME")],
      }),
    );
    draft = withTargetColumns(draft, [{ name: "PRIMARY", columns: ["ID"] }]);
    expect(draft.spec.primary_key).toEqual(["ID"]);
    const step = view(draft, 1).step;
    if (step.step !== 1) throw new Error("expected step 1");
    expect(step.rows[0].primaryKeyLock).toBe("目标表已定义主键（ID），按它锁定");
  });

  it("does not overwrite a primary key the person ticked themselves", () => {
    // 静默改掉手勾的主键，正是这一轮要修掉的那类毛病；对不上由第 3 步的
    // 「主键定义对不上」当面说，不在背后改。
    const draft = withTargetColumns(workedDraft(), [{ name: "PRIMARY", columns: ["C_NAME"] }]);
    expect(draft.spec.primary_key).toEqual(["ID"]);
    const step = view(draft, 1).step;
    if (step.step !== 1) throw new Error("expected step 1");
    expect(step.rows[0].primaryKeyLock).toBeNull();
  });

  it("renders a same-name match read-only and everything else as a dropdown", () => {
    let draft = withTargetColumns(workedDraft());
    draft = done(apply(draft, { type: "rename-target", source: "C_NAME", target: "CUST_NAME" }));
    const step = view(draft, 1).step;
    if (step.step !== 1) throw new Error("expected step 1");
    expect(step.rows.find((row) => row.source === "ID")?.control).toBe("auto");
    expect(step.rows.find((row) => row.source === "C_NAME")?.control).toBe("manual");
  });

  it("drops a primary-key entry whose target field stops existing", () => {
    // 主键存的是目标字段名。留着一个不再产出的名字，会一路走到 sink 预检才炸成
    // 「主键列不在选中的列里」，而用户什么都没做错。
    let draft = withTargetColumns(workedDraft());
    expect(draft.spec.primary_key).toEqual(["ID"]);
    draft = done(apply(draft, { type: "rename-target", source: "ID", target: "" }));
    expect(draft.spec.primary_key).toEqual([]);
  });

  it("offers 只保存 beside 开始导入 when creating, and only 保存 when editing", () => {
    const creating = view(withTargetColumns(workedDraft()), 4).step;
    if (creating.step !== 4) throw new Error("expected step 4");
    expect(creating.confirm.actions).toEqual(["start", "save-only"]);
    const editing = view(openExisting(savedTask(), SOURCE, TARGET), 4).step;
    if (editing.step !== 4) throw new Error("expected step 4");
    expect(editing.confirm.actions).toEqual(["save"]);
  });

  it("never offers a jump on the rail", () => {
    expect(view(workedDraft(), 1).rail.every((entry) => entry.jumpable === false)).toBe(true);
  });
});

describe("what goes out", () => {
  it("keeps the two source shapes from carrying each other's fields", () => {
    let draft = done(apply(openNew(SOURCE, TARGET), { type: "fetch-mode", fetchMode: "sql" }));
    draft = done(apply(draft, { type: "source-sql", sql: "SELECT ID FROM APP.T" }));
    draft = done(apply(draft, { type: "target-table", table: "t" }));
    const spec = toSpec(draft);
    expect(spec.owner).toBe("");
    expect(spec.where_clause).toBe("");
    expect(spec.dblink).toBeUndefined();
    expect(spec.source_sql).toBe("SELECT ID FROM APP.T");
  });

  it("writes a history divider for anything that changes an outcome", () => {
    const before = savedTask();
    let draft = openExisting(before, SOURCE, TARGET);
    draft = done(apply(draft, { type: "where", clause: "STATUS = 9" }));
    expect(historyDivider(before, draft)).toBe("任务定义已变更：过滤条件");
  });

  it("writes none for a rename", () => {
    // 改任务名不影响可比性；为它切一刀只会把历史切碎，人就不看了。
    const before = savedTask();
    const draft = done(apply(openExisting(before, SOURCE, TARGET), { type: "task-name", name: "别的名字" }));
    expect(historyDivider(before, draft)).toBeNull();
  });

  it("names the source datasource when that is what moved", () => {
    const before = savedTask();
    const draft = confirm(openExisting(before, SOURCE, TARGET), {
      type: "source-datasource",
      datasource: { datasource_id: "ds-other", name: "灾备 Oracle" },
    });
    expect(historyDivider(before, draft)).toContain("源端数据源");
  });
});

describe("opening a saved task", () => {
  it("lands on the earliest step that needs attention", () => {
    const broken = savedTask();
    broken.spec.primary_key = [];
    expect(openExisting(broken, SOURCE, TARGET).step).toBe(1);
    // 一份四步都通得过的任务落在第 1 步，因为上下文在那里。
    expect(openExisting(savedTask(), SOURCE, TARGET, false).step).toBe(1);
  });
});

// ---------------------------------------------------------------------------

function withTargetColumns(draft: Draft, keys: TargetKey[] = []): Draft {
  return done(
    apply(draft, {
      type: "target-columns-arrived",
      columns: [targetColumn("ID", 1), targetColumn("C_NAME", 2), targetColumn("D_BIZ", 3)],
      keys,
    }),
  );
}

function passingCheck(draft: Draft): Draft {
  const check: TargetCheckResult = { ok: true, findings: [], suggested_ddl: null };
  return done(apply(draft, { type: "check-arrived", check }));
}

function preview(): PreviewResult {
  return { columns: ["ID"], rows: [[1]], truncated: false, elapsed_ms: 12 };
}

function savedTask(): Task {
  return {
    task_id: "task-1",
    name: "客户主档",
    source_datasource_id: SOURCE.datasource_id,
    target_datasource_id: TARGET.datasource_id,
    spec: {
      owner: "APP",
      table: "T_CUSTOMER",
      target_table: "t_customer",
      columns: [
        { source: "ID", target: "ID" },
        { source: "C_NAME", target: "C_NAME" },
      ],
      primary_key: ["ID"],
      where_clause: "STATUS = 1",
    },
  };
}
