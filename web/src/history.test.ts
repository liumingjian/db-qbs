import { describe, expect, it } from "vitest";

import type { RunHistory } from "./api";
import {
  failureKindLabel,
  historyPresentation,
  knownTerminalEffect,
  runIdPresentation,
  runTaskName,
  runTriggerLabel,
} from "./history";

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
  column: "AMOUNT",
  value: "secret",
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
      conclusion: "[校验门禁] 目标端：门禁计数不一致",
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
    ["DATA_REJECTED", 400],
    ["SINK_ENVIRONMENT", 500],
    ["BATCH_WRITE_FAILED", 500],
  ] as const)("preserves the HTTP status for %s", (code, httpStatus) => {
    expect(historyPresentation(history({ sink_code: code })).error).toEqual({
      code,
      httpStatus,
    });
  });

  it.each([
    { run_id: null, sink_code: "VERIFY_FAILED", staging_table: null },
    { run_id: "run-1", sink_code: "PRECHECK_FAILED", staging_table: null },
    {
      run_id: "run-1",
      sink_code: "STAGING_CREATE_FAILED",
      staging_table: null,
    },
  ] satisfies Array<Partial<RunHistory>>)(
    "does not invent a sink tombstone for $sink_code without a created run",
    (overrides) => {
      expect(historyPresentation(history(overrides)).terminalEffect).toBeNull();
    },
  );

  it("authors the success conclusion locally instead of echoing the English message", () => {
    expect(
      historyPresentation(
        history({
          outcome: "SUCCEEDED",
          target_table_effect: "SWAPPED",
          sink_code: null,
          sink_reported_rows: 100000,
          message: "run completed successfully",
        }),
      ),
    ).toEqual({
      kind: "succeeded",
      conclusion: "目标端：运行成功：已推送 100,000 行，已按主键合并进目标表。",
      terminalEffect: "SWAPPED",
      error: null,
    });
  });

  // #264：整表替换与按主键合并是两件事，结论条上必须分得开。
  it("整表替换那一档的结论条说的是整表替换，不是按主键合并", () => {
    expect(
      historyPresentation(
        history({
          outcome: "SUCCEEDED",
          target_table_effect: "REPLACED",
          sink_code: null,
          sink_reported_rows: 42,
          message: "run completed successfully",
        }),
      ),
    ).toEqual({
      kind: "succeeded",
      conclusion: "目标端：运行成功：已推送 42 行，目标表已整表替换为本次查询结果。",
      terminalEffect: "REPLACED",
      error: null,
    });
  });

  // 服务端那一列会原样搬运它不认识的拼写（#264）。展示层照样原样摆出来——吞掉它
  // 等于把「跑数的那一端比这块屏幕新」从屏幕上抹掉；但**不能拿一个认得的词去糊它**，
  // 也不能据它下任何判断，判断走 `knownTerminalEffect`。
  it("认不出来的终态原样透出，但不被当成 SWAPPED，也不被当成 DISCARDED", () => {
    const presented = historyPresentation(
      history({
        outcome: "SUCCEEDED",
        target_table_effect: "SOMETHING_NEW",
        sink_code: null,
      }),
    );
    expect(presented.terminalEffect).toBe("SOMETHING_NEW");
    expect(knownTerminalEffect(presented.terminalEffect)).toBeNull();
    // 结论条一个字都不替它编：只说推了多少行。
    expect(presented.conclusion).not.toContain("合并");
    expect(presented.conclusion).not.toContain("整表替换");
  });

  it("没走到目标端的那些行，`terminalEffect` 才是 null——那是「无从谈起」，不是「不认识」", () => {
    expect(
      historyPresentation(history({ run_id: null, target_table_effect: "SWAPPED" }))
        .terminalEffect,
    ).toBeNull();
  });

  it("does not claim a swap the sink never reported", () => {
    expect(
      historyPresentation(
        history({
          outcome: "SUCCEEDED",
          target_table_effect: "UNKNOWN",
          sink_code: null,
          sink_reported_rows: null,
          staged_rows: 7,
          message: "run completed successfully",
        }),
      ).conclusion,
    ).toBe("目标端：运行成功：已推送 7 行。");
  });

  it.each([
    ["SOURCE_CONNECT", "[Oracle 连接] 源端：ORA-12541"],
    ["SOURCE_DBLINK", "[dblink] 源端：ORA-12541"],
    ["NETWORK", "[网络中断] 源端：ORA-12541"],
    ["SINK_WRITE", "[MySQL 写入] 源端：ORA-12541"],
  ] as const)("tags a %s failure with its category", (kind, conclusion) => {
    expect(
      historyPresentation(
        history({ failure_kind: kind, message: "源端：ORA-12541" }),
      ).conclusion,
    ).toBe(conclusion);
  });

  it("shows an unknown category code as-is instead of swallowing it", () => {
    expect(
      historyPresentation(
        history({ failure_kind: "SOMETHING_NEW", message: "目标端：坏了" }),
      ).conclusion,
    ).toBe("[SOMETHING_NEW] 目标端：坏了");
  });

  it("leaves an unclassified old history row untagged", () => {
    expect(
      historyPresentation(
        history({ failure_kind: null, message: "目标端：门禁计数不一致" }),
      ).conclusion,
    ).toBe("目标端：门禁计数不一致");
  });

  it("names the stage in the live conclusion instead of echoing the wire spelling", () => {
    expect(
      historyPresentation(
        history({ outcome: null, finished_at: null, stage: "STREAMING" }),
      ).conclusion,
    ).toBe("进行中 · 传输中");
  });

  it("says a run is only accepted while no stage has been reported", () => {
    expect(
      historyPresentation(
        history({ outcome: null, finished_at: null, stage: null }),
      ).conclusion,
    ).toBe("已受理，正在拉起");
  });

  it("shows an unknown stage as-is instead of swallowing it", () => {
    // Same rule as the failure category above: an unrecognised spelling means
    // the two ends are on different versions, and that belongs on screen.
    expect(
      historyPresentation(
        history({ outcome: null, finished_at: null, stage: "RESUMING" }),
      ).conclusion,
    ).toBe("进行中 · RESUMING");
  });

  it("stops calling a row live once it has a finish time but no outcome", () => {
    // The parent recorded a finish and never folded a verdict. 「进行中」 is a
    // promise that the row will change; this one never will, and calling it
    // live also keeps it under a one-second poll forever.
    const presentation = historyPresentation(
      history({
        outcome: null,
        finished_at: "2026-08-15T10:01:00.000Z",
        stage: "STREAMING",
      }),
    );
    expect(presentation.kind).toBe("unknown");
    expect(presentation.conclusion).toBe("记录不完整，结局未知");
  });

  it("still lets a named unknown reason speak first", () => {
    expect(
      historyPresentation(
        history({
          outcome: null,
          finished_at: null,
          unknown_reason: "SERVICE_RESTARTED",
        }),
      ).conclusion,
    ).toBe("服务重启，结局未知");
  });

  it("explains a missing run id", () => {
    expect(runIdPresentation(history({ run_id: null }))).toBe(
      "未发起，目标端不知道这次运行",
    );
  });
});

