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
      conclusion: history.message ?? "运行成功",
      terminalEffect,
      error,
    };
  }
  return {
    kind: "failed",
    conclusion: history.message ?? "运行失败",
    terminalEffect,
    error,
  };
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
