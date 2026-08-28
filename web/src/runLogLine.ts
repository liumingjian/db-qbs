/**
 * 一行原始运行日志 → 一句人话。**这是整套「人话」唯一的产地**。
 *
 * 进程间的 JSON Lines 契约不动（`crates/shared/src/lib.rs` 的 `LogLine`）：子进程照旧吐
 * 结构化行，父进程照旧折叠，原文照旧原样落库（`crates/source/src/run_log_store.rs`）。
 * **结构留在字段里，好看留在展示层**——让后端直接吐句子，父进程就得拿正则去认句子，
 * 而运行历史会在每次改文案的那一天全线碎掉。
 *
 * 两条规矩，比这张表本身重要：
 *
 * 1. **认不出来的事件按原样透出，绝不吞掉。** 一个不认识的 `event` 意味着两端跑着不同的
 *    版本，那恰恰是最该出现在屏幕上的事。同一条规矩已经写在 `stageLabel`
 *    （`runStage.ts`）和 `failureKindLabel`（`history.ts`）里，这里是第三处。
 * 2. **不据此做任何判断。** 这里只产出给人看的字符串：不改状态、不推结论、不影响轮询。
 *    所以「显示一个看不懂的词」的代价只有一个看不懂的词。
 *
 * 字段缺了、类型不对，也走同一条路：能拼出多少拼多少，拼不出就把原文摆出来。
 * 半句编出来的话比一行看得懂的 JSON 有害得多。
 */

import { formatTimestamp } from "./history";
import { stageLabel } from "./runStage";

/** 服务端 `GET /api/runs/{}/logs` 返回的一行：序号（游标本身）+ 原文。 */
export interface RunLogLine {
  seq: number;
  line: string;
}

/** 屏幕上的一行：时间 · 事件 · 说明。 */
export interface LogLineView {
  seq: number;
  /** 本地时间 `HH:MM:SS`。取不到 `ts` 时是空串——空着比编一个时间诚实。 */
  time: string;
  /** 事件的中文短名；认不出来时就是原文里那个拼写。 */
  event: string;
  /** 说明。认不出来的事件这里是**整行原文**。 */
  text: string;
  tone: "info" | "warn" | "error";
  /** 这一行是不是被认出来了。`false` 的行在界面上会自陈「未知事件」。 */
  known: boolean;
}

const countFormatter = new Intl.NumberFormat("zh-CN");

/** 认不出来的行摆在「原文」名下：它不是某个事件，它就是它自己。 */
const RAW_LABEL = "原文";

/**
 * 通用 `stage_changed` 那句机器话。子进程用它当占位符，屏幕上不必再念一遍
 * 「run entered STREAMING」——阶段名本身已经说完了。只对**完全相等**的串生效，
 * 不做前缀猜测：失败时同一个事件带的是一句真正的中文原因，那一句必须留下。
 */
function isBoilerplateStageMessage(message: string | null, stage: string | null): boolean {
  return stage !== null && message === `run entered ${stage}`;
}

const TERMINAL_LABELS: Readonly<Record<string, string>> = {
  SWAPPED: "已按主键合并进目标表",
  DISCARDED: "已丢弃暂存表",
};

export function formatRunLogLine(entry: RunLogLine): LogLineView {
  const parsed = parseObject(entry.line);
  if (parsed === null) {
    // JSON 都不是的一行也照存照显：那多半是子进程崩溃时打到 stderr/stdout 上的
    // 一段裸文本，而那正是最需要被看见的一段。
    return {
      seq: entry.seq,
      time: "",
      event: RAW_LABEL,
      text: entry.line,
      tone: "info",
      known: false,
    };
  }

  const time = timeOf(text(parsed, "ts"));
  const tone = toneOf(text(parsed, "level"));
  const event = text(parsed, "event");
  const rendered = event === null ? null : render(event, parsed);
  if (rendered === null) {
    return {
      seq: entry.seq,
      time,
      event: event ?? RAW_LABEL,
      text: entry.line,
      tone,
      known: false,
    };
  }
  return { seq: entry.seq, time, event: rendered.event, text: rendered.text, tone, known: true };
}

