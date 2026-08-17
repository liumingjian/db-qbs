export type ColumnPrecision = Record<string, [number, number]>;

export interface TaskInput {
  name: string;
  source_sql: string;
  source_date_col: string;
  target_table: string;
  target_date_col: string;
  column_precision?: ColumnPrecision;
}

export interface Task extends TaskInput {
  task_id: string;
}

export type TaskDefinition = Omit<TaskInput, "name">;

export interface BuilderTable {
  owner: string;
  name: string;
}

export interface BuilderColumn {
  name: string;
  data_type: string;
  precision: number | null;
  scale: number | null;
  length: number | null;
  nullable: boolean;
}

export interface BuilderSelection {
  dblink: string;
  owner: string;
  table: string;
  columns: string[];
  source_date_col: string;
  target_table: string;
  target_date_col: string;
}

export interface ShapeCheck {
  rule: string;
  passed: boolean;
  message: string;
}

export interface MappingIssue {
  column: string | null;
  source: string | null;
  target: string | null;
  rule: string | null;
  message: string | null;
}

export interface ColumnFetchResult {
  columns: Omit<BuilderColumn, "nullable">[];
  target_ddl: string;
}

export interface RunHistory {
  run_record_id: string;
  run_id: string | null;
  task_id: string;
  biz_date: string;
  staging_table: string | null;
  started_at: string;
  finished_at: string | null;
  outcome: "SUCCEEDED" | "FAILED" | null;
  target_table_effect: "SWAPPED" | "DISCARDED" | "UNKNOWN" | null;
  stage: string | null;
  source_rows: number | null;
  staged_rows: number | null;
  sink_reported_rows: number | null;
  purged_rows: number | null;
  source_batches: number | null;
  received_batches: number | null;
  fetch_ms: number | null;
  push_ms: number | null;
  commit_ms: number | null;
  count_ms: number | null;
  cursor_ms: number | null;
  source_code: string | null;
  sink_code: string | null;
  column: string | null;
  value: string | null;
  message: string | null;
  unknown_reason: "PROCESS_DISAPPEARED" | "SERVICE_RESTARTED" | null;
  seq: number;
  rows_pushed: number;
  bytes: number;
  ms: number;
  last_ts: string | null;
  shape_checks: ShapeCheck[];
  mapping_issues: MappingIssue[];
}

export interface LiveRunDetail {
  run_record_id: string;
  run_id: string | null;
  biz_date: string | null;
  staging_table: string | null;
  stage: string | null;
  seq: number;
  rows_pushed: number;
  bytes: number;
  ms: number;
  last_ts: string | null;
  live: true;
}

export type RunDetail = LiveRunDetail | (RunHistory & { live: false });

export interface RunHistoryFilters {
  taskId?: string;
  bizDate?: string;
}

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly body: unknown,
  ) {
    super(message);
  }
}

export function taskInputFrom(
  task: TaskInput,
  overrides: Partial<TaskInput> = {},
): TaskInput {
  return {
    name: task.name,
    source_sql: task.source_sql,
    source_date_col: task.source_date_col,
    target_table: task.target_table,
    target_date_col: task.target_date_col,
    ...(task.column_precision === undefined
      ? {}
      : { column_precision: task.column_precision }),
    ...overrides,
  };
}

export async function listTasks(): Promise<Task[]> {
  const response = await fetch("/api/tasks", {
    headers: { Accept: "application/json" },
  });
  return readJson<Task[]>(response, "加载任务失败");
}

export async function listRunHistory(
  filters: RunHistoryFilters = {},
): Promise<RunHistory[]> {
  const query = new URLSearchParams();
  if (filters.taskId !== undefined && filters.taskId !== "") {
    query.set("task_id", filters.taskId);
  }
  if (filters.bizDate !== undefined && filters.bizDate !== "") {
    query.set("biz_date", filters.bizDate);
  }
  const suffix = query.size === 0 ? "" : `?${query.toString()}`;
  const response = await fetch(`/api/runs${suffix}`, {
    headers: { Accept: "application/json" },
  });
  return readJson<RunHistory[]>(response, "加载运行历史失败");
}

