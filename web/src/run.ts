import type { RunDetail } from "./api";
import type { RunPhase } from "./components/DesignSystem";
import { historyPresentation } from "./history";

export type RunPresentationKind =
  | "accepted"
  | "live"
  | "succeeded"
  | "shape-failed"
  | "mapping-failed"
  | "failed"
  | "unknown";

export interface RunPresentation {
  kind: RunPresentationKind;
  phase: RunPhase | null;
  conclusion: string;
  terminalEffect: "SWAPPED" | "DISCARDED" | null;
  error: { code: string; httpStatus: number | null } | null;
  metrics: {
    rows: number;
    sequence: number;
    milliseconds: number;
    bytes: number;
  };
}

export function runPresentation(detail: RunDetail): RunPresentation {
  const metrics = {
    rows: detail.rows_pushed,
    sequence: detail.seq,
    milliseconds: detail.ms,
    bytes: detail.bytes,
  };

  if (detail.live) {
    const phase = runPhase(detail.stage);
    if (phase === null) {
      return {
        kind: "accepted",
        phase: null,
        conclusion: "已受理，正在拉起",
        terminalEffect: null,
        error: null,
        metrics,
      };
    }
    return {
      kind: "live",
      phase,
      conclusion: `运行中 ${phase}`,
      terminalEffect: null,
      error: null,
      metrics,
    };
  }

  const presentation = historyPresentation(detail);
  if (presentation.kind === "unknown") {
    return {
      kind: "unknown",
      phase: null,
      conclusion: presentation.conclusion,
      terminalEffect: null,
      error: null,
      metrics,
    };
  }
  if (detail.run_id === null) {
    return {
      kind: "shape-failed",
      phase: null,
      conclusion: shapeFailureConclusion(detail),
      terminalEffect: null,
      error: null,
      metrics,
    };
  }
  if (detail.sink_code === "PRECHECK_FAILED") {
    return {
      kind: "mapping-failed",
      phase: null,
      conclusion: presentation.conclusion,
      terminalEffect: null,
      error: presentation.error,
      metrics,
    };
  }

  return {
    kind: presentation.kind === "succeeded" ? "succeeded" : "failed",
    phase: null,
    conclusion: presentation.conclusion,
    terminalEffect: presentation.terminalEffect,
    error: presentation.error,
    metrics,
  };
}

/**
 * 形状预检失败的中文人话结论。
 *
 * source 回的 `message` 是英文原文（属于 API 语义，不动），直接拿来当结论条等于没给结论。
 * 这里按映射预检那条（「目标端：映射预检未通过：一次发现 1 项问题，未创建暂存表」）的句式
 * 给源端配一句，并明写目标表未被触碰——形状不过时根本没向 sink 发过请求。
 */
function shapeFailureConclusion(detail: RunDetail & { live: false }): string {
  const failed = detail.shape_checks.filter((check) => !check.passed).length;
  const count = failed === 0 ? "" : `一次发现 ${failed} 项问题，`;
  return `源端：SQL 形状预检未通过：${count}未向 sink 发出请求，未创建暂存表，目标表未被触碰。`;
}

function runPhase(stage: string | null): RunPhase | null {
  switch (stage) {
    case "PREPARING":
    case "STREAMING":
    case "COMMITTING":
      return stage;
    default:
      return null;
  }
}
