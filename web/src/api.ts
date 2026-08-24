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
  source_sql?: string;
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
  /**
   * 这条目标库经**哪台 agent** 访问（ADR-0044 §3）。回的是 id 不是地址——
   * 名字与在线状态在 `/api/agents` 那份里，两处各有各的真相源。
   */
  agent_id: string;
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
      /** 必填（ADR-0044 §1）：目标库只能经 agent 访问，没绑 agent 的数据源存不进去。 */
      agent_id: string;
      host: string;
      port: number;
      username: string;
      password: string;
      database: string;
    }
);

/**
 * 目标端 agent（ADR-0044）。**一台 agent = 目标端那个 sink 进程**，
 * 目标库只能经它访问：元数据、测连、写入三条链都落在它身上。
 *
 * `status` 三档不是两档：`mismatch` 说的是「这个地址还通，但应答的是另一台 agent」，
 * 它的处置与「没起来」完全不同，合并成一个「离线」等于把线索抹掉。
 */
export type AgentStatus = "online" | "offline" | "mismatch";

export interface Agent {
  agent_id: string;
  name: string;
  base_url: string;
  /** agent 自报的稳定身份，注册那一刻钉下。迁移出来、还没探过的那条是空串。 */
  instance_id: string;
  version: string;
  last_seen_at: string | null;
  status: AgentStatus;
  last_error: string | null;
}

export interface AgentInput {
  name: string;
  base_url: string;
}

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
  /**
   * 开跑前那一次 `COUNT(*)` 拿到的**总行数**，也就是迁移进度那一列的分母（ADR-0043 §7）。
   *
   * **可以为 `null`**：计数本身失败时它缺席，而那次运行**照常跑**——为了一个进度条把整次
   * 搬运判死是拿主功能换装饰。前端读到 `null` 就把进度退回 `—` 并在 `title` 上自陈。
   * 它是**开跑那一刻**的事实，不是实时的：与随后的读取之间存在时间差。
   */
  total_rows: number | null;
  /**
   * 那一次计数自己的耗时。**单独一栏，不混进读取耗时**（`fetch_ms`）——把它揉进去，
   * 下一个人看到的「取数慢」会是两件事的和。与 sink 侧的门禁计数 `count_ms` 也是两回事。
   */
  precount_ms: number | null;
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
  total_rows: number | null;
  precount_ms: number | null;
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

export async function listAgents(): Promise<Agent[]> {
  const response = await fetch("/api/agents", {
    headers: { Accept: "application/json" },
  });
  return readJson<Agent[]>(response, "加载目标端 agent 失败");
}

/**
 * 注册一台 agent。**服务端当场探一次，探不通就不落库**（ADR-0044 §3）——
 * 所以这个调用失败的含义是「那个地址上没有一台活着的 agent」，不是「表单填错了」。
 */
export async function registerAgent(input: AgentInput): Promise<Agent> {
  return postJson<Agent>("/api/agents", input, "注册目标端 agent 失败");
}

export async function updateAgent(
  agentId: string,
  input: AgentInput,
): Promise<Agent> {
  const response = await fetch(`/api/agents/${encodeURIComponent(agentId)}`, {
    method: "PUT",
    headers: { Accept: "application/json", "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
  return readJson<Agent>(response, "更新目标端 agent 失败");
}

/** 手动探一台。**探测失败也是 200**：结果本身就是要显示的信息。 */
export async function probeAgent(agentId: string): Promise<Agent> {
  return postJson<Agent>(
    `/api/agents/${encodeURIComponent(agentId)}/probe`,
    {},
    "探测目标端 agent 失败",
  );
}

export async function deleteAgent(agentId: string): Promise<Agent> {
  const response = await fetch(`/api/agents/${encodeURIComponent(agentId)}`, {
    method: "DELETE",
    headers: { Accept: "application/json" },
  });
  return readJson<Agent>(response, "删除目标端 agent 失败");
}

/**
 * 删 agent 被拒（409）时，服务端点名的那几条数据源。与 [`referencedTasksFrom`] 同一形态。
 */
export function referencedDatasourcesFrom(error: unknown): string[] {
  return namesFromConflict(error, "datasources");
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

/** 「测试连接」的回报。`label` 是 MySQL 的库名 / Oracle 的连接串，拼在成功那一行里。 */
export interface DatasourceTestResult {
  ok: true;
  elapsed_ms: number;
  label: string;
}

/**
 * 用**表单里当前填的那组值**测连（ADR-0039 §3），不是库里存的那条。
 *
 * 「测通才让存」这条门槛决定了它必须是草稿测连：新建的数据源库里还没有，
 * 按 id 测无从谈起；改了口令的编辑态按 id 测的也是旧口令。
 *
 * `datasourceId` 只在编辑态给，用途单一——口令留空时服务端去库里取那一份，
 * 与保存的「留空 = 不改」是同一条解释规则。
 */
export async function testDatasourceDraft(
  input: DatasourceInput,
  datasourceId: string | null,
): Promise<DatasourceTestResult> {
  return postJson<DatasourceTestResult>(
    "/api/datasources/test-connection",
    { ...input, datasource_id: datasourceId },
    "测试连接失败",
  );
}

/**
 * 删数据源被拒（409）时，服务端点名的那几个任务（ADR-0039 §4）。
 *
 * 拿不到就返回空数组——**报文正文里本来就带着同一句话**，列表只是把它摆成可扫的形状。
 */
export function referencedTasksFrom(error: unknown): string[] {
  return namesFromConflict(error, "tasks");
}

/** 409 报文里 `error.<key>` 那串名字。拿不到就空数组——正文里本来就带着同一句话。 */
function namesFromConflict(error: unknown, key: string): string[] {
  if (!(error instanceof ApiError) || error.status !== 409) {
    return [];
  }
  const body = error.body;
  if (typeof body !== "object" || body === null || !("error" in body)) {
    return [];
  }
  const detail = (body as { error: unknown }).error;
  if (typeof detail !== "object" || detail === null || !(key in detail)) {
    return [];
  }
  const names = (detail as Record<string, unknown>)[key];
  return Array.isArray(names)
    ? names.filter((name): name is string => typeof name === "string")
    : [];
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

export async function fetchBuilderDblinks(datasourceId: string): Promise<string[]> {
  return postJson<string[]>(
    "/api/builder/dblinks",
    { datasource_id: datasourceId },
    "读取 Oracle DBLINK 失败",
  );
}

export async function fetchBuilderSqlColumns(input: {
  datasource_id: string;
  source_sql: string;
}): Promise<FetchedColumn[]> {
  return postJson<FetchedColumn[]>(
    "/api/builder/sql-columns",
    input,
    "读取自定义 SQL 列失败",
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

/**
 * 取列面（`POST /api/columns`）。
 *
 * **当前没有 UI 调用方**：构建器里那张取列卡随 `47a2fed` 摘掉，所有者 2026-08-21
 * 裁定判废（ADR-0043 「两条收尾裁定」第一条）。**端点与本函数都留着**——
 * 端点是协议面的东西，不因为界面上暂时没人调就删；真要接回建表 SQL 时从这里起步。
 */
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
