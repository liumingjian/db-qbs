import type { RunHistory } from "./api";
import { historyPresentation } from "./history";
import type { Step } from "./wizard";

export type RowRunAction =
  | { kind: "start"; disabled: boolean }
  | { kind: "stop"; runRecordId: string };

export function rowRunAction(
  run: RunHistory | undefined,
  startBusy: boolean,
): RowRunAction {
  if (run !== undefined && historyPresentation(run).kind === "live") {
    return { kind: "stop", runRecordId: run.run_record_id };
  }
  return { kind: "start", disabled: startBusy };
}

export interface Remediation {
  step: Step;
  label: string;
}

const SOURCE_FAILURES = new Set([
  "CONFIG",
  "SOURCE_CONNECT",
  "SOURCE_DBLINK",
  "SOURCE_QUERY",
  "SOURCE_VALUE",
]);
const MAPPING_FAILURES = new Set(["SHAPE_PRECHECK", "MAPPING_PRECHECK"]);

export function remediationFor(run: RunHistory): Remediation | null {
  if (historyPresentation(run).kind !== "failed" || run.failure_kind === null) {
    return null;
  }
  if (SOURCE_FAILURES.has(run.failure_kind)) {
    return { step: 2, label: "修改源端与取数设置" };
  }
  if (MAPPING_FAILURES.has(run.failure_kind)) {
    return { step: 3, label: "修改字段映射" };
  }
  return null;
}
