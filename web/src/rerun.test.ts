import { describe, expect, it } from "vitest";

import type { Condition, RunHistory, Task, TaskSpec } from "./api";
import { rerunAction, rerunPrefill } from "./rerun";

const baseHistory: RunHistory = {
  run_record_id: "record-1",
  run_id: "run-1",
  task_id: "task-1",
  run_params: { d_biz: "2026-08-14", region: "SH" },
  source_sql: "SELECT a.ID AS ID\n  FROM APP.ORDERS a",
  staging_table: "STG_1",
  started_at: "2026-08-15T10:00:00.000Z",
  finished_at: "2026-08-15T10:01:00.000Z",
  outcome: "FAILED",
  target_table_effect: "DISCARDED",
  stage: "FAILED",
  source_rows: 3,
  staged_rows: 3,
  sink_reported_rows: 2,
  purged_rows: 0,
  source_batches: 1,
  received_batches: 1,
  fetch_ms: 4,
  push_ms: 10,
  commit_ms: 6,
  total_rows: null,
  precount_ms: null,
  count_ms: 2,
  cursor_ms: 1,
  source_code: null,
  sink_code: "VERIFY_FAILED",
  column: null,
  value: null,
  message: "目标端：门禁计数不一致",
  failure_kind: "VERIFY_FAILED",
  unknown_reason: null,
  seq: 1,
  rows_pushed: 3,
  bytes: 64,
  ms: 10,
  last_ts: "2026-08-15T10:01:00.000Z",
  mapping_issues: [],
};

function history(overrides: Partial<RunHistory>): RunHistory {
  return { ...baseHistory, ...overrides };
}

function runtimeCondition(parameter: string): Condition {
  return {
    column: parameter.toUpperCase(),
    operator: "eq",
    value_type: "text",
    parameter,
    value_source: "runtime",
    constant: "",
  };
}

function spec(parameters: string[]): TaskSpec {
  return {
    owner: "APP",
    table: "T_ORDERS",
    target_table: "ORDERS",
    columns: [{ source: "ID", target: "ID" }],
    primary_key: ["ID"],
    conditions: [
      {
        column: "STATUS",
        operator: "eq",
        value_type: "text",
        parameter: "status",
        value_source: "constant",
        constant: "OK",
      },
      ...parameters.map(runtimeCondition),
    ],
    order_by: [],
  };
}

function task(parameters: string[]): Task {
  return {
    task_id: "task-1",
    name: "订单日增量",
    source_datasource_id: "ds-ora",
    target_datasource_id: "ds-my",
    spec: spec(parameters),
  };
}

const tasks = [task(["d_biz", "region"])];

describe("rerun eligibility", () => {
  it("offers a rerun on a FAILED run", () => {
    expect(rerunAction(history({}), tasks)).toEqual({
      kind: "enabled",
      task: tasks[0],
    });
  });

  it.each(["PROCESS_DISAPPEARED", "SERVICE_RESTARTED"] as const)(
    "offers a rerun on an unknown outcome (%s) — upsert 幂等，重跑是安全的",
    (unknownReason) => {
      const row = history({
        unknown_reason: unknownReason,
        outcome: null,
        target_table_effect: null,
        sink_code: null,
      });
      expect(rerunAction(row, tasks)).toEqual({ kind: "enabled", task: tasks[0] });
    },
  );

  it("hides the rerun on a SUCCEEDED run", () => {
    expect(rerunAction(history({ outcome: "SUCCEEDED" }), tasks)).toEqual({
      kind: "hidden",
    });
  });

  it.each([null, "STREAMING"])(
    "hides the rerun while the run is still live (stage %s)",
    (stage) => {
      const row = history({
        outcome: null,
        stage,
        finished_at: null,
        sink_code: null,
        target_table_effect: null,
      });
      expect(rerunAction(row, tasks)).toEqual({ kind: "hidden" });
    },
  );

  it("disables the rerun — with a reason — when the task is gone", () => {
    const action = rerunAction(history({ task_id: "task-gone" }), tasks);
    expect(action.kind).toBe("disabled");
    expect(action.kind === "disabled" && action.reason).toContain("任务已删除");
  });

  it("disables the rerun while the task list has not been read yet", () => {
    const action = rerunAction(history({}), null);
    expect(action.kind).toBe("disabled");
    expect(action.kind === "disabled" && action.reason).toContain("任务清单");
  });

  it("keeps a SUCCEEDED row hidden even when its task is gone", () => {
    expect(
      rerunAction(history({ outcome: "SUCCEEDED", task_id: "task-gone" }), tasks),
    ).toEqual({ kind: "hidden" });
  });
});

describe("rerun prefill", () => {
  it("takes the value from the row when the parameter still exists", () => {
    expect(rerunPrefill(spec(["d_biz", "region"]), baseHistory.run_params)).toEqual({
      d_biz: "2026-08-14",
      region: "SH",
    });
  });

  it("leaves a parameter empty when the row has no value for it", () => {
    expect(rerunPrefill(spec(["d_biz", "channel"]), baseHistory.run_params)).toEqual({
      d_biz: "2026-08-14",
      channel: "",
    });
  });

  it("drops row values whose parameter the spec no longer declares", () => {
    expect(rerunPrefill(spec(["d_biz"]), baseHistory.run_params)).toEqual({
      d_biz: "2026-08-14",
    });
  });

  it("ignores constant conditions — only 运行时填 gets prefilled", () => {
    expect(rerunPrefill(spec([]), { status: "BAD", d_biz: "2026-08-14" })).toEqual(
      {},
    );
  });

  it("keeps the empty string as a real value, not a missing one", () => {
    expect(rerunPrefill(spec(["d_biz"]), { d_biz: "" })).toEqual({ d_biz: "" });
  });

  it("prefills nothing but the empty form when there is no previous run", () => {
    expect(rerunPrefill(spec(["d_biz", "region"]), {})).toEqual({
      d_biz: "",
      region: "",
    });
  });
});
