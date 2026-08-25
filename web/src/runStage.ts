// The five states a Run passes through, on this side of the wire.
//
// The other adapter is `crates/shared/src/run_stage.rs`; the seam between them
// is the `stage` field's spelling in a Run Log line, which is a contract and
// does not change. Everything a screen needs to know about a stage is decided
// here: what it is called in Chinese, whether the run can still be stopped, and
// why not when it cannot.
//
// Before this module the answers were scattered: `history.ts` interpolated the
// raw wire spelling into a sentence, `run.ts` narrowed three of the five and
// silently dropped the rest, `DesignSystem.tsx` kept its own label table, and
// the stop rule existed only on the server, where the screen learned it by
// clicking and reading a 409.

/** The wire spelling. Identical to `RunStage::as_str` on the Rust side. */
export type RunStage =
  | "PREPARING"
  | "STREAMING"
  | "COMMITTING"
  | "SUCCEEDED"
  | "FAILED";

/**
 * The three the run is *doing something* in, in the order it walks them.
 *
 * The phase line draws these and only these, ending in 「终态待定」: the two
 * terminal stages are not another step along the same track, they are the track
 * finishing, and the run's outcome says which way.
 */
export type RunPhase = "PREPARING" | "STREAMING" | "COMMITTING";

export const RUN_PHASES: readonly RunPhase[] = [
  "PREPARING",
  "STREAMING",
  "COMMITTING",
];

const LABELS: Readonly<Record<RunStage, string>> = {
  PREPARING: "准备中",
  STREAMING: "传输中",
  COMMITTING: "提交中",
  SUCCEEDED: "已完成",
  FAILED: "已失败",
};

function known(raw: string | null): RunStage | null {
  return raw !== null && raw in LABELS ? (raw as RunStage) : null;
}

/** The three-stage subset, for the phase line. `null` for everything else. */
export function runPhase(raw: string | null): RunPhase | null {
  return RUN_PHASES.find((phase) => phase === raw) ?? null;
}

/**
 * What a person should see for this stage, or `null` when there is no stage.
 *
 * **A spelling we do not recognise is shown as it arrived, not swallowed** —
 * the same rule `failureKindLabel` follows, for the same reason: an unknown
 * value means the two ends are on different versions, and that is exactly the
 * moment you want it on screen rather than smoothed away. Nothing is decided
 * off this value, so showing it raw costs nothing but a strange-looking word.
 */
export function stageLabel(raw: RunStage): string;
export function stageLabel(raw: string | null): string | null;
export function stageLabel(raw: string | null): string | null {
  if (raw === null) {
    return null;
  }
  return known(raw) === null ? raw : LABELS[raw as RunStage];
}

/**
 * Whether `source` can still stop this run — the same rule as
 * `RunStage::abort_allowed`, evaluated a second time so the screen can grey the
 * button *before* the round trip instead of after a 409.
 *
 * `null` (the run is accepted but the child has not reported a stage yet) and
 * an unrecognised spelling both answer **no**. Both mean "I do not know what
 * this run is doing", and the server refuses on both, so a lit button would be
 * a promise the next click breaks.
 */
export function abortAllowed(raw: string | null): boolean {
  const stage = known(raw);
  return stage === "PREPARING" || stage === "STREAMING";
}

/**
 * Why the stop button is greyed, in one clause, or `null` when it is not.
 *
 * Three different reasons, deliberately not collapsed into one: at
 * `COMMITTING` the **permission** is gone, at a terminal stage the **process**
 * is gone, and before the first stage there is nothing to stop yet. Only the
 * first is a rule worth explaining; the other two are just facts.
 */
export function abortRefusal(raw: string | null): string | null {
  if (abortAllowed(raw)) {
    return null;
  }
  switch (known(raw)) {
    case "COMMITTING":
      return "已过封口点：暂存表的处置权已经交给目标端";
    case "SUCCEEDED":
    case "FAILED":
      return "运行已经结束，没有可停的进程";
    default:
      return "运行还没拉起来，暂时停不了";
  }
}
