import { describe, expect, it } from "vitest";

import type { BuilderColumn, TargetColumn, TargetKey, Task } from "./api";
import {
  apply,
  canAdvance,
  checkIsFresh,
  confirm,
  foldedSteps,
  historyDivider,
  leaving,
  leavingConfirmation,
  openExisting,
  openNew,
  previewIsFresh,
  taskName,
  toSpec,
  view,
} from "./wizard";
import type { PreviewResult, TargetCheckResult } from "./api";
import type { Applied, Change, Draft } from "./wizard";

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

  it("does not ask or clear when a destructive selector keeps its current value", () => {
    const draft = workedDraft();
    const source = apply(draft, { type: "source-datasource", datasource: draft.source });
    const target = apply(draft, {
      type: "target-datasource",
      datasource: draft.target,
      online: draft.targetAgentOnline,
    });
    const mode = apply(draft, { type: "fetch-mode", fetchMode: draft.fetchMode });

    expect(source).toEqual({ kind: "done", draft });
    expect(target).toEqual({ kind: "done", draft });
    expect(mode).toEqual({ kind: "done", draft });
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
    const { draft, lines } = agreed(workedDraft(), { type: "remove-column", source: "ID" });
    expect(lines).toEqual(["源列 ID 将不再参与同步"]);
    expect(draft.sourceColumns.map((column) => column.name)).not.toContain("ID");
    expect(draft.spec.columns.map((column) => column.source)).not.toContain("ID");
  });

  it("asks before deleting a machine-selected column", () => {
    let draft = done(apply(openNew(SOURCE, TARGET), { type: "source-table", owner: "APP", table: "T_CUSTOMER" }));
    draft = done(apply(draft, { type: "source-columns-arrived", columns: [sourceColumn("ID")] }));
    expect(apply(draft, { type: "remove-column", source: "ID" })).toMatchObject({
      kind: "needs-confirm",
      loses: { headline: "确认删除这一列？" },
    });
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
  it("opens on a checklist, not on three red errors", () => {
    // 刚进向导时什么都还没填，于是三条阻塞同时成立。把它们和「目标字段重复」摆成
    // 同一种红色告警，等于开屏就宣布出了三件事——而人一步都还没走（UX 评审 P1-2）。
    const fresh = openNew(SOURCE, TARGET);
    const blockers = canAdvance(fresh, 1);
    expect(blockers.length).toBeGreaterThan(0);
    expect(blockers.every((blocker) => blocker.kind === "todo")).toBe(true);
  });

  it("still calls a real conflict an error", () => {
    let draft = withTargetColumns(workedDraft());
    draft = done(apply(draft, { type: "rename-target", source: "C_NAME", target: "ID" }));
    const errors = canAdvance(draft, 1).filter((blocker) => blocker.kind === "error");
    expect(errors.map((blocker) => blocker.message).join(" ")).toContain("重复");
  });


  it("locates a duplicate target field to both rows", () => {
    let draft = withTargetColumns(workedDraft());
    draft = done(apply(draft, { type: "rename-target", source: "C_NAME", target: "ID" }));
    const blockers = canAdvance(draft, 1).filter((blocker) => blocker.message.includes("重复"));
    expect(blockers.map((blocker) => blocker.column).sort()).toEqual(["C_NAME", "ID"]);
    const step = view(draft, 1).step;
    if (step.step !== 1) throw new Error("expected step 1");
    expect(step.rows.filter((row) => row.problem?.includes("目标字段 ID 重复")).map((row) => row.source).sort()).toEqual([
      "C_NAME",
      "ID",
    ]);
  });

  it("blocks on a missing primary key and says so", () => {
    let draft = done(apply(openNew(SOURCE, TARGET), { type: "target-table", table: "t" }));
    draft = done(apply(draft, { type: "source-table", owner: "APP", table: "T_CUSTOMER" }));
    draft = done(apply(draft, { type: "source-columns-arrived", columns: [sourceColumn("ID")] }));
    expect(canAdvance(draft, 1)).toContainEqual({
      step: 1,
      kind: "error",
      column: null,
      message: "主键必选：至少要勾一列作为 upsert 的去重键",
    });
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

  it("blocks step 3 when the fresh server check reports a finding", () => {
    let draft = withTargetColumns(workedDraft());
    draft = done(apply(draft, {
      type: "check-arrived",
      check: {
        ok: false,
        findings: [{
          column: "C_NAME",
          kind: "insufficient_length_or_precision",
          expected: "VARCHAR(90)",
          actual: "varchar(30)",
          message: "目标 VARCHAR 长度不足",
        }],
        suggested_ddl: "CREATE TABLE `t_customer` (...) ",
      },
    }));

    expect(canAdvance(draft, 3)).toEqual([{
      step: 3,
      kind: "error",
      column: null,
      message: "目标表检查未通过（1 项）",
    }]);
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
  it.each(["table", "sql"] as const)("selects every returned column in %s mode", (fetchMode) => {
    let draft = openNew(SOURCE, TARGET);
    if (fetchMode === "sql") {
      draft = done(apply(draft, { type: "fetch-mode", fetchMode }));
    }
    draft = done(apply(draft, {
      type: "source-columns-arrived",
      columns: [sourceColumn("ID"), sourceColumn("C_NAME")],
    }));
    expect(draft.spec.columns).toEqual([
      { source: "ID", target: "ID" },
      { source: "C_NAME", target: "C_NAME" },
    ]);
  });

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

  it("settles target names without caring about case", () => {
    let draft = done(apply(openNew(SOURCE, TARGET), {
      type: "source-columns-arrived",
      columns: [sourceColumn("ID"), sourceColumn("C_NAME")],
    }));
    draft = done(apply(draft, {
      type: "target-columns-arrived",
      columns: [targetColumn("id"), targetColumn("c_name", 2)],
      keys: [],
    }));
    const step = view(draft, 1).step;
    if (step.step !== 1) throw new Error("expected step 1");
    expect(step.rows.find((row) => row.source === "ID")).toMatchObject({
      target: "ID",
      control: "auto",
    });
  });

  it("leaves the primary key unlocked when not every target key column is mapped", () => {
    let draft = done(apply(openNew(SOURCE, TARGET), { type: "target-table", table: "t_customer" }));
    draft = done(apply(draft, { type: "source-columns-arrived", columns: [sourceColumn("ID")] }));
    draft = done(apply(draft, {
      type: "target-columns-arrived",
      columns: [targetColumn("ID")],
      keys: [{ name: "PRIMARY", columns: ["ID", "TENANT_ID"] }],
    }));
    const step = view(draft, 1).step;
    if (step.step !== 1) throw new Error("expected step 1");
    expect(draft.spec.primary_key).toEqual([]);
    expect(step.rows[0].primaryKeyLock).toBeNull();
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

  it("does not report a check that never ran as 已通过", () => {
    // 编辑态 + 目标端 agent 离线是 `canAdvance` 明确放行的一条路（不放行等于「先去把
    // agent 救活才准改一行 WHERE」）。于是最后一屏会在**一次检查都没跑过**的情况下
    // 被读到——原来它照样写「已通过」，因为空的 findings 与通过的 findings 长得一样。
    const offline = view(openExisting(savedTask(), SOURCE, TARGET, false), 4).step;
    if (offline.step !== 4) throw new Error("expected step 4");
    expect(offline.confirm.targetCheck.state).toBe("unchecked");
    expect(offline.confirm.targetCheck.excused).toContain("不在线");
  });

  it("reports a fresh passing check as passed, with no excuse attached", () => {
    const checked = passingCheck(withTargetColumns(workedDraft()));
    const step = view(checked, 4).step;
    if (step.step !== 4) throw new Error("expected step 4");
    expect(step.confirm.targetCheck).toEqual({
      state: "passed",
      findings: [],
      excused: null,
    });
  });

  it("goes back to 尚未检查 once the check the mapping changed under it goes stale", () => {
    let draft = passingCheck(withTargetColumns(workedDraft()));
    expect(view(draft, 4).step).toMatchObject({
      confirm: { targetCheck: { state: "passed" } },
    });
    draft = done(apply(draft, { type: "toggle-column", source: "C_NAME" }));
    const step = view(draft, 4).step;
    if (step.step !== 4) throw new Error("expected step 4");
    expect(step.confirm.targetCheck.state).toBe("unchecked");
  });

  it("treats a target table that does not exist yet as one made of the source columns", () => {
    // `/api/target/columns` 对不存在的表回空清单而不是错误（ADR-0038 §9）。
    // 原来这里等于死路：目标列一个都没有，映射那一列的下拉是空的，谁也走不下去。
    let draft = done(apply(workedDraft(), { type: "target-table", table: "t_brand_new" }));
    draft = done(apply(draft, { type: "target-columns-arrived", columns: [], keys: [] }));
    expect(draft.targetTableExists).toBe(false);
    expect(draft.spec.columns).toEqual([
      { source: "ID", target: "ID" },
      { source: "C_NAME", target: "C_NAME" },
    ]);
    const step = view(draft, 1).step;
    if (step.step !== 1) throw new Error("expected step 1");
    expect(step.rows.every((row) => row.control === "new")).toBe(true);
  });

  it("goes back to a known table the moment the target table changes", () => {
    let draft = done(apply(workedDraft(), { type: "target-table", table: "t_brand_new" }));
    draft = done(apply(draft, { type: "target-columns-arrived", columns: [], keys: [] }));
    expect(draft.targetTableExists).toBe(false);
    draft = done(apply(draft, { type: "target-table", table: "t_customer" }));
    expect(draft.targetTableExists).toBe(true);
  });

  it("walks past the target-table check when it has nothing to say", () => {
    // 第 3 步在检查通过时是一屏一句话，而检查在第 1 步做完就自动跑了（UX 评审 P1-7）。
    const passed = passingCheck(withTargetColumns(workedDraft()));
    const at2 = { ...passed, step: 2 as const };
    expect(done(apply(at2, { type: "advance" })).step).toBe(4);
    expect(done(apply({ ...passed, step: 4 as const }, { type: "back" })).step).toBe(2);
  });

  it("stops at the check when it has something to say", () => {
    let draft = withTargetColumns(workedDraft());
    draft = done(apply(draft, {
      type: "check-arrived",
      check: {
        ok: false,
        findings: [{
          column: "C_NAME",
          kind: "insufficient_length_or_precision",
          expected: "VARCHAR(90)",
          actual: "varchar(30)",
          message: "目标 VARCHAR 长度不足",
        }],
        suggested_ddl: null,
      },
    }));
    expect(done(apply({ ...draft, step: 2 as const }, { type: "advance" })).step).toBe(3);
  });

  it("stops at the check when it has not run", () => {
    const unchecked = { ...withTargetColumns(workedDraft()), step: 2 as const };
    expect(done(apply(unchecked, { type: "advance" })).step).toBe(3);
  });

  it("marks a folded check step done on the rail, not skipped over silently", () => {
    const passed = passingCheck(withTargetColumns(workedDraft()));
    const rail = view({ ...passed, step: 4 }, 4).rail;
    expect(rail.find((entry) => entry.step === 3)?.state).toBe("done");
  });

  it("never offers a jump on the rail", () => {
    expect(view(workedDraft(), 1).rail.every((entry) => entry.jumpable === false)).toBe(true);
  });
});

describe("leaving the wizard", () => {
  it("says the draft is kept, because it now is", () => {
    // 草稿离开时写进 sessionStorage（UX 评审 P1-5），所以「离开会清掉」这句话不再成立。
    const loss = leaving(workedDraft());
    expect(loss).not.toBeNull();
    expect(loss!.headline).toContain("留着");
    expect(loss!.headline).not.toContain("清掉");
  });

  it("has nothing to list on a draft with nothing hand-made in it", () => {
    expect(leaving(openNew(SOURCE, TARGET))).toBeNull();
  });

  it("still asks when it has nothing to list", () => {
    // 列不出东西不是放行的理由：「里面有没有值得留的东西」是人自己的判断（#242）。
    const question = leavingConfirmation(openNew(SOURCE, TARGET));
    expect(question.headline).toBe("要离开这个向导吗？");
    expect(question.lines).toEqual([]);
    // 列得出来的时候还是列，问句不换。
    expect(leavingConfirmation(workedDraft())).toEqual(leaving(workedDraft()));
  });
});

describe("what the wizard folded past", () => {
  it("counts only the steps it actually skipped", () => {
    const passed = passingCheck(withTargetColumns(workedDraft()));
    expect(foldedSteps(passed, 2, 4)).toEqual([3]);
    // 走过去的那一步不是跳过的。
    expect(foldedSteps(passed, 2, 3)).toEqual([]);
    expect(foldedSteps(passed, 1, 2)).toEqual([]);
  });

  it("counts nothing when the step it passed still has something to say", () => {
    // 第 3 步没折，人是从 2 一路走到 4 的——中间隔着一步不等于跳过了它。
    const unchecked = withTargetColumns(workedDraft());
    expect(foldedSteps(unchecked, 2, 4)).toEqual([]);
  });

  it("counts nothing when going back", () => {
    // 往回走从来不是跳过：说「已跳过」会把一件没发生的事念给读屏的人听。
    const passed = passingCheck(withTargetColumns(workedDraft()));
    expect(foldedSteps(passed, 4, 2)).toEqual([]);
    expect(foldedSteps(passed, 3, 3)).toEqual([]);
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
  it("opens ordinary editing at the mapping step", () => {
    expect(openExisting(savedTask(), SOURCE, TARGET).step).toBe(1);
  });

  it("honours a remediation step when its prerequisites pass", () => {
    expect(openExisting(savedTask(), SOURCE, TARGET, true, 3).step).toBe(3);
  });

  it("falls back to the earliest failed prerequisite", () => {
    const broken = savedTask();
    broken.spec.primary_key = [];
    expect(openExisting(broken, SOURCE, TARGET, true, 3).step).toBe(1);
  });

  it("stops a confirmation-page request at the missing target check", () => {
    expect(openExisting(savedTask(), SOURCE, TARGET, true, 4).step).toBe(3);
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
  return { columns: ["ID"], rows: [["1"]], truncated: false, elapsed_ms: 12 };
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
