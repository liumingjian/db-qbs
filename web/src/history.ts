import type { RunHistory } from "./api";

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
 * 不是一句话。类目词表对齐 STRATEGY-V1 M4「区分 Oracle 连接失败 / dblink 不可用 /
 * 类型映射错 / 网络中断 / MySQL 写入失败 / 校验不通过」那六类。
 * 闭集外的值原样显示，不吞掉——那说明 source 增了分类而这里没跟上。
 */
const FAILURE_KIND_LABELS: Readonly<Record<string, string>> = {
  CONFIG: "配置",
  ORCHESTRATOR: "未拉起",
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

export function runIdPresentation(history: RunHistory): string {
  return history.run_id ?? "未发起，目标端不知道这次运行";
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
    return {
      kind: "live",
      conclusion:
        history.stage === null ? "已受理，正在拉起" : `进行中 ${history.stage}`,
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
 *
 * 形状预检失败不走这里——`runPresentation` 在 web 侧另成一句（`shapeFailureConclusion`）。
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
 * 直接拿来当结论条会让同一个位置一半中文一半英文——形状预检失败那条和映射预检失败那条都是中文。
 * 这里照那两条的句式在 web 侧成文，只说已核实的事：推了多少行、暂存表切没切。
 * 目标端没报出 `SWAPPED` 时不提切换，别替它下结论。
 */
function succeededConclusion(
  history: RunHistory,
  terminalEffect: HistoryPresentation["terminalEffect"],
): string {
  const rows =
    history.sink_reported_rows ?? history.staged_rows ?? history.rows_pushed;
  const swapped =
    terminalEffect === "SWAPPED" ? "，暂存表已切换为目标表" : "";
  return `目标端：运行成功：已推送 ${countFormatter.format(rows)} 行${swapped}。`;
}

function sinkTerminalEffect(
  history: RunHistory,
): HistoryPresentation["terminalEffect"] {
  if (history.run_id === null || history.sink_code === "PRECHECK_FAILED") {
    return null;
  }

  const effect = history.target_table_effect;
  if (effect === "SWAPPED" || effect === "DISCARDED") {
    return effect;
  }
  return null;
}