export async function startRun(
  taskId: string,
  bizDate: string,
): Promise<{ run_record_id: string }> {
  return postJson<{ run_record_id: string }>(
    "/api/runs",
    { task_id: taskId, biz_date: bizDate },
    "发起运行失败",
  );
}

export async function fetchRun(runRecordId: string): Promise<RunDetail> {
  const response = await fetch(
    `/api/runs/${encodeURIComponent(runRecordId)}`,
    { headers: { Accept: "application/json" } },
  );
  return readJson<RunDetail>(response, "读取运行详情失败");
}

export async function cancelRun(
  runRecordId: string,
): Promise<{ message: string }> {
  return postJson<{ message: string }>(
    `/api/runs/${encodeURIComponent(runRecordId)}/cancel`,
    {},
    "取消运行失败",
  );
}

export async function createTask(input: TaskInput): Promise<Task> {
  const response = await fetch("/api/tasks", {
    method: "POST",
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
    },
    body: JSON.stringify(taskInputFrom(input)),
  });
  return readJson<Task>(response, "新建任务失败");
}

export async function updateTask(taskId: string, input: TaskInput): Promise<Task> {
  const response = await fetch(`/api/tasks/${encodeURIComponent(taskId)}`, {
    method: "PUT",
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
    },
    body: JSON.stringify(taskInputFrom(input)),
  });
  return readJson<Task>(response, "更新任务失败");
}

export async function deleteTask(taskId: string): Promise<Task> {
  const response = await fetch(`/api/tasks/${encodeURIComponent(taskId)}`, {
    method: "DELETE",
    headers: { Accept: "application/json" },
  });
  return readJson<Task>(response, "删除任务失败");
}

export async function fetchBuilderTables(dblink: string): Promise<BuilderTable[]> {
  return postJson<BuilderTable[]>("/api/builder/tables", { dblink }, "读取 Oracle 表失败");
}

export async function fetchBuilderColumns(input: {
  dblink: string;
  owner: string;
  table: string;
}): Promise<BuilderColumn[]> {
  return postJson<BuilderColumn[]>("/api/builder/columns", input, "读取 Oracle 列失败");
}

export async function generateBuilderTask(
  input: BuilderSelection,
): Promise<TaskDefinition> {
  return postJson<TaskDefinition>("/api/builder/sql", input, "生成 SQL 失败");
}

export async function inspectSqlShape(
  input: TaskDefinition,
): Promise<ShapeCheck[]> {
  const response = await postJson<{ checks: ShapeCheck[] }>(
    "/api/sql-shape",
    input,
    "检查 SQL 形状失败",
  );
  return response.checks;
}

export async function fetchColumns(
  input: TaskDefinition,
): Promise<ColumnFetchResult> {
  return postJson<ColumnFetchResult>("/api/columns", input, "取列失败");
}

async function postJson<T>(path: string, body: unknown, fallback: string): Promise<T> {
  const response = await fetch(path, {
    method: "POST",
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
  });
  return readJson<T>(response, fallback);
}

async function readJson<T>(response: Response, fallback: string): Promise<T> {
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    body = undefined;
  }
  if (!response.ok) {
    const message = errorMessage(body) ?? `${fallback}（HTTP ${response.status}）`;
    throw new ApiError(message, response.status, body);
  }
  return body as T;
}

function errorMessage(body: unknown): string | undefined {
  if (typeof body !== "object" || body === null) {
    return undefined;
  }
  if ("message" in body && typeof body.message === "string") {
    return body.message;
  }
  if (!("error" in body)) {
    return undefined;
  }
  const error = body.error;
  if (typeof error !== "object" || error === null || !("message" in error)) {
    return undefined;
  }
  return typeof error.message === "string" ? error.message : undefined;
}
