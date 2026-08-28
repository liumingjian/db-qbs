export type ColumnPrecision = Record<string, [number, number]>;

/**
 * 任务定义的**结构化规格**，唯一真相源。
 *
 * SQL 不在里面：它由规格现算，界面上只读，没有编辑入口。
 */
export interface ColumnMapping {
  source: string;
  /** 目标列名，默认预填 = `source`。落到 SQL 上就是投影的别名。 */
  target: string;
}

export interface TaskSpec {
  source_sql?: string;
  dblink?: string;
  owner: string;
  table: string;
  target_table: string;
  /**
   * upsert 的去重键，必选。存的是**目标列名**，与 `columns[].target` 同一个名字空间。
   */
  primary_key: string[];
  columns: ColumnMapping[];
  /**
   * 过滤条件：**原样拼进 `WHERE` 后面的一段自由文本**，不含 `WHERE` 这个词本身。
   *
   * 空串就是不加 `WHERE`，即整表取数。前端始终送字符串（哪怕是空串）。
   *
   * **读的时候它可能缺席**：服务端那边是 `Option<String>`，`None` 时整个字段不序列化
   * （直接调 API 建的任务就会这样）。所以每个读它的地方都得 `?? ""`——
   * 少一处就是一个 `undefined.trim()`。
   */
  where_clause?: string;
}

export type TargetCheckKind =
  | "missing_column"
  | "nullability_mismatch"
  | "insufficient_length_or_precision"
  | "primary_key_mismatch"
  | "type_not_whitelisted";

export interface CheckFinding {
  column: string | null;
  kind: TargetCheckKind;
  expected: string;
  actual: string;
  message: string;
}

