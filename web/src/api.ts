export type ColumnPrecision = Record<string, [number, number]>;

/** 比较符只有这三个（ADR-0035 §3 字面）：`>` / `<` / `=`。 */
export type Comparison = "gt" | "lt" | "eq";

/**
 * 条件值怎么绑进 SQL。**不是源列的 Oracle 类型**——describe 回来的类型不许进任务定义
 * （ADR-0036 §6），所以这是用户在构建器里就该列做的声明：按字典 `DATA_TYPE` 预填、可改。
 */
export type ValueType = "text" | "number" | "date";

/** 值来源二选一（ADR-0035 §3）：建任务时写死，或发起运行时逐条填。 */
export type ValueSource = "constant" | "runtime";

export type Direction = "asc" | "desc";

export interface Condition {
  column: string;
  operator: Comparison;
  value_type: ValueType;
  /** 绑定变量名，规格内唯一，由用户拥有——运行参数与运行历史都拿它当键。 */
  parameter: string;
  value_source: ValueSource;
  /** 仅 `value_source === "constant"` 时有意义；运行时填的条件必须留空。 */
  constant: string;
}

export interface OrderTerm {
  column: string;
  direction: Direction;
}

/**
 * 任务定义的**结构化规格**（ADR-0036 §1），唯一真相源。
 *
 * SQL 不在里面：它由规格现算，界面上只读，v1 没有编辑入口。
 */
export interface ColumnMapping {
  source: string;
  /** 目标列名，默认预填 = `source`（ADR-0038 §2）。落到 SQL 上就是投影的别名。 */
  target: string;
}

export interface TaskSpec {
  dblink?: string;
  owner: string;
  table: string;
  target_table: string;
  /**
   * upsert 的去重键，必选（所有者 2026-08-18 裁定）。存的是**目标列名**（ADR-0038 §2/§6），
   * 与 `columns[].target` 同一个名字空间。
   */
  primary_key: string[];
  columns: ColumnMapping[];
  conditions: Condition[];
  order_by: OrderTerm[];
}

/**
 * 数据源（ADR-0037）。**响应里永远没有 `password`，连密文都没有**（§5）——
 * 界面上只看得到 `has_password` 的「已设置 / 未设置」。
 */
export type DatasourceKind = "oracle" | "mysql";

export interface OracleDatasourceView {
  kind: "oracle";
  connect_string: string;
  username: string;
  has_password: boolean;
}

export interface MysqlDatasourceView {
  kind: "mysql";
  host: string;
  port: number;
  username: string;
  database: string;
  has_password: boolean;
}

export type Datasource = { datasource_id: string; name: string } & (
  | OracleDatasourceView
  | MysqlDatasourceView
);

/** 写入面。`password` 留空 = 不改（新建时 = 没有口令），见 ADR-0037 §5。 */
export type DatasourceInput = { name: string } & (
  | { kind: "oracle"; connect_string: string; username: string; password: string }
  | {
      kind: "mysql";
      host: string;
      port: number;
      username: string;
      password: string;
      database: string;
    }
);

export interface TaskInput {
  name: string;
  /** 绑定，不是规格（ADR-0037 §8）：规格只描述搬什么，这两个 id 说的是从哪搬到哪。 */
  source_datasource_id: string;
  target_datasource_id: string;
  spec: TaskSpec;
}

export interface Task extends TaskInput {
  task_id: string;
}

/** 运行时逐条填的参数：参数名 → 值。顺序无关，服务端按参数名规范化（ADR-0036 §7）。 */
export type RunParams = Record<string, string>;

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

/**
 * 目标表的一列（`information_schema.COLUMNS`）。
 *
 * `length` 是**字节**（MySQL 的 `CHARACTER_MAXIMUM_LENGTH` 在 utf8mb4 下按字符给、
 * 但映射预检按字节判——两套单位的账见 ADR-0033，界面只标注不修）。
 */
export interface TargetColumn {
  name: string;
  column_type: string;
  data_type: string;
  precision: number | null;
  scale: number | null;
  length: number | null;
  datetime_precision: number | null;
  nullable: boolean;
  character_set: string | null;
  ordinal: number;
  /** 无默认值时是 `null`。它与 `extra` 是 ADR-0038 §5 第 3 分支的判据。 */
  default_value: string | null;
  /** `auto_increment` / `DEFAULT_GENERATED` 之类，没有就是空串。 */
  extra: string;
}

