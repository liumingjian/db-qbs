import { describe, expect, it } from "vitest";

import type { RunHistory } from "./api";
import { remediationFor, rowRunAction } from "./troubleshooting";

function history(overrides: Partial<RunHistory> = {}): RunHistory {
  return {
    run_record_id: "record-7",
    run_id: "run-7",
    task_id: "task-1",
    source_sql: "SELECT ID FROM APP.ORDERS",
    staging_table: "STG_7",
    started_at: "2026-08-25T00:00:00.000Z",
    finished_at: "2026-08-25T00:01:00.000Z",
    outcome: "FAILED",
    target_table_effect: "DISCARDED",
    stage: "FAILED",
    source_rows: 1,
    staged_rows: 0,
    sink_reported_rows: 0,
    purged_rows: 0,
    source_batches: 1,
    received_batches: 0,
    total_rows: 1,
    precount_ms: 1,
    fetch_ms: 2,
    push_ms: 3,
    commit_ms: 4,
    count_ms: 5,
    cursor_ms: 6,
    source_code: null,
    sink_code: null,
    column: null,
    value: null,
    message: "failed",
    failure_kind: "SOURCE_QUERY",
    unknown_reason: null,
    seq: 0,
    rows_pushed: 0,
    bytes: 0,
    ms: 0,
    last_ts: "2026-08-25T00:01:00.000Z",
    mapping_issues: [],
    ...overrides,
  };
}

describe("row run action", () => {
  it("uses the same slot for start and the displayed live record's stop", () => {
    expect(rowRunAction(undefined, false)).toEqual({ kind: "start", disabled: false });
    expect(rowRunAction(history({ outcome: "SUCCEEDED" }), false)).toEqual({
      kind: "start",
      disabled: false,
    });
    expect(
      rowRunAction(
        history({ outcome: null, finished_at: null, stage: "STREAMING" }),
        false,
      ),
    ).toEqual({ kind: "stop", runRecordId: "record-7", refusal: null });
  });

  it("only disables the start action while that task is being submitted", () => {
    expect(rowRunAction(undefined, true)).toEqual({ kind: "start", disabled: true });
  });

  it("carries the same refusal the run screen shows, so the row learns it before the 409", () => {
    // 停不停得了的规则在 `abortRefusal` 里，运行详情屏一直读着它，而列表这一颗
    // 一直无条件亮着——于是同一条规则在两个地方给出两个答案，列表那个答案是错的
    // （UX 评审 P1-11）。
    expect(
      rowRunAction(
        history({ outcome: null, finished_at: null, stage: "COMMITTING" }),
        false,
      ),
    ).toEqual({
      kind: "stop",
      runRecordId: "record-7",
      refusal: "已过封口点：暂存表的处置权已经交给目标端",
    });
  });
});

describe("failure remediation", () => {
  it.each(["CONFIG", "SOURCE_CONNECT", "SOURCE_DBLINK"])(
    "sends %s to the datasource screen, not into the wizard",
    (failureKind) => {
      // 连不上源库不是任务定义的问题，向导里没有一个字能改它。
      expect(remediationFor(history({ failure_kind: failureKind }))).toMatchObject({
        kind: "datasources",
      });
    },
  );

  it.each(["SHAPE_PRECHECK", "MAPPING_PRECHECK"])(
    "routes %s to the mapping step, which is step 1",
    (failureKind) => {
      expect(remediationFor(history({ failure_kind: failureKind }))).toEqual({
        kind: "wizard",
        step: 1,
        label: "修改字段映射",
      });
    },
  );

  it("routes a source query failure to the filter step", () => {
    expect(remediationFor(history({ failure_kind: "SOURCE_QUERY" }))).toMatchObject({
      kind: "wizard",
      step: 2,
    });
  });

  it.each(["SINK_WRITE", "DATA_REJECTED", "VERIFY_FAILED"])(
    "routes %s to the target-table check",
    (failureKind) => {
      expect(remediationFor(history({ failure_kind: failureKind }))).toMatchObject({
        kind: "wizard",
        step: 3,
      });
    },
  );

  it.each(["NETWORK", "SINK_ENVIRONMENT", "ORCHESTRATOR", "DEFECT", "UNKNOWN", "TARGET_BUSY", null])(
    "still says something about %s instead of going silent",
    (failureKind) => {
      // 16 个分类里有 8 个原来回 null，界面上于是什么都不出——而「这一类没有可改的地方」
      // 本身就是那个人最需要知道的一句话。
      const remediation = remediationFor(history({ failure_kind: failureKind }));
      expect(remediation?.kind).toBe("none");
      expect(remediation).toHaveProperty("reason");
      expect((remediation as { reason: string }).reason.length).toBeGreaterThan(8);
    },
  );

  it("does not offer remediation for a successful record", () => {
    expect(
      remediationFor(history({ outcome: "SUCCEEDED", failure_kind: "SOURCE_QUERY" })),
    ).toBeNull();
  });
});