/** 一次把一页原文翻成人话。顺序即 `seq` 顺序，服务端已经排好。 */
export function formatRunLogLines(entries: readonly RunLogLine[]): LogLineView[] {
  return entries.map(formatRunLogLine);
}

interface Rendered {
  event: string;
  text: string;
}

/**
 * 事件表。返回 `null` 表示「这一行我认不出来」——调用方随即把原文原样摆出去。
 *
 * 只覆盖 source 子进程会吐的那些事件。`sink_started` / `sink_unavailable` /
 * `http_response_failed` 写在目标端自己的标准输出里，到不了这张表存的那条流；
 * 万一哪天到了，它们也走原样透出这条路，这是对的。
 */
function render(event: string, fields: Record<string, unknown>): Rendered | null {
  const message = text(fields, "message");
  switch (event) {
    case "source_started":
      return { event: "开始", text: "源端进程已启动" };

    case "cli_failed":
      return { event: "失败", text: message ?? "命令行参数无效" };

    case "source_config_failed":
      return fallback("失败", message, "源端配置有误");

    case "task_config_failed":
      return fallback("失败", message, "任务定义有误");

    case "stage_changed": {
      const stage = text(fields, "stage");
      if (stage === null) {
        return null;
      }
      const label = `进入${stageLabel(stage)}`;
      return {
        event: "阶段",
        text:
          message === null || isBoilerplateStageMessage(message, stage)
            ? label
            : `${label}：${message}`,
      };
    }

    case "precount_finished": {
      const total = count(fields, "total_rows");
      const elapsed = count(fields, "precount_ms");
      if (total === null) {
        // 计数失败不中断运行（ADR-0043 §7）：这一行说的是「进度没有分母了」，
        // 不是「这次搬运完了」。
        return fallback("计数", message === null ? null : `未取到总行数：${message}`, "未取到总行数");
      }
      const spent = elapsed === null ? "" : `（耗时 ${duration(elapsed)}）`;
      return { event: "计数", text: `预计 ${countFormatter.format(total)} 行${spent}` };
    }

    case "run_opened": {
      const staging = text(fields, "staging_table");
      if (staging === null) {
        return null;
      }
      const columns = count(fields, "columns_checked");
      const checked = columns === null ? "" : `，校验 ${countFormatter.format(columns)} 列`;
      return { event: "开表", text: `目标端已建暂存表 ${staging}${checked}` };
    }

    case "batch_pushed": {
      const seq = count(fields, "seq");
      const rows = count(fields, "rows");
      if (seq === null || rows === null) {
        return null;
      }
      const parts = [`第 ${countFormatter.format(seq)} 批`, `${countFormatter.format(rows)} 行`];
      const bytes = count(fields, "bytes");
      if (bytes !== null) {
        parts.push(size(bytes));
      }
      const cumulative = count(fields, "source_rows");
      if (cumulative !== null) {
        parts.push(`累计 ${countFormatter.format(cumulative)}`);
      }
      return { event: "推送", text: parts.join(" · ") };
    }

    case "mapping_precheck_failed": {
      const column = text(fields, "column");
      if (column === null) {
        return null;
      }
      const source = text(fields, "source");
      const target = text(fields, "target");
      const types = source !== null && target !== null ? `（${source} → ${target}）` : "";
      const reason = message === null ? "类型映射不通过" : message;
      const suggestion = text(fields, "suggestion");
      const advice = suggestion === null ? "" : `，建议：${suggestion}`;
      return { event: "映射", text: `列 ${column}${types}：${reason}${advice}` };
    }

    case "range_check_executed": {
      const columns = list(fields, "columns");
      const scanned = count(fields, "scanned_rows");
      if (columns === null || scanned === null) {
        return null;
      }
      const parts = [
        `范围校验 ${columns.length === 0 ? "0 列" : columns.join("、")}`,
        `扫描 ${countFormatter.format(scanned)} 行`,
      ];
      const elapsed = count(fields, "ms");
      if (elapsed !== null) {
        parts.push(duration(elapsed));
      }
      return { event: "校验", text: parts.join(" · ") };
    }

    case "commit_diagnosed": {
      const terminal = text(fields, "terminal");
      // 终态拼写不认识就原样念出来——#264 会往这张表里加 `REPLACED`，在它到来之前
      // 屏幕上出现的是那个词本身，而不是一句被抹平的空话。
      const effect = terminal === null ? "" : `（${TERMINAL_LABELS[terminal] ?? terminal}）`;
      return fallback("提交", message === null ? null : `${message}${effect}`, "提交结果待判定");
    }

    case "abort_failed":
      return fallback("中止", message === null ? null : `中止未成功：${message}`, "中止未成功");

    case "run_finished": {
      const terminal = text(fields, "terminal");
      if (terminal === "SUCCEEDED") {
        const parts = ["运行成功"];
        const sourceRows = count(fields, "source_rows");
        if (sourceRows !== null) {
          parts.push(`源端 ${countFormatter.format(sourceRows)} 行`);
        }
        const written = count(fields, "sink_reported_rows");
        if (written !== null) {
          parts.push(`目标端报回 ${countFormatter.format(written)} 行`);
        }
        const batches = count(fields, "source_batches");
        if (batches !== null) {
          parts.push(`${countFormatter.format(batches)} 批`);
        }
        return { event: "完成", text: parts.join(" · ") };
      }
      if (terminal === "FAILED") {
        return { event: "失败", text: failureText(fields, message) };
      }
      // 终态是别的拼写：认不出来，交给原样透出。
      return null;
    }

    default:
      return null;
  }
}

