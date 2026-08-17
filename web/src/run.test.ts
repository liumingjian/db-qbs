import { describe, expect, it } from "vitest";

import type { RunDetail } from "./api";
import { runPresentation } from "./run";

function terminal(overrides: Partial<RunDetail> = {}): RunDetail {
  return {
    run_record_id: "record-01",
    run_id: "run-01",
    task_id: "task-01",
    biz_date: "2026-08-14",
    staging_table: "ORDERS__stg_run_01",
    started_at: "2026-08-15T10:00:00.000Z",
    finished_at: "2026-08-15T10:01:00.000Z",
    outcome: "FAILED",
    target_table_effect: "DISCARDED",
    stage: "PREPARING",
    source_rows: 0,
    staged_rows: 0,
    sink_reported_rows: 0,
    purged_rows: 0,
    source_batches: 0,
    received_batches: 0,
    fetch_ms: 0,
    push_ms: 0,
    commit_ms: 0,
    count_ms: 0,
    cursor_ms: 0,
    source_code: null,
    sink_code: null,
    column: null,
    value: null,
    message: "failed",
    failure_kind: "VERIFY_FAILED",
  unknown_reason: null,
    seq: 0,
    rows_pushed: 0,
    bytes: 0,
    ms: 0,
    last_ts: "2026-08-15T10:01:00.000Z",
    shape_checks: [],
    mapping_issues: [],
    live: false,
    ...overrides,
  } as RunDetail;
}

describe("run detail presentation", () => {
  it("keeps accepted runs distinct from PREPARING", () => {
    const presentation = runPresentation({
      run_record_id: "record-01",
      run_id: null,
      biz_date: null,
      staging_table: null,
      stage: null,
      seq: 0,
      rows_pushed: 0,
      bytes: 0,
      ms: 0,
      last_ts: null,
      live: true,
    });

    expect(presentation).toMatchObject({
      kind: "accepted",
      phase: null,
      conclusion: "已受理，正在拉起",
      terminalEffect: null,
    });
  });

  it("shows the original live phase and all four aggregate values", () => {
    const detail: RunDetail = {
      run_record_id: "record-01",
      run_id: "run-01",
      biz_date: "2026-08-14",
      staging_table: "ORDERS__stg_run_01",
      stage: "STREAMING",
      seq: 4,
      rows_pushed: 1200,
      bytes: 88000,
      ms: 230,
      last_ts: "2026-08-15T10:00:04.000Z",
      live: true,
    };

    expect(runPresentation(detail)).toMatchObject({
      kind: "live",
      phase: "STREAMING",
      metrics: { rows: 1200, sequence: 4, milliseconds: 230, bytes: 88000 },
      terminalEffect: null,
    });
  });

  it("does not invent terminal blocks for local shape, mapping, or unknown failures", () => {
    expect(runPresentation(terminal({ run_id: null }))).toMatchObject({
      kind: "shape-failed",
      terminalEffect: null,
    });
    expect(runPresentation(terminal({ sink_code: "PRECHECK_FAILED" }))).toMatchObject({
      kind: "mapping-failed",
      terminalEffect: null,
    });
    expect(
      runPresentation(
        terminal({
          target_table_effect: null,
          source_code: null,
          sink_code: null,
          unknown_reason: "SERVICE_RESTARTED",
        }),
      ),
    ).toMatchObject({ kind: "unknown", terminalEffect: null, error: null });
  });
});
