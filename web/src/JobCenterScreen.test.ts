import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { RunHistory, Task } from "./api";
import { JobCenterScreen, queuedTitle } from "./JobCenterScreen";

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
  onDelete: () => undefined,
  startingTaskId: null,
  onStart: () => undefined,
  onStop: () => undefined,
  onRetryRelease: () => undefined,
  onRerun: () => undefined,
  onEditFailure: () => undefined,
  onChanged: () => undefined,
  focusTaskId: null,
  onFocusConsumed: () => undefined,
};

describe("job row actions", () => {
  // #271：占用还在的时候，那一格说的必须是实话——第六颗按钮会挤坏这一列，
  // 所以三态四态全挤在第一格里，与「发起与停止共用这一格」是同一个决定。
  it("says 停止中… instead of offering a run while the hold is being released", () => {
    const html = renderToStaticMarkup(createElement(JobCenterScreen, {
      ...BASE_PROPS,
      latestRuns: new Map([
        [
          "task-1",
          {
            ...LATEST_RUN,
            outcome: null,
            finished_at: null,
            stage: "STREAMING",
            target_hold: "RELEASING" as const,
          } as RunHistory,
        ],
      ]),
    }));

    expect(html).toContain("停止中…（目标表占用尚未释放）");
    expect(html).not.toContain("发起运行");
    // 已经停过一次了，「停止运行」那一颗也不该再出现。
    expect(html).not.toContain("停止运行 record-1");
  });

  it("offers the retry instead of a run while the hold could not be released", () => {
    const html = renderToStaticMarkup(createElement(JobCenterScreen, {
      ...BASE_PROPS,
      latestRuns: new Map([
        [
          "task-1",
          {
            ...LATEST_RUN,
            outcome: "FAILED",
            unknown_reason: "STOPPED_BY_USER" as const,
            message: "已由用户停止",
            target_hold: "HELD" as const,
            target_hold_message: "暂存表 drop 不掉",
          } as RunHistory,
        ],
      ]),
    }));

    expect(html).toContain("锁未释放，点此重试（暂存表 drop 不掉）");
    expect(html).not.toContain("发起运行");
  });

  // 操作列**恰好五颗**，第六颗会挤坏它——所以发起、停止、停止中、锁未释放四态
  // 全挤在第一格里。这条用例数的就是那一行上的按钮个数（#271）。
  it("keeps the action column to five buttons while the hold is stuck", () => {
    const actionButtons = (run: RunHistory) =>
      (renderToStaticMarkup(createElement(JobCenterScreen, {
        ...BASE_PROPS,
        latestRuns: new Map([["task-1", run]]),
      }))
        .split('<td class="action-column">')[1]
        ?.split("</td>")[0]
        ?.match(/<button/g) ?? []).length;

    expect(actionButtons({ ...LATEST_RUN, target_hold: "HELD" } as RunHistory)).toBe(5);
    // 占用那一格换的是同一颗按钮的字面，不是多加一颗：没有占用时也是五颗。
    expect(actionButtons(LATEST_RUN)).toBe(5);
  });

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
      onDelete: () => undefined,
      startingTaskId: null,
      onStart: () => undefined,
      onStop: () => undefined,
      onRetryRelease: () => undefined,
      onRerun: () => undefined,
      onEditFailure: () => undefined,
      onChanged: () => undefined,
      focusTaskId: null,
      onFocusConsumed: () => undefined,
    }));

    expect(html).toContain("发起运行");
    expect(html).toContain("查看详情");
    expect(html).toContain('title="这个任务还没有跑过"');
    expect(html).toContain('aria-label="查看详情" disabled=""');
  });

  // 操作列只留五颗：启停、定时任务、编辑、查看详情、删除。日志不再单独占一颗——
  // 它本来就是运行详情的一段，第二个入口只是把同一个地方说成两件事。
  it("keeps the action column to five buttons", () => {
    const html = renderToStaticMarkup(createElement(JobCenterScreen, {
      ...BASE_PROPS,
      tasks: [NEVER_RUN_TASK],
      latestRuns: new Map([["task-1", LATEST_RUN]]),
    }));

    expect(html).not.toContain("查看日志");
    expect(html).not.toContain("复制 cURL");
    expect(html).not.toContain("改名");
    // 终局的运行给的是「发起运行」，进行中才换成「停止运行」——同一颗按钮的两副面孔。
    for (const label of ["发起运行", "定时任务", "编辑任务定义", "查看详情", "删除"]) {
      expect(html).toContain(label);
    }
  });

  it("lights 查看详情 up for a task that has run", () => {
    const html = renderToStaticMarkup(createElement(JobCenterScreen, {
      ...BASE_PROPS,
      tasks: [NEVER_RUN_TASK],
      latestRuns: new Map([["task-1", LATEST_RUN]]),
    }));

    expect(html).toContain('aria-label="查看详情"');
    expect(html).not.toContain('aria-label="查看详情" disabled=""');
  });

  // 那颗按钮上写着这条任务此刻的调度状态，三档各说各的（#265 的两个字段）。
  it("says on the schedule button what this task's schedule is", () => {
    const off = renderToStaticMarkup(createElement(JobCenterScreen, { ...BASE_PROPS }));
    expect(off).toContain("定时任务：未配置");

    const running = renderToStaticMarkup(createElement(JobCenterScreen, {
      ...BASE_PROPS,
      tasks: [{
        ...NEVER_RUN_TASK,
        spec: { ...NEVER_RUN_TASK.spec, schedule_cron: "0 2 * * *", schedule_enabled: true },
      }],
    }));
    expect(running).toContain("定时任务：0 2 * * *");

    // 停用不等于没配：表达式还在，人只是把它按停了。
    const paused = renderToStaticMarkup(createElement(JobCenterScreen, {
      ...BASE_PROPS,
      tasks: [{
        ...NEVER_RUN_TASK,
        spec: { ...NEVER_RUN_TASK.spec, schedule_cron: "0 2 * * *", schedule_enabled: false },
      }],
    }));
    expect(paused).toContain("定时任务：已停用（0 2 * * *）");
  });
});

