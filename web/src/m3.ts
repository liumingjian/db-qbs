import { ApiError } from "./api";
import type {
  FetchedColumn,
  MappingIssue,
  TargetDdlIssue,
} from "./api";

export type DdlTextSegment =
  | { kind: "text"; value: string }
  | { kind: "placeholder"; value: string };

export interface TargetDdlFailure {
  columns: FetchedColumn[];
  issues: TargetDdlIssue[];
}

export function fetchedColumnType(
  column: Pick<FetchedColumn, "type" | "support">,
): string {
  const marker =
    column.support === "needs_precision"
      ? " [待配精度]"
      : column.support === "unsupported"
        ? " [不支持]"
        : "";
  return `${column.type}${marker}`;
}

/** Keep legacy history rows actionable without deriving a rule in the web client. */
export function mappingSuggestion(
  issue: Pick<MappingIssue, "suggestion">,
): string {
  return issue.suggestion?.trim() || "按规则修正该列";
}

export function ddlTextSegments(ddl: string): DdlTextSegment[] {
  const segments: DdlTextSegment[] = [];
  const placeholders = /<目标表名>|DECIMAL\(<p>,<s>\)/g;
  let textStart = 0;

  for (const match of ddl.matchAll(placeholders)) {
    const index = match.index ?? textStart;
    if (index > textStart) {
      segments.push({ kind: "text", value: ddl.slice(textStart, index) });
    }
    segments.push({ kind: "placeholder", value: match[0] });
    textStart = index + match[0].length;
  }

  if (textStart < ddl.length) {
    segments.push({ kind: "text", value: ddl.slice(textStart) });
  }

  return segments;
}

export function targetDdlFailureFrom(error: unknown): TargetDdlFailure | null {
  if (!(error instanceof ApiError) || !isRecord(error.body)) {
    return null;
  }
  if (error.body.kind !== "target_ddl") {
    return null;
  }

  const columns = error.body.described_columns;
  const issues = error.body.columns;
  if (
    !Array.isArray(columns) ||
    !columns.every(isFetchedColumn) ||
    !Array.isArray(issues) ||
    !issues.every(isTargetDdlIssue)
  ) {
    return null;
  }

  return { columns, issues };
}

function isFetchedColumn(value: unknown): value is FetchedColumn {
  if (!isRecord(value)) {
    return false;
  }
  return (
    typeof value.name === "string" &&
    typeof value.type === "string" &&
    isNullableNumber(value.precision) &&
    isNullableNumber(value.scale) &&
    isNullableNumber(value.length) &&
    (value.fsp === undefined || isNullableNumber(value.fsp)) &&
    (value.support === undefined ||
      value.support === null ||
      value.support === "ok" ||
      value.support === "needs_precision" ||
      value.support === "unsupported")
  );
}

function isTargetDdlIssue(value: unknown): value is TargetDdlIssue {
  return (
    isRecord(value) &&
    typeof value.column === "string" &&
    typeof value.source === "string" &&
    typeof value.message === "string"
  );
}

function isNullableNumber(value: unknown): value is number | null {
  return value === null || typeof value === "number";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