/** 目标表上一条唯一性约束（`PRIMARY KEY` 或 `UNIQUE`）覆盖的列。 */
export interface TargetKey {
  name: string;
  columns: string[];
}

export interface TargetTableMetadata {
  columns: TargetColumn[];
  keys: TargetKey[];
}

/** `POST /api/builder/sql` 回的一条运行参数声明。 */
export interface RunParameter {
  parameter: string;
  column: string;
  value_type: ValueType;
}

/**
 * 规格的派生面（ADR-0036 §6）：源端 SQL 与运行参数清单。
 *
 * **只出不进**——web 拿规格换一份现算的 SQL 来展示，没有把 SQL 传回来的路。
 */
export interface BuilderSql {
  source_sql: string;
  run_parameters: RunParameter[];
}

export interface MappingIssue {
  column: string | null;
  source: string | null;
  target: string | null;
  rule: string | null;
  message: string | null;
  /**
   * 动作型建议，**由 sink 侧算**（ADR-0010 2026-08-16 增补二 §1）。判定式不得复制进
   * TypeScript 重算一遍——建议与判定同源，两者都只有 sink 一份。
   */
  suggestion?: string | null;
}

export interface TargetDdlIssue {
  column: string;
  source: string;
  message: string;
}

/**
 * 取列面的三档支持标记（ADR-0010 2026-08-16 增补二 §2）。由 source 侧 describe 产出，
 * web 只负责显示，**不得自判**——自判会造出第三份白名单实现。
 */
export type ColumnSupport = 'ok' | 'needs_precision' | 'unsupported';

/**
 * `POST /api/columns` 回的列，字段名是 `type`——注意它和 `POST /api/builder/columns` 的
 * `data_type` 不同名（服务端两个端点本来就不一致）。这里照实声明；照着 `BuilderColumn`
 * 复用会读到 undefined，「describe 类型」那一列就会一直是空的。
 */
export interface FetchedColumn {
  name: string;
  type: string;
  precision: number | null;
  scale: number | null;
  length: number | null;
  /** `TIMESTAMP(n)` 的 `n`（ADR-0010 2026-08-16 增补一）。非 `TIMESTAMP` 列不带它。 */
  fsp?: number | null;
  /** 见 {@link ColumnSupport}。 */
  support?: ColumnSupport | null;
}

export interface ColumnFetchResult {
  columns: FetchedColumn[];
  target_ddl: string;
}

export interface RunHistory {
  run_record_id: string;
  run_id: string | null;
  task_id: string;
  /** 本次运行参数集，参数名 → 值（ADR-0036 §7）。无参数的任务是空对象。 */
  run_params: RunParams;
  /**
   * 当次**实际执行**的源端 SQL 快照（ADR-0036 §2）。存的是未绑定的语句文本，
   * 参数值不内联——值由 `run_params` 展示。
   */
  source_sql: string;
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
  /**
   * 失败分类闭集（source 侧 `FailureKind`，见 ADR-0029）。成功与进行中都是 `null`——
   * 读到 `null` 不是错误。
   */
  failure_kind: string | null;
  unknown_reason: "PROCESS_DISAPPEARED" | "SERVICE_RESTARTED" | null;
  seq: number;
  rows_pushed: number;
  bytes: number;
  ms: number;
  last_ts: string | null;
  mapping_issues: MappingIssue[];
}

