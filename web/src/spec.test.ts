import { describe, expect, it } from "vitest";

import { emptySpec } from "./api";
import type { TaskSpec } from "./api";
import {
  matchSameNameTargets,
  renameTargetField,
  sourceSummary,
  targetFieldOf,
  whereSummary,
} from "./spec";

describe("where clause summary", () => {
  it("gives the text back verbatim, only collapsing whitespace to one line", () => {
    // 这一格的全部价值是它是那段原文：不重排、不补 WHERE、不改一个字符。
    expect(
      whereSummary({
        where_clause: "  D_BIZ >= DATE '2026-08-01'\n   AND STATUS IN ('OK')  ",
      }),
    ).toBe("D_BIZ >= DATE '2026-08-01' AND STATUS IN ('OK')");
  });

  it("says 整表 when nothing was written, not a blank cell", () => {
    // 空白读起来像「这里应该有点什么但没渲染」。
    expect(whereSummary({ ...emptySpec(), where_clause: "" })).toBe("整表");
    expect(whereSummary({ where_clause: "  \n " })).toBe("整表");
  });

  it("survives the field being absent altogether", () => {
    // 服务端那边是 `Option<String>`，`None` 时整个字段不序列化——直接调 API 建的任务
    // 回来就是这样。少一个 `?? ""` 这里就是一次 `undefined.trim()`。
    expect(whereSummary({})).toBe("整表");
  });
});

describe("column mapping lookup", () => {
  const columns = [
    { source: "C_NAME", target: "CUST_NAME" },
    { source: "D_BIZ", target: "D_BIZ" },
  ];

  it("answers with the target field, not the source column", () => {
    // 界面上一行是源列、主键存的是目标字段：这一层换不掉，改过名的列上省掉它就会错。
    expect(targetFieldOf(columns, "C_NAME")).toBe("CUST_NAME");
    expect(targetFieldOf(columns, "D_BIZ")).toBe("D_BIZ");
  });

  it("says undefined for a column that is not selected", () => {
    expect(targetFieldOf(columns, "N_VA_PRICE")).toBeUndefined();
    expect(targetFieldOf([], "C_NAME")).toBeUndefined();
    // 目标名不是入口：拿目标字段去查也查不出来。
    expect(targetFieldOf(columns, "CUST_NAME")).toBeUndefined();
  });
});

describe("renameTargetField（改目标名时主键跟着走）", () => {
  const spec = {
    columns: [
      { source: "ID", target: "ID" },
      { source: "C_NAME", target: "C_NAME" },
      { source: "LOAD_DATE", target: "LOAD_DATE" },
    ],
    primary_key: ["ID", "C_NAME"],
  };

  it("改的那一列同时改掉映射与主键，顺序原样保留", () => {
    const next = renameTargetField(spec, "C_NAME", "CUST_NAME");
    expect(next.columns).toEqual([
      { source: "ID", target: "ID" },
      { source: "C_NAME", target: "CUST_NAME" },
      { source: "LOAD_DATE", target: "LOAD_DATE" },
    ]);
    // 复合键的列序是有意义的：CUST_NAME 仍排在 ID 之后。
    expect(next.primary_key).toEqual(["ID", "CUST_NAME"]);
  });

  it("改的列不在主键里时主键一个字不动", () => {
    const next = renameTargetField(spec, "LOAD_DATE", "BIZ_DATE");
    expect(next.primary_key).toEqual(["ID", "C_NAME"]);
    expect(next.columns[2]).toEqual({ source: "LOAD_DATE", target: "BIZ_DATE" });
  });

  it("改成一个已经在主键里的名字时不留重复项", () => {
    const next = renameTargetField(spec, "C_NAME", "ID");
    expect(next.primary_key).toEqual(["ID"]);
  });

  it("没选中的源列原样返回，不凭空造一条映射", () => {
    expect(renameTargetField(spec, "NOT_SELECTED", "X")).toBe(spec);
  });

  it("改成同一个名字是空操作", () => {
    expect(renameTargetField(spec, "ID", "ID")).toBe(spec);
  });

  it("清空目标名不会留下「界面勾着、规格里是旧名」的中间态", () => {
    const next = renameTargetField(spec, "ID", "");
    expect(next.columns[0]).toEqual({ source: "ID", target: "" });
    // 没有目标字段就不能再作主键；留下空字符串只会把错误拖到提交时才爆。
    expect(next.primary_key).toEqual(["C_NAME"]);
  });
});