describe("the task name a run record carries", () => {
  // 改名不回改历史：这一行说的是当时那次运行（#259）。
  it("shows the name snapshotted at start, not what the task is called now", () => {
    expect(runTaskName(history({ task_name: "订单日增量" }), "订单日增量（已停用）")).toBe(
      "订单日增量",
    );
  });

  it("falls back to the current name only for records older than the field", () => {
    expect(runTaskName(history({ task_name: "" }), "订单日增量")).toBe("订单日增量");
    expect(runTaskName(history({ task_name: "   " }), "订单日增量")).toBe("订单日增量");
    expect(runTaskName({}, "订单日增量")).toBe("订单日增量");
  });
});

describe("runTriggerLabel", () => {
  // 夜里两点那次是自动跑的、还是有人手动补的一次，事后只有这一格答得出来（#266）。
  it("tells a scheduled run apart from one a person pressed", () => {
    expect(runTriggerLabel("MANUAL")).toBe("手动发起");
    expect(runTriggerLabel("SCHEDULED")).toBe("调度发起");
  });

  // 服务端把老历史行一律迁成了 MANUAL，所以缺席只有一种解释：前端比服务端新。
  // 那时候什么都不显示，不拿「手动」去糊一个自己不知道的事实。
  it("shows nothing at all rather than guessing when the column is absent", () => {
    expect(runTriggerLabel(undefined)).toBeNull();
    expect(runTriggerLabel(null)).toBeNull();
    expect(runTriggerLabel("")).toBeNull();
  });

  it("passes a spelling it does not know straight through", () => {
    expect(runTriggerLabel("REPLAYED")).toBe("REPLAYED");
  });
});

describe("failureKindLabel", () => {
  // 「本次跳过」是闭集里唯一一个什么都没做的类目：到点了，上一次还没结束，
  // 于是这一次没发起。它不是一次故障，是那个触发时刻的答案（#266）。
  it("names the occurrence that was skipped rather than failed", () => {
    expect(failureKindLabel("SKIPPED")).toBe("本次跳过");
  });

  it("passes an unknown kind through, because that means source got ahead of web", () => {
    expect(failureKindLabel("BRAND_NEW_KIND")).toBe("BRAND_NEW_KIND");
    expect(failureKindLabel(null)).toBeNull();
  });
});

describe("historyPresentation on a skipped occurrence", () => {
  it("reads as a classified failure with no run id and an untouched target table", () => {
    const skipped: RunHistory = {
      ...baseHistory,
      run_id: null,
      trigger: "SCHEDULED",
      staging_table: null,
      outcome: "FAILED",
      target_table_effect: "DISCARDED",
      stage: null,
      sink_code: null,
      message: "上次尚未结束，本次跳过",
      failure_kind: "SKIPPED",
    };

    const presentation = historyPresentation(skipped);

    expect(presentation.kind).toBe("failed");
    expect(presentation.conclusion).toBe("[本次跳过] 上次尚未结束，本次跳过");
    // 目标表效果那一格是空的：没有 `run_id`，目标端对这一次一无所知。
    expect(presentation.terminalEffect).toBeNull();
    expect(runIdPresentation(skipped)).toBe("未发起，目标端不知道这次运行");
  });
});
