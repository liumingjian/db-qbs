import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { RunHistory, Task } from "./api";
import { JobCenterScreen } from "./JobCenterScreen";

const NEVER_RUN_TASK: Task = {
  task_id: "task-1",
  name: "客户主档",
  source_datasource_id: "source-1",
  target_datasource_id: "target-1",
  spec: {
    owner: "APP",
    table: "CUSTOMER",
    target_table: "customer",
    columns: [{ source: "ID", target: "ID" }],
    write_mode: "APPEND",
    schedule_enabled: false,
    primary_key: ["ID"],
    where_clause: "",
  },
};

const LATEST_RUN: RunHistory = {
  run_record_id: "record-1",
  run_id: "run-1",
  task_id: "task-1",
  task_name: "客户主档",
  source_sql: "SELECT 1 FROM DUAL",
  staging_table: "stg_1",
  started_at: "2026-08-20T10:00:00.000Z",
  finished_at: "2026-08-20T10:01:00.000Z",
  outcome: "SUCCEEDED",
  target_table_effect: "SWAPPED",
  stage: "SUCCEEDED",
  source_rows: 3,
  staged_rows: 3,
  sink_reported_rows: 3,
  purged_rows: 0,
  source_batches: 1,
  received_batches: 1,
  total_rows: 3,
  precount_ms: 1,
  fetch_ms: 1,
  push_ms: 1,
  commit_ms: 1,
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
  bytes: 128,
  ms: 12,
  last_ts: "2026-08-20T10:01:00.000Z",
  mapping_issues: [],
};

const BASE_PROPS = {
  tasks: [NEVER_RUN_TASK],
  datasources: [],
  latestRuns: new Map<string, RunHistory>(),
  refreshing: false,
  onRefresh: () => undefined,
  onCreate: () => undefined,
  onEdit: () => undefined,
  onViewLogs: () => undefined,
  onDelete: () => undefined,
  startingTaskId: null,
  onStart: () => undefined,
  onStop: () => undefined,
  onRerun: () => undefined,
  onEditFailure: () => undefined,
  onChanged: () => undefined,
  focusTaskId: null,
  onFocusConsumed: () => undefined,
};

describe("job row actions", () => {
  // The slot is held open rather than dropped (UX review P2-15): a vanishing button
  // puts the same action at a different x on neighbouring rows, so muscle memory
  // lands on whichever button slid into its place.
  it("keeps the run-details slot, disabled, when a task has never run", () => {
    const html = renderToStaticMarkup(createElement(JobCenterScreen, {
      tasks: [NEVER_RUN_TASK],
      datasources: [],
      latestRuns: new Map(),
      refreshing: false,
      onRefresh: () => undefined,
      onCreate: () => undefined,
      onEdit: () => undefined,
      onViewLogs: () => undefined,
      onDelete: () => undefined,
      startingTaskId: null,
      onStart: () => undefined,
      onStop: () => undefined,
      onRerun: () => undefined,
      onEditFailure: () => undefined,
      onChanged: () => undefined,
      focusTaskId: null,
      onFocusConsumed: () => undefined,
    }));

    expect(html).toContain("发起运行");
    expect(html).toContain("运行详情");
    expect(html).toContain('title="这个任务还没有跑过"');
    expect(html).toContain('aria-label="运行详情" disabled=""');
  });

  // 改名并进了编辑向导（#259），空出来的位子给「查看日志」（#263）。
  it("puts 查看日志 where 改名 used to be", () => {
    const html = renderToStaticMarkup(createElement(JobCenterScreen, {
      ...BASE_PROPS,
      tasks: [NEVER_RUN_TASK],
    }));

    expect(html).not.toContain("改名");
    expect(html).toContain("查看日志");
    // 没跑过的任务这颗按不动，但位子占住不撤（P2-15）。
    expect(html).toContain('aria-label="查看日志" disabled=""');
  });

  it("lights 查看日志 up for a task that has run", () => {
    const html = renderToStaticMarkup(createElement(JobCenterScreen, {
      ...BASE_PROPS,
      tasks: [NEVER_RUN_TASK],
      latestRuns: new Map([["task-1", LATEST_RUN]]),
    }));

    expect(html).toContain('aria-label="查看日志"');
    expect(html).not.toContain('aria-label="查看日志" disabled=""');
  });
});

describe("纯追加写的可见标记（#261）", () => {
  function render(task: Task): string {
    return renderToStaticMarkup(createElement(JobCenterScreen, {
      tasks: [task],
      datasources: [],
      latestRuns: new Map(),
      refreshing: false,
      onRefresh: () => undefined,
      onCreate: () => undefined,
      onEdit: () => undefined,
      onViewLogs: () => undefined,
      onDelete: () => undefined,
      startingTaskId: null,
      onStart: () => undefined,
      onStop: () => undefined,
      onRerun: () => undefined,
      onEditFailure: () => undefined,
      onChanged: () => undefined,
      focusTaskId: null,
      onFocusConsumed: () => undefined,
    }));
  }

  it("marks a task whose target table has no primary key", () => {
    // 「这个任务重跑会翻倍」必须在**看清单的时候**就知道，不能只在点进去之后才说。
    const html = render({
      ...NEVER_RUN_TASK,
      spec: { ...NEVER_RUN_TASK.spec, primary_key: [] },
    });

    expect(html).toContain("纯追加写");
    expect(html).toContain("重跑会产生重复数据");
  });

  it("says nothing extra about a task that upserts, because that is today's behaviour", () => {
    const html = render(NEVER_RUN_TASK);

    expect(html).not.toContain("纯追加写");
  });
});
