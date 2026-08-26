import { describe, expect, it } from "vitest";

import { ApiError } from "./api";
import {
  ddlTextSegments,
  fetchedColumnType,
  mappingSuggestion,
  targetDdlFailureFrom,
} from "./m3";

describe("M3 web presentation helpers", () => {
  it("renders only the server-provided support marker beside a fetched type", () => {
    expect(fetchedColumnType({ type: "NUMBER", support: "ok" })).toBe("NUMBER");
    expect(
      fetchedColumnType({ type: "NUMBER", support: "needs_precision" }),
    ).toBe("NUMBER [待配精度]");
    expect(
      fetchedColumnType({ type: "CLOB", support: "unsupported" }),
    ).toBe("CLOB [不支持]");
    expect(fetchedColumnType({ type: "DATE", support: null })).toBe("DATE");
  });

  it("keeps sink suggestions and fills legacy rows with an action", () => {
    expect(mappingSuggestion({ suggestion: "改为 DECIMAL(12,2)" })).toBe(
      "改为 DECIMAL(12,2)",
    );
    expect(mappingSuggestion({ suggestion: "   " })).toBe("按规则修正该列");
    expect(mappingSuggestion({ suggestion: null })).toBe("按规则修正该列");
  });

  it("marks table-name and precision placeholders with the same segment kind", () => {
    expect(
      ddlTextSegments(
        "CREATE TABLE <目标表名> (amount DECIMAL(<p>,<s>) NULL);",
      ),
    ).toEqual([
      { kind: "text", value: "CREATE TABLE " },
      { kind: "placeholder", value: "<目标表名>" },
      { kind: "text", value: " (amount " },
      { kind: "placeholder", value: "DECIMAL(<p>,<s>)" },
      { kind: "text", value: " NULL);" },
    ]);
  });

  it("parses a target-DDL rejection without losing the describe list", () => {
    const columns = [
      {
        name: "N_RAW",
        type: "NUMBER",
        precision: null,
        scale: null,
        length: null,
        support: "needs_precision" as const,
      },
    ];
    const issues = [
      {
        column: "N_RAW",
        source: "NUMBER",
        message: "configured DECIMAL(66,0) is outside MySQL DECIMAL(65,30)",
      },
    ];
    const parsed = targetDdlFailureFrom(
      new ApiError("target DDL cannot be generated", 422, {
        error: {
          kind: "target_ddl",
          message: "target DDL cannot be generated",
          columns: issues,
          described_columns: columns,
        },
      }),
    );

    expect(parsed).toEqual({ columns, issues });
  });
});
