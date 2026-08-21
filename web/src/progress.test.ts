import { describe, expect, it } from "vitest";

import type { RunHistory } from "./api";
import { progressOf } from "./progress";

function historyRow(overrides: Partial<RunHistory> = {}): RunHistory {
  return {
    run_record_id: "record-1",
    run_id: "run-1",
    task_id: "task-1",
    run_params: {},
    source_sql: "SELECT 1 FROM DUAL",
    staging_table: "STG_1",
    started_at: "2026-08-20T10:00:00.000Z",
    finished_at: "2026-08-20T10:01:00.000Z",
    outcome: "SUCCEEDED",
    target_table_effect: "SWAPPED",
    stage: "DONE",
    source_rows: 3,
    staged_rows: 3,
    sink_reported_rows: 3,
    purged_rows: 0,
    source_batches: 1,
    received_batches: 1,
    fetch_ms: 1,
    push_ms: 1,
    commit_ms: 1,
    total_rows: 3,
    precount_ms: 12,
    count_ms: 1,
    cursor_ms: 1,
    source_code: null,
    sink_code: null,
    column: null,
    value: null,
    message: null,
    failure_kind: null,
    unknown_reason: null,
    seq: 1,
    rows_pushed: 3,
    bytes: 64,
    ms: 3,
    last_ts: "2026-08-20T10:01:00.000Z",
    mapping_issues: [],
    ...overrides,
  };
}

describe("progressOf", () => {
  it("向下取整，99.98% 是 99% 而不是 100%", () => {
    // ADR-0043 §7 边界 1 的原型：四舍五入成 100% 等于拿显示撒谎。
    const cell = progressOf(
      historyRow({ total_rows: 12000, rows_pushed: 11998 }),
    );
    expect(cell.kind).toBe("value");
    expect(cell.label).toBe("99%");
  });

  it("100% 只在真跑完时出现", () => {
    expect(progressOf(historyRow({ total_rows: 120, rows_pushed: 120 })).label).toBe(
      "100%",
    );
    expect(progressOf(historyRow({ total_rows: 120, rows_pushed: 119 })).label).toBe(
      "99%",
    );
  });

  it("尚未运行是「—」，不是 0%", () => {
    const cell = progressOf(undefined);
    expect(cell.kind).toBe("none");
    expect(cell.label).toBe("—");
    expect(cell.title).toContain("尚未运行");
  });

  it("跑了但一行没动是 0%，与「没跑过」分得开", () => {
    const cell = progressOf(historyRow({ total_rows: 120, rows_pushed: 0 }));
    expect(cell.kind).toBe("value");
    expect(cell.label).toBe("0%");
  });

  it("开跑前计数失败是「—」并自陈原因，不是 0%", () => {
    const cell = progressOf(historyRow({ total_rows: null, rows_pushed: 120 }));
    expect(cell.kind).toBe("unknown");
    expect(cell.label).toBe("—");
    expect(cell.title).toContain("未取到总行数");
  });

  it("进行中的行也有真百分比——分母是开跑前买下来的", () => {
    const cell = progressOf(
      historyRow({
        total_rows: 1200,
        rows_pushed: 430,
        outcome: null,
        stage: "STREAMING",
        finished_at: null,
        target_table_effect: null,
        sink_code: null,
      }),
    );
    expect(cell).toMatchObject({ kind: "value", label: "35%", tone: "live" });
  });

  it("空表当作跑完，不是永远停在 0%", () => {
    const cell = progressOf(historyRow({ total_rows: 0, rows_pushed: 0 }));
    expect(cell.label).toBe("100%");
  });

  it("已推送多于分母时夹回 100%，不出 103% 这种数", () => {
    // 分母是开跑那一刻的事实，随后源端还在长——夹回是实话里最保守的那一句。
    expect(progressOf(historyRow({ total_rows: 100, rows_pushed: 103 })).label).toBe(
      "100%",
    );
  });

  it("着色跟着运行结局走，不自立一套语义", () => {
    expect(progressOf(historyRow()).kind === "value" && progressOf(historyRow()).kind).toBe(
      "value",
    );
    const failed = progressOf(
      historyRow({ outcome: "FAILED", sink_code: "VERIFY_FAILED" }),
    );
    expect(failed).toMatchObject({ tone: "bad" });
    expect(progressOf(historyRow())).toMatchObject({ tone: "ok" });
  });
});