export interface TargetCheckResult {
  ok: boolean;
  findings: CheckFinding[];
  suggested_ddl: string | null;
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
  /**
   * agent 上报的、它所连 MySQL 的版本（#257）。
   *
   * **`null` 是「还没报过」，不是「8.0」**：agent 自己不持有目标库凭据，要等它经手过
   * 一次目标端检查或一次开跑才知道；#257 之前的 agent 则永远报不出来。
   */
  mysql_version: string | null;
  /** 同上，utf8mb4 的默认字符序——生成建表语句时那一段 `COLLATE` 取的就是它。 */
  mysql_collation: string | null;
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
  /**
   * `information_schema.COLUMNS.EXTRA`，例如 `auto_increment`；没有就是空串。
   *
   * 取值随目标端 MySQL 版本而变：`DEFAULT_GENERATED` 只有 8.0 会给，5.7 同一根列是空串。
   * 判自增只能按「小写之后包含 `auto_increment`」，不能与某个版本独有的取值做等值比较（#262）。
   */
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

/**
 * 规格的派生面：现算的源端 SQL。
 *
 * **只出不进**——web 拿规格换一份现算的 SQL 来展示，没有把 SQL 传回来的路。
 */
export interface BuilderSql {
  source_sql: string;
}

export interface PreviewResult {
  columns: string[];
  rows: (string | null)[][];
  truncated: boolean;
  elapsed_ms: number;
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

export interface RunEvidence {
  source?: {
    datasource_id: string;
    connect_string: string;
    username: string;
    client_lib_dir: string;
  } | null;
  target?: {
    datasource_id: string;
    host: string;
    port: number;
    database: string;
    username: string;
  } | null;
  agent?: {
    agent_id: string;
    name: string;
    base_url: string;
    instance_id: string;
  } | null;
  parameters?: {
    target_table: string;
    columns: ColumnMapping[];
    primary_key: string[];
    source_sql: string;
  } | null;
}

export interface RunHistory {
  run_record_id: string;
  run_id: string | null;
  task_id: string;
  /**
   * 当次**实际执行**的源端 SQL 快照：它回答「当时执行了什么」，规格之后怎么改都不动它。
   * 过滤条件就在这条语句里，没有另一半取值需要对照着读。
   */
  source_sql: string;
  evidence?: RunEvidence;
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
  source_sql: string;
  evidence?: RunEvidence;
  staging_table: string | null;
  /** 发起时刻。「已用时」按它算墙钟——`ms` 是批次耗时的累加，不是运行的时长。 */
  started_at: string;
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
 * 业务日期那一维早就没了：它从来不是一个跨任务的通用维度。
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

// ---------------------------------------------------------------- 登录与会话
//
// **这道门只装在 source 的 HTTP 面上。** 目标端 sink 仍然没有任何鉴权，
// 能连上 sink 端口的人照旧可以绕过这里清空重写目标表——界面上看不见的这件事，
// 不因为多了一个登录页而消失。

export interface SessionState {
  authenticated: boolean;
  username: string | null;
}

type SessionLostListener = () => void;

let sessionLostListener: SessionLostListener | null = null;

/**
 * 注册「票据没了」的唯一去处。整套界面只有 `App` 订阅它，收到就回登录页。
 *
 * 做成单个监听者而不是一串：这个事件的正确后果只有一个（整屏回到登录页），
 * 允许多方各自反应，只会得到几个互相打架的跳转。
 */
export function onSessionLost(listener: SessionLostListener | null) {
  sessionLostListener = listener;
}

function notifySessionLost() {
  sessionLostListener?.();
}

/**
 * 首屏问一句「我登着吗」。**这个接口没登录也答得出来**（回 `authenticated: false`），
 * 所以首屏不必先撞一个 401 再从错误里反推。
 */
export async function fetchSession(): Promise<SessionState> {
  const response = await fetch("/api/session", {
    headers: { Accept: "application/json" },
  });
  return readJson<SessionState>(response, "读取登录状态失败");
}

/**
 * 登录。票据是 `HttpOnly` 的 cookie，**前端一个字都碰不到**——
 * 这里没有 token 可存，也没有 `localStorage` 可写。
 */
export async function login(
  username: string,
  password: string,
): Promise<SessionState> {
  const response = await fetch("/api/session", {
    method: "POST",
    headers: { Accept: "application/json", "Content-Type": "application/json" },
    body: JSON.stringify({ username, password }),
  });
  // 这里的 401 是「口令不对」，不是「会话没了」，所以不广播。
  return readJson<SessionState>(response, "登录失败", { ownsUnauthorized: true });
}

/** 退出登录。**只销这一张票**：同一个账号在别处登着的不受影响。 */
export async function logout(): Promise<void> {
  const response = await fetch("/api/session", {
    method: "DELETE",
    headers: { Accept: "application/json" },
  });
  await readJson<unknown>(response, "退出登录失败");
}

/**
 * 改口令。**改完除了当前这一张之外的会话全部失效**，所以别处登着的浏览器
 * 下一次请求就会被弹回登录页——这正是改口令这个动作该有的后果。
 */
export async function changePassword(
  currentPassword: string,
  newPassword: string,
): Promise<void> {
  const response = await fetch("/api/password", {
    method: "PUT",
    headers: { Accept: "application/json", "Content-Type": "application/json" },
    body: JSON.stringify({
      current_password: currentPassword,
      new_password: newPassword,
    }),
  });
  await readJson<unknown>(response, "修改口令失败");
}

export function emptySpec(): TaskSpec {
  return {
    owner: "",
    table: "",
    target_table: "",
    columns: [],
    primary_key: [],
    where_clause: "",
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
  const detail = errorDetail(error.body);
  if (detail === undefined) {
    return [];
  }
  const names = detail[key];
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

/** 服务端给出完整命令；前端不重建请求形状，只把返回值原样写进剪贴板。 */
export async function copyTaskCurl(
  taskId: string,
  writeText: (command: string) => Promise<void> = (command) =>
    navigator.clipboard.writeText(command),
): Promise<void> {
  const response = await fetch(`/api/tasks/${encodeURIComponent(taskId)}/curl`, {
    headers: { Accept: "application/json" },
  });
  const result = await readJson<{ command: string }>(response, "读取 cURL 命令失败");
  await writeText(result.command);
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

/**
 * 发起一次运行。**任务身份就是全部输入**——点了就跑，没有对话框、没有参数。
 *
 * 同一个任务已有一次在飞时服务端回 409，消息由调用方原样展示。
 */
export async function startRun(
  taskId: string,
): Promise<{ run_record_id: string }> {
  return postJson<{ run_record_id: string }>(
    "/api/runs",
    { task_id: taskId },
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

export async function checkTargetTable(
  sourceDatasourceId: string,
  targetDatasourceId: string,
  targetTable: string,
  spec: TaskSpec,
): Promise<TargetCheckResult> {
  return postJson<TargetCheckResult>(
    "/api/target/check",
    {
      source_datasource_id: sourceDatasourceId,
      target_datasource_id: targetDatasourceId,
      target_table: targetTable,
      spec,
    },
    "检查目标表失败",
  );
}

export async function generateBuilderSql(spec: TaskSpec): Promise<BuilderSql> {
  return postJson<BuilderSql>("/api/builder/sql", spec, "生成 SQL 失败");
}

export async function previewBuilderRows(
  sourceDatasourceId: string,
  spec: TaskSpec,
  limit = 10,
): Promise<PreviewResult> {
  return postJson<PreviewResult>(
    "/api/builder/preview",
    { source_datasource_id: sourceDatasourceId, spec, limit },
    "预览源端数据失败",
  );
}

export function previewErrorMessage(error: unknown): string {
  if (!(error instanceof ApiError)) return "数据预览失败，请稍后重试";
  if (error.status === 400) return `预览请求无效：${error.message}`;
  if (error.status === 504) return `数据预览超时：${error.message}`;
  if (error.status === 502) {
    const detail = errorDetail(error.body);
    const kind =
      typeof detail?.failure_kind === "string" ? `（${detail.failure_kind}）` : "";
    return `源数据库预览失败${kind}：${error.message}`;
  }
  return error.message;
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

async function readJson<T>(
  response: Response,
  fallback: string,
  options: { ownsUnauthorized?: boolean } = {},
): Promise<T> {
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    body = undefined;
  }
  if (!response.ok) {
    // 401 只有一个含义：**票据没了**（过期、被改密连坐、或者压根没登过）。
    // 它在这一层统一广播，而不是让三十几个调用点各自认一遍——漏掉的那一个
    // 会变成一屏「加载失败」，而真正该发生的是回到登录页。
    //
    // `ownsUnauthorized` 是登录接口自己的豁免：那里的 401 是「口令不对」，
    // 不是「会话没了」，广播出去只会把用户正在填的表单清掉。
    if (response.status === 401 && options.ownsUnauthorized !== true) {
      notifySessionLost();
    }
    const message = errorMessage(body) ?? `${fallback}（HTTP ${response.status}）`;
    throw new ApiError(message, response.status, body);
  }
  return body as T;
}

/**
 * source 的**唯一**错误信封：`{"error": {"message": ..., "kind"?: ...}}`（#199）。
 *
 * 壳只有一种，所以这里不必按端点认形状。`kind` 是壳里的可选字段，读它的是需要
 * 「下一步该找谁」的屏（取列、目标端元数据）；不需要的屏当它不存在。
 */
export function errorDetail(body: unknown): Record<string, unknown> | undefined {
  if (typeof body !== "object" || body === null || !("error" in body)) {
    return undefined;
  }
  const detail = (body as { error: unknown }).error;
  return typeof detail === "object" && detail !== null
    ? (detail as Record<string, unknown>)
    : undefined;
}

function errorMessage(body: unknown): string | undefined {
  const message = errorDetail(body)?.message;
  return typeof message === "string" ? message : undefined;
}
