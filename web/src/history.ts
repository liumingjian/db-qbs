import type { RunHistory } from "./api";
import { stageLabel } from "./runStage";

export interface HistoryPresentation {
  kind: "live" | "succeeded" | "failed" | "unknown";
  conclusion: string;
  terminalEffect: "SWAPPED" | "DISCARDED" | null;
  error: { code: string; httpStatus: number | null } | null;
}

const ERROR_HTTP_STATUS: Readonly<Record<string, number>> = {
  PRECHECK_FAILED: 422,
  SEQ_MISMATCH: 409,
  RUN_SEALED: 409,
  RUN_UNKNOWN: 404,
  VERIFY_FAILED: 409,
  SWAP_FAILED: 500,
  SWAP_TARGET_BUSY: 409,
  DATA_REJECTED: 400,
  SINK_ENVIRONMENT: 500,
  BATCH_WRITE_FAILED: 500,
  INTERNAL_PRECHECK_ESCAPE: 500,
  INTERNAL_ASSERTION_FAILED: 500,
  PAYLOAD_TOO_LARGE: 413,
  BAD_REQUEST: 400,
};

const countFormatter = new Intl.NumberFormat("zh-CN");

/**
 * 失败分类的中文短名（ADR-0029 §3）。
 *
 * 它加在结论条最前面，作用是**给这次失败归类**，不是复述人话——所以是方括号里的类目名，
 * 不是一句话。类目词表对齐 M4「区分 Oracle 连接失败 / dblink 不可用 /
 * 类型映射错 / 网络中断 / MySQL 写入失败 / 校验不通过」那六类。
 * 闭集外的值原样显示，不吞掉——那说明 source 增了分类而这里没跟上。
 */
const FAILURE_KIND_LABELS: Readonly<Record<string, string>> = {
  CONFIG: "配置",
  ORCHESTRATOR: "未拉起",
  // 已退役、不会再产生（ADR-0036 §5 取消了形状预检）；闭集只增不删，标签跟着留。
  SHAPE_PRECHECK: "SQL 形状",
  SOURCE_CONNECT: "Oracle 连接",
  SOURCE_DBLINK: "dblink",
  SOURCE_QUERY: "源端查询",
  SOURCE_VALUE: "源端值",
  MAPPING_PRECHECK: "类型映射",
  NETWORK: "网络中断",
  SINK_WRITE: "MySQL 写入",
  DATA_REJECTED: "数据被拒",
  SINK_ENVIRONMENT: "目标端环境",
  TARGET_BUSY: "目标表占用",
  VERIFY_FAILED: "校验门禁",
  DEFECT: "程序缺陷",
  UNKNOWN: "结局未知",
};

export function failureKindLabel(kind: string | null): string | null {
  if (kind === null) {
    return null;
  }
  return FAILURE_KIND_LABELS[kind] ?? kind;
}

const UNKNOWN_CONCLUSIONS: Readonly<
  Record<NonNullable<RunHistory["unknown_reason"]>, string>
> = {
  PROCESS_DISAPPEARED: "进程消失，无终态日志",
  SERVICE_RESTARTED: "服务重启，结局未知",
};

/**
 * 「目标端运行号」栏位的显示值（走查 V15）。
 *
 * 没有 `run_id` 时写的是**一句话，不是空白也不是横杠**：这一次根本没走到向 sink 发请求那步，
 * 目标端对它一无所知。空白会被读成「漏渲染」，横杠会被读成「有但没给」，
 * 两者都不如把事实说出来。`run_record_id` 与 `run_id` **谁也不替代谁**（原 V14，现由 V15 兼守）。
 */
export function runIdPresentation(history: { run_id: string | null }): string {
  return history.run_id ?? "未发起，目标端不知道这次运行";
}

/**
 * 一条运行记录顶上该写哪个名字（#259）。
 *
 * 任务名是展示标签，在向导里随时可以改。运行记录认领任务靠 `task_id`，名字则在开跑那一刻
 * 快照进这一行——否则改一次名，屏幕上**过去每一次**运行都会跟着改名，而那些运行当时叫的
 * 是别的名字。快照优先；只有空串（早于这个字段的老记录）才回退到任务当前的名字，
 * 因为那时确实没有别的可说，用当前名总好过一片空白。
 */
export function runTaskName(run: { task_name?: string }, currentName: string): string {
  const snapshot = (run.task_name ?? "").trim();
  return snapshot === "" ? currentName : run.task_name!;
}

