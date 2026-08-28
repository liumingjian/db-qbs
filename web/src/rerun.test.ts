import { describe, expect, it } from "vitest";

import type { RunHistory, Task, TaskSpec } from "./api";
import { rerunAction } from "./rerun";

const baseHistory: RunHistory = {
  run_record_id: "record-1",
  run_id: "run-1",
  task_id: "task-1",
  task_name: "订单日增量",
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

function spec(): TaskSpec {
  return {
    owner: "APP",
    table: "T_ORDERS",
    target_table: "ORDERS",
    columns: [{ source: "ID", target: "ID" }],
    write_mode: "APPEND",
    primary_key: ["ID"],
    where_clause: "STATUS = 'OK'",
  };
}

function task(): Task {
  return {
    task_id: "task-1",
    name: "订单日增量",
    source_datasource_id: "ds-ora",
    target_datasource_id: "ds-my",
    spec: spec(),
  };
}

const tasks = [task()];

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