export interface LiveRunDetail {
  run_record_id: string;
  run_id: string | null;
  run_params: RunParams;
  source_sql: string;
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

/**
 * 运行历史的筛选面只剩任务一维。
 *
 * 业务日期那一维随 ADR-0035 §3 一起没了：运行参数是任务自己定义的名字，
 * 不同任务的参数名各不相同，筛不出一个跨任务的通用维度。
 */
export interface RunHistoryFilters {
  taskId?: string;
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

export function emptySpec(): TaskSpec {
  return {
    owner: "",
    table: "",
    target_table: "",
    columns: [],
    primary_key: [],
    conditions: [],
    order_by: [],
  };
}

export function taskInputFrom(
  task: TaskInput,
  overrides: Partial<TaskInput> = {},
): TaskInput {
  return {
    name: task.name,
    source_datasource_id: task.source_datasource_id,
    target_datasource_id: task.target_datasource_id,
    spec: task.spec,
    ...overrides,
  };
}

export async function listDatasources(): Promise<Datasource[]> {
  const response = await fetch("/api/datasources", {
    headers: { Accept: "application/json" },
  });
  return readJson<Datasource[]>(response, "加载数据源失败");
}

export async function createDatasource(input: DatasourceInput): Promise<Datasource> {
  return postJson<Datasource>("/api/datasources", input, "新建数据源失败");
}

export async function updateDatasource(
  datasourceId: string,
  input: DatasourceInput,
): Promise<Datasource> {
  const response = await fetch(
    `/api/datasources/${encodeURIComponent(datasourceId)}`,
    {
      method: "PUT",
      headers: { Accept: "application/json", "Content-Type": "application/json" },
      body: JSON.stringify(input),
    },
  );
  return readJson<Datasource>(response, "更新数据源失败");
}

export async function deleteDatasource(datasourceId: string): Promise<Datasource> {
  const response = await fetch(
    `/api/datasources/${encodeURIComponent(datasourceId)}`,
    { method: "DELETE", headers: { Accept: "application/json" } },
  );
  return readJson<Datasource>(response, "删除数据源失败");
}

export async function testDatasource(datasourceId: string): Promise<{ ok: true }> {
  return postJson<{ ok: true }>(
    `/api/datasources/${encodeURIComponent(datasourceId)}/test-connection`,
    {},
    "测试连接失败",
  );
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
  const suffix = query.size === 0 ? "" : `?${query.toString()}`;
  const response = await fetch(`/api/runs${suffix}`, {
    headers: { Accept: "application/json" },
  });
  return readJson<RunHistory[]>(response, "加载运行历史失败");
}

export async function startRun(
  taskId: string,
  runParams: RunParams,
): Promise<{ run_record_id: string }> {
  return postJson<{ run_record_id: string }>(
    "/api/runs",
    { task_id: taskId, run_params: runParams },
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

export async function fetchBuilderTables(
  datasourceId: string,
  dblink: string,
): Promise<BuilderTable[]> {
  return postJson<BuilderTable[]>(
    "/api/builder/tables",
    { datasource_id: datasourceId, dblink },
    "读取 Oracle 表失败",
  );
}

export async function fetchBuilderColumns(input: {
  datasource_id: string;
  dblink: string;
  owner: string;
  table: string;
}): Promise<BuilderColumn[]> {
  return postJson<BuilderColumn[]>("/api/builder/columns", input, "读取 Oracle 列失败");
}

/**
 * 目标端元数据面（ADR-0038 §3）。**结果纯瞬态**：只活在页面状态里，
 * 不进任务定义、不进 SQLite，刷新即丢（§8）。映射关系本身要存，目标表结构快照不存。
 *
 * 界面只报数据源 id——凭据由 source 解一次再过线，前端一次也不碰。
 */
export async function fetchTargetTables(datasourceId: string): Promise<string[]> {
  const body = await postJson<{ tables: string[] }>(
    "/api/target/tables",
    { datasource_id: datasourceId },
    "读取目标表清单失败",
  );
  return body.tables;
}

/** 一张目标表的列清单与唯一性约束。**表不存在回空清单，不是错误**（ADR-0038 §9）。 */
export async function fetchTargetColumns(
  datasourceId: string,
  targetTable: string,
): Promise<TargetTableMetadata> {
  return postJson<TargetTableMetadata>(
    "/api/target/columns",
    { datasource_id: datasourceId, target_table: targetTable },
    "读取目标列失败",
  );
}

export async function generateBuilderSql(spec: TaskSpec): Promise<BuilderSql> {
  return postJson<BuilderSql>("/api/builder/sql", spec, "生成 SQL 失败");
}

export async function fetchColumns(
  datasourceId: string,
  spec: TaskSpec,
  columnPrecision?: ColumnPrecision,
): Promise<ColumnFetchResult> {
  return postJson<ColumnFetchResult>(
    "/api/columns",
    columnPrecision === undefined
      ? { datasource_id: datasourceId, spec }
      : { datasource_id: datasourceId, spec, column_precision: columnPrecision },
    "取列失败",
  );
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
