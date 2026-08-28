import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { RunHistory } from "./api";
import { UnknownConclusion } from "./components/DesignSystem";
import { FailureEvidence } from "./FailureEvidence";

const UNKNOWN_RUN: RunHistory = {
  run_record_id: "record-9",
  run_id: null,
  task_id: "task-1",
  task_name: "订单日增量",
  source_sql: "SELECT a.ID AS ID\n  FROM APP.ORDERS a",
  staging_table: "STG_9",
  started_at: "2026-08-15T10:00:00.000Z",
  finished_at: "2026-08-15T10:01:00.000Z",
  outcome: null,
  target_table_effect: "UNKNOWN",
  stage: "STREAMING",
  source_rows: null,
  staged_rows: 1234,
  sink_reported_rows: null,
  purged_rows: 0,
  source_batches: 2,
  received_batches: 2,
  fetch_ms: 4,
  push_ms: 10,
  commit_ms: 0,
  total_rows: null,
  precount_ms: null,
  count_ms: 2,
  cursor_ms: 1,
  source_code: null,
  sink_code: null,
  column: null,
  value: null,
  message: null,
  failure_kind: null,
  unknown_reason: null,
  seq: 2,
  rows_pushed: 1234,
  bytes: 64,
  ms: 10,
  last_ts: "2026-08-15T10:01:00.000Z",
  mapping_issues: [],
};

function evidenceHtml(run: RunHistory): string {
  return renderToStaticMarkup(createElement(FailureEvidence, {
    run,
    variant: "unknown",
    onEditTask: () => undefined,
  }));
}

describe("unknown-outcome clues", () => {
  // The two facts below are about *how far this run got*, not about which machine it
  // connected to, so they must not ride along with the connection snapshot. A record
  // written before snapshots existed — and PROCESS_DISAPPEARED is the likeliest one —
  // has no snapshot at all, and that is exactly the run you need them for.
  it("shows the staging table and last known row count without a connection snapshot", () => {
    const html = evidenceHtml(UNKNOWN_RUN);

    expect(html).toContain("此运行记录创建时尚未记录连接快照。");
    expect(html).toContain("核对线索");
    expect(html).toContain("重跑是安全的");
    expect(html).toContain("暂存表");
    expect(html).toContain("STG_9");
    expect(html).toContain("最后已知行数");
    expect(html).toContain("1,234");
  });

  it("falls back to an em dash when even the staging table is unknown", () => {
    const html = evidenceHtml({ ...UNKNOWN_RUN, staging_table: null });

    expect(html).toContain("暂存表");
    expect(html).toContain("最后已知行数");
  });

  // `unknown_reason` is null on older records; the two render sites each used to spell
  // `is-${reason?.toLowerCase()}` inline, which put a literal `is-undefined` in the DOM.
  it("omits the reason modifier when there is no reason", () => {
    const html = renderToStaticMarkup(createElement(UnknownConclusion, {
      reason: null,
      conclusion: "运行中断，未能确认结局。",
    }));

    expect(html).toContain('class="unknown-conclusion"');
    expect(html).not.toContain("is-undefined");
  });

  it("keeps the reason modifier when there is one", () => {
    const html = renderToStaticMarkup(createElement(UnknownConclusion, {
      reason: "SERVICE_RESTARTED",
      conclusion: "服务重启，未能确认结局。",
    }));

    expect(html).toContain("is-service_restarted");
  });
});
