import { describe, expect, it } from "vitest";

// Rust 那份的原文，`?raw` 读进来（见 `globals.d.ts`）。
import precheckSource from "../../crates/sink/src/precheck.rs?raw";

import type { RunEvidence, TaskSpec } from "./api";
import {
  APPEND_ONLY_CONCLUSION,
  clearsTarget,
  runWriteSemantics,
  runWriteView,
  targetHasUniqueKey,
  writeSemanticsDone,
  writeSemanticsNote,
  writeStatementLabel,
  writeStatementOf,
  WRITE_MODES,
} from "./writeMode";

describe("writeStatementOf", () => {
  it("每一条都只看任务定义记下的主键，别的什么都不看", () => {
    expect(writeStatementOf(["ID"])).toBe("upsert");
    expect(writeStatementOf(["C_FUND", "D_BIZ"])).toBe("upsert");
    // 空不是「还没填」，它是一个有含义的值。
    expect(writeStatementOf([])).toBe("insert");
  });
});

describe("写入模式清单", () => {
  it("两档：追加写与先清空再导入", () => {
    expect(WRITE_MODES.map((entry) => entry.mode)).toEqual([
      "APPEND",
      "CLEAR_THEN_IMPORT",
    ]);
    expect(WRITE_MODES[0].label).toBe("追加写");
    expect(WRITE_MODES[1].label).toBe("先清空再导入");
  });

  it("会不会清空只有一处判据", () => {
    expect(clearsTarget("APPEND")).toBe(false);
    expect(clearsTarget("CLEAR_THEN_IMPORT")).toBe(true);
  });

  // 这是 #264 里最容易被写错的一条：清空看起来像是「另一种写法」，其实不是。
  it("清空不改变语句的选择——语句只看主键，模式一个字都不参与", () => {
    expect(writeStatementOf(["ID"])).toBe("upsert");
    expect(writeStatementOf([])).toBe("insert");
    // 上面两行没有模式参数可传，这正是本用例要说的事：派生函数根本不接受模式。
    expect(writeStatementOf.length).toBe(1);
  });
});

describe("写入语义那句话", () => {
  it("两种写法各说各的，因为它们各有各的坑", () => {
    const upsert = writeSemanticsNote("upsert");
    const insert = writeSemanticsNote("insert");
    expect(upsert).not.toBe(insert);
    // upsert 的坑：源端删掉的行不会跟着消失。
    expect(upsert).toContain("源端删除的行");
    // 纯追加的坑：重跑翻倍。这一句必须一字不差地出现——目标端预检也说的是它。
    expect(insert).toContain(APPEND_ONLY_CONCLUSION);
  });

  it("跑完之后的说法同样分叉，不能拿 upsert 那句话套无主键的运行", () => {
    expect(writeSemanticsDone("upsert")).toContain("按主键 upsert");
    expect(writeSemanticsDone("insert")).toContain("再追加一份");
    expect(writeSemanticsDone("insert")).not.toContain("按主键 upsert");
  });

  it("清空模式下那两个坑都不存在，换来的是另一笔代价，而它必须说满", () => {
    const cleared = writeSemanticsNote("upsert", "CLEAR_THEN_IMPORT");
    // 「源端删掉的行留着」正是清空模式来解决的，所以那句话不能出现在这里。
    expect(cleared).not.toContain("源端删除的行不会跟着消失");
    expect(cleared).toContain("精确等于本次查询结果");
    // 撤销已整体移除（#256），而清空模式是最容易让人去找撤销的那一档。
    expect(cleared).toContain("不可恢复");
    expect(cleared).toContain("没有撤销入口");
    // 语句仍由主键决定，这句交底自己就要说出来。
    expect(cleared).toContain("按主键 upsert");
    expect(writeSemanticsNote("insert", "CLEAR_THEN_IMPORT")).toContain("纯追加写");
  });

  it("跑完之后，整表替换与按主键合并各说各的", () => {
    const done = writeSemanticsDone("upsert", "CLEAR_THEN_IMPORT");
    expect(done).toContain("整表替换");
    expect(done).not.toContain("源端删除的行仍保留");
    expect(done).not.toBe(writeSemanticsDone("upsert"));
  });

  it("短标签是给一格里放的，长句子是给交底用的", () => {
    expect(writeStatementLabel("upsert")).toBe("按主键 upsert");
    expect(writeStatementLabel("insert")).toBe("纯追加写");
  });
});

describe("targetHasUniqueKey", () => {
  it("认的是唯一约束，不只是 PRIMARY——一条 UNIQUE 同样会让纯 INSERT 撞 1062", () => {
    expect(targetHasUniqueKey([])).toBe(false);
    expect(targetHasUniqueKey([{ name: "PRIMARY", columns: ["ID"] }])).toBe(true);
    expect(targetHasUniqueKey([{ name: "uk_code", columns: ["CODE"] }])).toBe(true);
  });
});

describe("运行详情说的是当时那一次", () => {
  const spec: TaskSpec = {
    owner: "APP",
    table: "T_CUSTOMER",
    target_table: "t_customer",
    write_mode: "CLEAR_THEN_IMPORT",
    schedule_enabled: false,
    primary_key: [],
    columns: [{ source: "ID", target: "ID" }],
  };
  const evidence: RunEvidence = {
    parameters: {
      target_table: "t_customer",
      columns: [{ source: "ID", target: "ID" }],
      primary_key: ["ID"],
      write_mode: "APPEND",
      source_sql: "SELECT 1 FROM DUAL",
    },
  };

  it("读的是那一行上的快照，任务后来改成什么样都不改写过去", () => {
    // 这一次跑的时候有主键、是追加写；今天任务的主键被清空、改成了先清空再导入。
    expect(runWriteView(evidence, spec)).toEqual({ statement: "upsert", mode: "APPEND" });
    expect(runWriteSemantics(evidence, spec)).toBe(
      writeSemanticsDone("upsert", "APPEND"),
    );
  });

  it("早于 #264 的历史行没有 write_mode，按当时唯一的那一档读", () => {
    const older: RunEvidence = {
      parameters: { ...evidence.parameters!, write_mode: undefined },
    };
    expect(runWriteView(older, spec).mode).toBe("APPEND");
  });

  it("整份运行证据都缺席的老记录才回退到当前定义——没有快照可读时它是唯一比空白好的答案", () => {
    expect(runWriteView(undefined, spec)).toEqual({
      statement: "insert",
      mode: "CLEAR_THEN_IMPORT",
    });
    expect(runWriteView({}, spec).statement).toBe("insert");
  });
});

describe("与目标端预检的那句结论逐字一致", () => {
  it("Rust 那份改了一个字，这条用例就红", () => {
    // 同一句话必须在两端各有一份（见 `APPEND_ONLY_CONCLUSION` 的注释：说这句话的
    // 地方大多没有服务端的回答可读）。两份一致过去只由一行注释守着，而注释拦不住
    // 任何人改一个字——这里把 Rust 那份读出来直接比。
    const matched = precheckSource.match(
      /pub const APPEND_ONLY_CONCLUSION: &str =\s*"([^"]*)";/,
    );
    expect(matched, "precheck.rs 里应当有 APPEND_ONLY_CONCLUSION").not.toBeNull();
    expect(matched![1]).toBe(APPEND_ONLY_CONCLUSION);
  });
});
