import { describe, expect, it } from "vitest";

import { formatRunLogLine, formatRunLogLines } from "./runLogLine";
import type { LogLineView } from "./runLogLine";

/** 时区随机器走，所以时间只验形状；句子才是这个模块的产出。 */
const TIME_SHAPE = /^\d{2}:\d{2}:\d{2}$/;

function line(fields: Record<string, unknown>, seq = 1): LogLineView {
  return formatRunLogLine({
    seq,
    line: JSON.stringify({
      ts: "2026-08-28T01:15:30.000Z",
      level: "info",
      event: "source_started",
      run_id: "run-1",
      task: "/tmp/task.json",
      ...fields,
    }),
  });
}

describe("日志行 → 人话（#263）", () => {
  const cases: ReadonlyArray<{
    name: string;
    fields: Record<string, unknown>;
    event: string;
    text: string;
    tone?: LogLineView["tone"];
  }> = [
    {
      name: "开始",
      fields: { event: "source_started" },
      event: "开始",
      text: "源端进程已启动",
    },
    {
      name: "命令行失败",
      fields: { level: "error", event: "cli_failed", message: "缺少 --config" },
      event: "失败",
      text: "缺少 --config",
      tone: "error",
    },
    {
      name: "源端配置有误",
      fields: { level: "error", event: "source_config_failed", message: "读不到配置文件" },
      event: "失败",
      text: "读不到配置文件",
      tone: "error",
    },
    {
      name: "阶段：机器占位话不复述",
      fields: { event: "stage_changed", stage: "STREAMING", message: "run entered STREAMING" },
      event: "阶段",
      text: "进入传输中",
    },
    {
      name: "阶段：带真正原因时原因留下",
      fields: {
        level: "error",
        event: "stage_changed",
        stage: "FAILED",
        message: "Oracle 游标打不开",
      },
      event: "阶段",
      text: "进入已失败：Oracle 游标打不开",
      tone: "error",
    },
    {
      name: "阶段：认不出的拼写原样念出来",
      fields: { event: "stage_changed", stage: "REHEARSING", message: "run entered REHEARSING" },
      event: "阶段",
      text: "进入REHEARSING",
    },
    {
      name: "计数成功",
      fields: {
        event: "precount_finished",
        total_rows: 128430,
        precount_ms: 1200,
        message: null,
      },
      event: "计数",
      text: "预计 128,430 行（耗时 1.2 秒）",
    },
    {
      name: "计数失败但运行继续",
      fields: {
        level: "warn",
        event: "precount_finished",
        total_rows: null,
        precount_ms: 30000,
        message: "查询超时",
      },
      event: "计数",
      text: "未取到总行数：查询超时",
      tone: "warn",
    },
    {
      name: "开表",
      fields: {
        event: "run_opened",
        staging_table: "stg_orders_1",
        columns_checked: 12,
        message: "sink accepted run and created staging",
      },
      event: "开表",
      text: "目标端已建暂存表 stg_orders_1，校验 12 列",
    },
    {
      name: "推送",
      fields: {
        event: "batch_pushed",
        seq: 3,
        rows: 5000,
        source_rows: 15000,
        bytes: 1258291,
        written: 5000,
        ms: 120,
      },
      event: "推送",
      text: "第 3 批 · 5,000 行 · 1.2 MB · 累计 15,000",
    },
    {
      name: "映射预检不通过",
      fields: {
        level: "error",
        event: "mapping_precheck_failed",
        column: "ORDER_AMT",
        source: "NUMBER(18,4)",
        target: "DECIMAL(10,2)",
        message: "目标端：小数位不足",
        rule: "小数位不足",
        suggestion: "把目标列放宽到 DECIMAL(18,4)",
      },
      event: "映射",
      text:
        "列 ORDER_AMT（NUMBER(18,4) → DECIMAL(10,2)）：目标端：小数位不足，" +
        "建议：把目标列放宽到 DECIMAL(18,4)",
      tone: "error",
    },
    {
      name: "范围校验",
      fields: {
        event: "range_check_executed",
        columns: ["ORDER_AMT", "QTY"],
        scanned_rows: 128430,
        ms: 850,
      },
      event: "校验",
      text: "范围校验 ORDER_AMT、QTY · 扫描 128,430 行 · 850 毫秒",
    },
    {
      name: "提交诊断带终态",
      fields: {
        level: "warn",
        event: "commit_diagnosed",
        terminal: "SWAPPED",
        message: "目标端确认已提交",
      },
      event: "提交",
      text: "目标端确认已提交（已按主键合并进目标表）",
      tone: "warn",
    },
    {
      name: "提交诊断的终态认不出时原样念出来",
      fields: {
        level: "warn",
        event: "commit_diagnosed",
        terminal: "REPLACED",
        message: "目标端确认已提交",
      },
      event: "提交",
      text: "目标端确认已提交（REPLACED）",
      tone: "warn",
    },
    {
      name: "中止未成功",
      fields: { level: "warn", event: "abort_failed", message: "暂存表已被别人锁住" },
      event: "中止",
      text: "中止未成功：暂存表已被别人锁住",
      tone: "warn",
    },
    {
      name: "运行成功",
      fields: {
        event: "run_finished",
        terminal: "SUCCEEDED",
        stage: "SUCCEEDED",
        message: "run completed successfully",
        source_rows: 128430,
        sink_reported_rows: 128430,
        source_batches: 26,
      },
      event: "完成",
      text: "运行成功 · 源端 128,430 行 · 目标端报回 128,430 行 · 26 批",
    },
    {
      name: "运行失败：列与值都摆出来",
      fields: {
        level: "error",
        event: "run_finished",
        terminal: "FAILED",
        stage: "STREAMING",
        message: "超出 DECIMAL(10,2)",
        failure_kind: "SOURCE_VALUE",
        column: "ORDER_AMT",
        value: "12.3456789",
      },
      event: "失败",
      text: '列 ORDER_AMT 的值 "12.3456789"，超出 DECIMAL(10,2)',
      tone: "error",
    },
    {
      name: "运行失败：被截断的值自陈截断过",
      fields: {
        level: "error",
        event: "run_finished",
        terminal: "FAILED",
        message: "超出 VARCHAR(8)",
        column: "REMARK",
        value: "x".repeat(64),
        value_truncated: true,
      },
      event: "失败",
      text: `列 REMARK 的值 "${"x".repeat(64)}"（值已截断），超出 VARCHAR(8)`,
      tone: "error",
    },
    {
      name: "运行失败：没有列与值时只说原因",
      fields: {
        level: "error",
        event: "run_finished",
        terminal: "FAILED",
        message: "目标端连不上",
        column: null,
        value: null,
      },
      event: "失败",
      text: "目标端连不上",
      tone: "error",
    },
  ];

  for (const item of cases) {
    it(item.name, () => {
      const view = line(item.fields);
      expect(view.event).toBe(item.event);
      expect(view.text).toBe(item.text);
      expect(view.tone).toBe(item.tone ?? "info");
      expect(view.known).toBe(true);
      expect(view.time).toMatch(TIME_SHAPE);
    });
  }
});