export function historyPresentation(history: RunHistory): HistoryPresentation {
  if (history.unknown_reason !== null) {
    return {
      kind: "unknown",
      conclusion: UNKNOWN_CONCLUSIONS[history.unknown_reason],
      terminalEffect: null,
      error: null,
    };
  }

  const code = history.sink_code;
  const error =
    code === null
      ? null
      : { code, httpStatus: ERROR_HTTP_STATUS[code] ?? null };
  const terminalEffect = sinkTerminalEffect(history);

  if (history.outcome === null) {
    // 记下了结束时间却没折出结局，说明父进程漏了一笔。**这种行不算在跑**：
    // 「进行中」是一个会自己变的承诺，屏幕上转着圈就是在说「等一下就有答案」，
    // 而它永远不会有答案——那样它会被每秒轮询到天荒地老。「结局不明」本来
    // 就是为「我不知道」准备的那一格，这行正是它。
    if (history.finished_at !== null) {
      return {
        kind: "unknown",
        conclusion: "记录不完整，结局未知",
        terminalEffect: null,
        error: null,
      };
    }
    const label = stageLabel(history.stage);
    return {
      kind: "live",
      conclusion: label === null ? "已受理，正在拉起" : `进行中 · ${label}`,
      terminalEffect,
      error,
    };
  }
  if (history.outcome === "SUCCEEDED") {
    return {
      kind: "succeeded",
      conclusion: succeededConclusion(history, terminalEffect),
      terminalEffect,
      error,
    };
  }
  return {
    kind: "failed",
    conclusion: failureConclusion(history),
    terminalEffect,
    error,
  };
}

/**
 * 失败结论条 = 分类标签 + 原有人话。
 *
 * 人话本来就点名到列与值（ADR-0017 §4），缺的是**这属于哪一类失败**：`sink_code` 只有
 * 目标端路径才有，源端连接失败、dblink 不可用、传输中断这三类过去在界面上一个码都没有，
 * 只能整句读完才知道是哪一侧坏了。分类标签补的就是这一格。
 */
function failureConclusion(history: RunHistory): string {
  const message = history.message ?? "运行失败";
  const label = failureKindLabel(history.failure_kind);
  return label === null ? message : `[${label}] ${message}`;
}

/**
 * 运行成功的中文人话结论。
 *
 * source 回的 `message` 是英文原文（`run completed successfully`，属于 API 语义，不动），
 * 直接拿来当结论条会让同一个位置一半中文一半英文——映射预检失败那条是中文。
 * 这里照那条的句式在 web 侧成文，只说已核实的事：推了多少行、目标端认没认这次写入。
 * 目标端没报出 `SWAPPED` 时什么都不多说，别替它下结论。
 *
 * **`SWAPPED` 不是「整表换过」**（2026-08 UX 评审 P0-1）。sink 打的是
 * `INSERT ... ON DUPLICATE KEY UPDATE`（`crates/sink/src/mysql_destination.rs`），
 * 按主键合并：新增和变更进目标表，**源端删掉的行不会跟着消失**（CONTEXT.md「刻意欠的债」）。
 * 原话「暂存表已切换为目标表」描述的是一次没发生过的切换——照字面读会以为目标表此刻
 * 等于源端那一份，于是拿它当全量快照用。措辞改成合并，债照旧记在 CONTEXT.md 上。
 */
function succeededConclusion(
  history: RunHistory,
  terminalEffect: HistoryPresentation["terminalEffect"],
): string {
  const rows =
    history.sink_reported_rows ?? history.staged_rows ?? history.rows_pushed;
  const merged =
    terminalEffect === "SWAPPED" ? "，已按主键合并进目标表" : "";
  return `目标端：运行成功：已推送 ${countFormatter.format(rows)} 行${merged}。`;
}

function sinkTerminalEffect(
  history: RunHistory,
): HistoryPresentation["terminalEffect"] {
  if (
    history.run_id === null ||
    history.staging_table === null ||
    history.sink_code === "PRECHECK_FAILED"
  ) {
    return null;
  }

  const effect = history.target_table_effect;
  if (effect === "SWAPPED" || effect === "DISCARDED") {
    return effect;
  }
  return null;
}

/**
 * 列表屏共用的时间戳格式化。
 *
 * 它原来长在 `HistoryScreen.tsx` 里，任务屏的「最近运行」要显示同一种时间——
 * 两处各写一份的后果是同一个时刻在两屏上长得不一样。**一个字都没改**，只换了住处。
 * 解析不出来的值**原样回显**，不吞成「—」：那说明服务端给的不是时间戳，得看得见。
 */
export function formatTimestamp(value: string | null, includeDate = false): string {
  if (value === null) {
    return "—";
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return new Intl.DateTimeFormat("zh-CN", {
    year: includeDate ? "numeric" : undefined,
    month: includeDate ? "2-digit" : undefined,
    day: includeDate ? "2-digit" : undefined,
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(date);
}