describe("source summary", () => {
  it("reads a table-mode spec as owner.table", () => {
    const summary = sourceSummary({ ...emptySpec(), owner: "APP", table: "T_HOLDING" });
    expect(summary).toEqual({
      kind: "table",
      label: "APP.T_HOLDING",
      full: "APP.T_HOLDING",
    });
  });

  it("never renders a bare dot for a custom-SQL spec", () => {
    // 自定义 SQL 的规格里 owner / table 都是空串。作业中心那一列原来直接拼
    // `owner.table`，于是渲染成一个孤零零的 `.`——这条用例守住它不再回来。
    const summary = sourceSummary({
      ...emptySpec(),
      owner: "",
      table: "",
      source_sql: "SELECT *\n  FROM APP.T_HOLDING@POC_LINK_A",
    });
    expect(summary.kind).toBe("sql");
    expect(summary.label).not.toBe(".");
    expect(summary.label).toBe("SELECT * FROM APP.T_HOLDING@POC_LINK_A");
  });

  it("keeps the whole statement in `full` and truncates only the label", () => {
    const source_sql =
      "SELECT ID, C_NAME, LOAD_DATE, N_AMT, STATUS FROM APP.T_HOLDING@POC_LINK_A WHERE STATUS = 1";
    const summary = sourceSummary({ ...emptySpec(), source_sql });
    expect(summary.full).toBe(source_sql);
    expect(summary.label.endsWith("\u2026")).toBe(true);
    expect(summary.label.length).toBeLessThan(source_sql.length);
  });

  it("treats a blank source_sql as table mode", () => {
    const summary = sourceSummary({
      ...emptySpec(),
      owner: "APP",
      table: "T_HOLDING",
      source_sql: "   ",
    });
    expect(summary.kind).toBe("table");
  });
});

describe("matchSameNameTargets（同名接线，两个调用点共用）", () => {
  const targets = [{ name: "ID" }, { name: "c_name" }, { name: "OTHER" }];

  function draft(columns: TaskSpec["columns"], primary_key: string[] = []) {
    return { columns, primary_key };
  }

  it("只补空位时不碰用户已经改过的映射", () => {
    const next = matchSameNameTargets(
      draft([
        { source: "ID", target: "" },
        { source: "C_NAME", target: "CUSTOMER_NAME" },
      ]),
      targets,
      { onlyUnmapped: true },
    );
    expect(next.columns).toEqual([
      { source: "ID", target: "ID" },
      // 用户手改成 CUSTOMER_NAME，同名的 c_name 不许把它冲掉。
      { source: "C_NAME", target: "CUSTOMER_NAME" },
    ]);
  });

  it("显式填充时覆盖已有映射", () => {
    const next = matchSameNameTargets(
      draft([{ source: "C_NAME", target: "CUSTOMER_NAME" }]),
      targets,
      { onlyUnmapped: false },
    );
    expect(next.columns).toEqual([{ source: "C_NAME", target: "c_name" }]);
  });

  it("大小写不敏感地匹配，但落的是目标端原样的大小写（ADR-0038 §8）", () => {
    const next = matchSameNameTargets(
      draft([{ source: "C_NAME", target: "" }]),
      targets,
      { onlyUnmapped: true },
    );
    expect(next.columns).toEqual([{ source: "C_NAME", target: "c_name" }]);
  });

  it("目标端没有同名列就留着不动", () => {
    const next = matchSameNameTargets(
      draft([{ source: "NOT_THERE", target: "" }]),
      targets,
      { onlyUnmapped: true },
    );
    expect(next.columns).toEqual([{ source: "NOT_THERE", target: "" }]);
  });

  it("改了目标名的列，主键跟着走", () => {
    const next = matchSameNameTargets(
      draft([{ source: "C_NAME", target: "TMP" }], ["TMP"]),
      targets,
      { onlyUnmapped: false },
    );
    expect(next.primary_key).toEqual(["c_name"]);
  });
});