describe("清单上不再复述写入方式（主界面从简）", () => {
  // 「纯追加写」/「先清空再导入」两枚标记撤了：它们是任务**属性**，一天看一次就够，
  // 却在每一行的任务名后面各占一块。两句话都还在——编辑向导第 1 步那格写入方式，
  // 以及运行详情里那句「这一次做了什么」，都在人正要做决定或正在追责的那一刻说。
  function render(task: Task): string {
    return renderToStaticMarkup(createElement(JobCenterScreen, { ...BASE_PROPS, tasks: [task] }));
  }

  it("says nothing extra beside the name for either write mode", () => {
    const appendOnly = render({
      ...NEVER_RUN_TASK,
      spec: { ...NEVER_RUN_TASK.spec, primary_key: [] },
    });
    expect(appendOnly).not.toContain("纯追加写");
    expect(appendOnly).not.toContain("重跑会产生重复数据");

    const clearing = render({
      ...NEVER_RUN_TASK,
      spec: { ...NEVER_RUN_TASK.spec, write_mode: "CLEAR_THEN_IMPORT" },
    });
    expect(clearing).not.toContain("先清空再导入");
  });
});

describe("queuedTitle", () => {
  // 排队中的那一条要说得出**它在等什么**（#266）：队列活在服务端一条后台线程里，
  // 只挂一枚「排队中」而不给理由，等于把「什么都没发生」换了个说法。
  it("says when it should have fired and what it is waiting on", () => {
    expect(
      queuedTitle({
        task_id: "task-1",
        task_name: "客户主档",
        due_at: "2026-08-28 02:00",
        waiting_reason: "目标端 agent「上交」的并发额度已满（在飞 4，上限 4），排队等待",
      }),
    ).toBe(
      "本该于 2026-08-28 02:00 触发；目标端 agent「上交」的并发额度已满（在飞 4，上限 4），排队等待",
    );
  });

  it("still answers before the first dispatch attempt has happened", () => {
    expect(
      queuedTitle({
        task_id: "task-1",
        task_name: "客户主档",
        due_at: "2026-08-28 02:00",
        waiting_reason: "",
      }),
    ).toBe("本该于 2026-08-28 02:00 触发；已到触发时刻，等待派发");
  });
});
