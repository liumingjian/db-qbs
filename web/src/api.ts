export interface TaskInput {
  name: string;
  source_sql: string;
  source_date_col: string;
  target_table: string;
  target_date_col: string;
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

export interface ColumnFetchResult {
  columns: Omit<BuilderColumn, "nullable">[];
  target_ddl: string;
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
    ...overrides,
  };
}

export async function listTasks(): Promise<Task[]> {
  const response = await fetch("/api/tasks", {
    headers: { Accept: "application/json" },
  });
  return readJson<Task[]>(response, "加载任务失败");
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

function postJson<T>(path: string, body: unknown, fallback: string): Promise<T> {
  return fetch(path, {
    method: "POST",
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
  }).then((response) => readJson<T>(response, fallback));
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
