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

async function readJson<T>(response: Response, fallback: string): Promise<T> {
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    body = undefined;
  }
  if (!response.ok) {
    const message = errorMessage(body) ?? `${fallback}（HTTP ${response.status}）`;
    throw new Error(message);
  }
  return body as T;
}

function errorMessage(body: unknown): string | undefined {
  if (typeof body !== "object" || body === null || !("error" in body)) {
    return undefined;
  }
  const error = body.error;
  if (typeof error !== "object" || error === null || !("message" in error)) {
    return undefined;
  }
  return typeof error.message === "string" ? error.message : undefined;
}
