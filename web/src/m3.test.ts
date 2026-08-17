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
        name: "PAYLOAD",
        type: "CLOB",
        precision: null,
        scale: null,
        length: null,
        support: "unsupported" as const,
      },
    ];
    const issues = [
      {
        column: "PAYLOAD",
        source: "CLOB",
        message: "source type is outside the target DDL whitelist",
      },
    ];
    const parsed = targetDdlFailureFrom(
      new ApiError("target DDL cannot be generated", 422, {
        kind: "target_ddl",
        columns: issues,
        described_columns: columns,
      }),
    );

    expect(parsed).toEqual({ columns, issues });
  });
});
