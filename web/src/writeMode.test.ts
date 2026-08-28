import { describe, expect, it } from "vitest";

import {
  APPEND_ONLY_CONCLUSION,
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
  it("今天只有追加写一档，而它仍然是一份清单", () => {
    // 清空后导入接进来时这个数字会变，那正是本用例要提醒的地方：
    // 加一项就意味着向导里那组单选按钮多一个，语句那一行**不受影响**。
    expect(WRITE_MODES.map((entry) => entry.mode)).toEqual(["APPEND"]);
    expect(WRITE_MODES[0].label).toBe("追加写");
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
