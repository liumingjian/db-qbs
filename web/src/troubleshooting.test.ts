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
    ).toEqual({ kind: "stop", runRecordId: "record-7" });
  });

  it("only disables the start action while that task is being submitted", () => {
    expect(rowRunAction(undefined, true)).toEqual({ kind: "start", disabled: true });
  });
});

describe("failure remediation", () => {
  it.each(["CONFIG", "SOURCE_CONNECT", "SOURCE_DBLINK", "SOURCE_QUERY", "SOURCE_VALUE"])(
    "routes %s to source and query settings",
    (failureKind) => {
      expect(remediationFor(history({ failure_kind: failureKind }))).toEqual({
        step: 2,
        label: "修改源端与取数设置",
      });
    },
  );

  it.each(["SHAPE_PRECHECK", "MAPPING_PRECHECK"])(
    "routes %s to mapping",
    (failureKind) => {
      expect(remediationFor(history({ failure_kind: failureKind }))).toEqual({
        step: 3,
        label: "修改字段映射",
      });
    },
  );

  it.each(["NETWORK", "SINK_ENVIRONMENT", "ORCHESTRATOR", "DEFECT", "UNKNOWN", null])(
    "does not claim %s can be fixed by editing",
    (failureKind) => {
      expect(remediationFor(history({ failure_kind: failureKind }))).toBeNull();
    },
  );

  it("does not offer remediation for a successful record", () => {
    expect(
      remediationFor(history({ outcome: "SUCCEEDED", failure_kind: "SOURCE_QUERY" })),
    ).toBeNull();
  });
});