describe("认不出来的行原样透出，绝不吞掉（#263）", () => {
  it("不认识的事件：事件名照原样，正文是整行原文", () => {
    const raw = JSON.stringify({
      ts: "2026-08-28T01:15:30.000Z",
      level: "warn",
      event: "quantum_shipped",
      run_id: "run-1",
      qubits: 7,
    });
    const view = formatRunLogLine({ seq: 9, line: raw });
    expect(view.known).toBe(false);
    expect(view.event).toBe("quantum_shipped");
    expect(view.text).toBe(raw);
    expect(view.tone).toBe("warn");
    expect(view.time).toMatch(TIME_SHAPE);
  });

  it("认识的事件但字段缺得拼不出句子时，也走原样透出", () => {
    const raw = JSON.stringify({ ts: "2026-08-28T01:15:30.000Z", event: "batch_pushed" });
    const view = formatRunLogLine({ seq: 2, line: raw });
    expect(view.known).toBe(false);
    expect(view.text).toBe(raw);
  });

  it("终态拼写不认识的 run_finished 不被强判成成功或失败", () => {
    const raw = JSON.stringify({
      ts: "2026-08-28T01:15:30.000Z",
      event: "run_finished",
      terminal: "PARTIAL",
    });
    const view = formatRunLogLine({ seq: 3, line: raw });
    expect(view.known).toBe(false);
    expect(view.text).toBe(raw);
  });

  it("根本不是 JSON 的一行照原样摆出来", () => {
    const view = formatRunLogLine({ seq: 4, line: "thread 'main' panicked at src/main.rs:12" });
    expect(view.known).toBe(false);
    expect(view.event).toBe("原文");
    expect(view.text).toBe("thread 'main' panicked at src/main.rs:12");
    expect(view.time).toBe("");
  });

  it("没有 event 字段的 JSON 行同样不被吞掉", () => {
    const raw = JSON.stringify({ ts: "2026-08-28T01:15:30.000Z", message: "hi" });
    const view = formatRunLogLine({ seq: 5, line: raw });
    expect(view.known).toBe(false);
    expect(view.event).toBe("原文");
    expect(view.text).toBe(raw);
  });
});

describe("整页翻译", () => {
  it("按 seq 原序返回，一行不少", () => {
    const views = formatRunLogLines([
      { seq: 1, line: JSON.stringify({ event: "source_started" }) },
      { seq: 2, line: "裸文本" },
    ]);
    expect(views.map((view) => view.seq)).toEqual([1, 2]);
    expect(views[0]?.known).toBe(true);
    expect(views[1]?.known).toBe(false);
  });
});
