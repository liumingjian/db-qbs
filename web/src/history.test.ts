import { describe, expect, it } from "vitest";

import type { RunHistory } from "./api";
import { historyPresentation, runIdPresentation } from "./history";

const baseHistory: RunHistory = {
  run_record_id: "record-1",
  run_id: "run-1",
  task_id: "task-1",
  biz_date: "2026-08-14",
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
  count_ms: 2,
  cursor_ms: 1,
  source_code: null,
  sink_code: "VERIFY_FAILED",
  column: "AMOUNT",
  value: "secret",
  message: "目标端：门禁计数不一致",
  unknown_reason: null,
  seq: 1,
  rows_pushed: 3,
  bytes: 64,
  ms: 10,
  last_ts: "2026-08-15T10:01:00.000Z",
};

function history(overrides: Partial<RunHistory>): RunHistory {
  return { ...baseHistory, ...overrides };
}

describe("run history presentation", () => {
  it.each([
    ["PROCESS_DISAPPEARED", "进程消失，无终态日志"],
    ["SERVICE_RESTARTED", "服务重启，结局未知"],
  ] as const)(
    "presents %s as an unknown outcome without sink status",
    (unknownReason, conclusion) => {
      expect(
        historyPresentation(
          history({
            unknown_reason: unknownReason,
            target_table_effect: null,
            sink_code: null,
            message: conclusion,
          }),
        ),
      ).toEqual({
        kind: "unknown",
        conclusion,
        terminalEffect: null,
        error: null,
      });
    },
  );

  it("presents a protocol failure with its terminal effect and HTTP status", () => {
    expect(historyPresentation(baseHistory)).toEqual({
      kind: "failed",
      conclusion: "目标端：门禁计数不一致",
      terminalEffect: "DISCARDED",
      error: { code: "VERIFY_FAILED", httpStatus: 409 },
    });
  });

  it("does not present a source error as a sink protocol error", () => {
    expect(
      historyPresentation(
        history({ source_code: "1555", sink_code: null }),
      ).error,
    ).toBeNull();
  });

  it("keeps an unmapped sink error code without inventing an HTTP status", () => {
    expect(
      historyPresentation(
        history({ sink_code: "STAGING_CREATE_FAILED" }),
      ).error,
    ).toEqual({ code: "STAGING_CREATE_FAILED", httpStatus: null });
  });

  it.each([
    { run_id: null, sink_code: "VERIFY_FAILED" },
    { run_id: "run-1", sink_code: "PRECHECK_FAILED" },
  ] satisfies Array<Partial<RunHistory>>)(
    "does not invent a sink tombstone for $sink_code without a created run",
    (overrides) => {
      expect(historyPresentation(history(overrides)).terminalEffect).toBeNull();
    },
  );

  it("explains a missing run id", () => {
    expect(runIdPresentation(history({ run_id: null }))).toBe(
      "未发起，目标端不知道这次运行",
    );
  });
});
