import type { RunDetail } from "./api";
import type { RunPhase } from "./components/DesignSystem";
import { historyPresentation } from "./history";

export type RunPresentationKind =
  | "accepted"
  | "live"
  | "succeeded"
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
  // SQL 形状预检那一支已随 ADR-0036 §5 整段取消，这里不再有对应的展示分支：
  // `SHAPE_PRECHECK` 是个不会再产生的分类值（闭集只增不删），落到通用失败一支即可，
  // 结论条前缀由 `failureKindLabel` 给出。
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
