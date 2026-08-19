import { describe, expect, it } from "vitest";

import { emptySpec } from "./api";
import type { Condition, TaskSpec } from "./api";
import {
  comparisonSymbol,
  defaultParameterName,
  defaultValueType,
  runParamsSummary,
  runtimeConditions,
  sameRunParams,
  targetFieldOf,
} from "./spec";

function condition(overrides: Partial<Condition> = {}): Condition {
  return {
    column: "D_BIZ",
    operator: "eq",
    value_type: "date",
    parameter: "d_biz",
    value_source: "runtime",
    constant: "",
    ...overrides,
  };
}

function spec(conditions: Condition[]): TaskSpec {
  return { ...emptySpec(), conditions };
}

describe("comparison operators", () => {
  it("offers exactly the three the first version supports", () => {
    // ADR-0035 §3 字面：`>` `<` `=`。>= / <= 不在第一版，别自作主张加。
    expect(["gt", "lt", "eq"].map((operator) =>
      comparisonSymbol(operator as Condition["operator"]),
    )).toEqual([">", "<", "="]);
  });
});

describe("value type prefill", () => {
  it.each([
    ["DATE", "date"],
    ["TIMESTAMP(6)", "date"],
    ["NUMBER", "number"],
    ["BINARY_DOUBLE", "number"],
    ["VARCHAR2", "text"],
    ["CLOB", "text"],
    [undefined, "text"],
  ] as const)("prefills %s as %s", (dataType, expected) => {
    expect(defaultValueType(dataType)).toBe(expected);
  });
});

describe("parameter naming", () => {
  it("defaults to the lowercased column name", () => {
    expect(defaultParameterName("D_BIZ", [])).toBe("d_biz");
  });

  it("suffixes only to avoid a collision, never renumbers existing names", () => {
    // 参数名是历史里的键：按序号自动编号会让增删一条参数把此前所有历史的键都对不上。
    expect(defaultParameterName("D_BIZ", ["d_biz"])).toBe("d_biz_2");
    expect(defaultParameterName("D_BIZ", ["d_biz", "d_biz_2"])).toBe("d_biz_3");
  });
});

describe("runtime parameters", () => {
  it("lists only the runtime-valued conditions, ordered by parameter name", () => {
    const conditions = [
      condition({ parameter: "to_date" }),
      condition({ parameter: "fixed", value_source: "constant", constant: "A" }),
      condition({ parameter: "from_date" }),
    ];

    expect(
      runtimeConditions(spec(conditions)).map((each) => each.parameter),
    ).toEqual(["from_date", "to_date"]);
  });
});

describe("run parameter sets", () => {
  it("compares two sets by name and value, order-insensitively", () => {
    expect(sameRunParams({ a: "1", b: "2" }, { b: "2", a: "1" })).toBe(true);
    expect(sameRunParams({ a: "1" }, { a: "2" })).toBe(false);
    expect(sameRunParams({ a: "1" }, { a: "1", b: "2" })).toBe(false);
    expect(sameRunParams({}, {})).toBe(true);
  });

  it("reads a set as name=value pairs sorted by name, values verbatim", () => {
    expect(runParamsSummary({ to: "2026-08-15", from: "2026-08-14" })).toBe(
      "from=2026-08-14 · to=2026-08-15",
    );
  });

  it("says so out loud when a task takes no parameters", () => {
    expect(runParamsSummary({})).toBe("—");
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