/**
 * 失败那一句。**业务值带引号且自陈是否被截断**：落库前 `value` 已按 64 字符截过
 * （`run_log_store.rs` 的 `truncate_business_values`），把半截值当完整值念出来，
 * 会让人拿着一个不存在的值去源库里找。
 */
function failureText(fields: Record<string, unknown>, message: string | null): string {
  const column = text(fields, "column");
  const value = text(fields, "value");
  const reason = message ?? "运行失败";
  if (column === null || value === null) {
    return reason;
  }
  const truncated = fields["value_truncated"] === true ? "（值已截断）" : "";
  return `列 ${column} 的值 "${value}"${truncated}，${reason}`;
}

function fallback(event: string, sentence: string | null, spare: string): Rendered {
  return { event, text: sentence ?? spare };
}

function parseObject(line: string): Record<string, unknown> | null {
  try {
    const parsed: unknown = JSON.parse(line);
    return typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

function text(fields: Record<string, unknown>, key: string): string | null {
  const value = fields[key];
  return typeof value === "string" ? value : null;
}

function count(fields: Record<string, unknown>, key: string): number | null {
  const value = fields[key];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function list(fields: Record<string, unknown>, key: string): string[] | null {
  const value = fields[key];
  return Array.isArray(value) && value.every((item) => typeof item === "string")
    ? (value as string[])
    : null;
}

function timeOf(ts: string | null): string {
  if (ts === null) {
    return "";
  }
  const formatted = formatTimestamp(ts);
  return formatted === "—" ? "" : formatted;
}

function toneOf(level: string | null): LogLineView["tone"] {
  return level === "warn" || level === "error" ? level : "info";
}

/** 字节数按 1024 进位，保留一位小数；不足 1 KB 的直接写 B。 */
function size(bytes: number): string {
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${Math.round(value)} B` : `${value.toFixed(1)} ${units[unit]}`;
}

/** 不足一秒写毫秒：一条 300 毫秒的批次写成「0.3 秒」会把量级读没。 */
function duration(ms: number): string {
  return ms < 1000 ? `${Math.round(ms)} 毫秒` : `${(ms / 1000).toFixed(1)} 秒`;
}
