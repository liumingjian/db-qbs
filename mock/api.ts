/**
 * 开发期的**假后端**，只在 `VITE_MOCK=1` 时挂到 vite dev server 上。
 *
 * 它不进产物，也不被 `web/src` 里的任何一行引用：整套界面照旧对着 `/api/*` 发请求，
 * 只是这一次答话的是这个中间件而不是 Rust 的 `source`。所以生产行为一个字没动——
 * 关掉环境变量，这个文件就完全不存在于运行期。
 *
 * 覆盖的是 `crates/source/src/http.rs` 的 `routes()` 整张表（含公开的三条会话路由）。
 * 数据是编出来的，状态存在进程内存里，重启 dev server 即回到初值。
 */

type Json = unknown;

interface MockReq {
  url?: string;
  method?: string;
  headers: Record<string, string | string[] | undefined>;
  setEncoding?(encoding: string): void;
  on(event: string, listener: (chunk?: unknown) => void): void;
}

interface MockRes {
  statusCode: number;
  setHeader(name: string, value: string | string[]): void;
  end(body?: string): void;
}

interface Reply {
  status: number;
  body: Json;
  cookie?: string;
}

const ok = (body: Json): Reply => ({ status: 200, body });
const fail = (status: number, message: string, extra: Json = {}): Reply => ({
  status,
  // source 的唯一错误信封：`{"error": {"message": ..., "kind"?: ...}}`
  body: { error: { message, ...(extra as Record<string, unknown>) } },
});

// --------------------------------------------------------------- 假数据初值

const COOKIE_NAME = "db_qbs_session";
const MOCK_USER = "admin";

let password = "admin";
const sessions = new Set<string>();

interface AgentRow {
  agent_id: string;
  name: string;
  base_url: string;
  instance_id: string;
  version: string;
  last_seen_at: string | null;
  status: "online" | "offline" | "mismatch";
  last_error: string | null;
  /** agent 上报的所连 MySQL 版本（#257）。`null` 是「还没报过」，不是 8.0。 */
  mysql_version: string | null;
  mysql_collation: string | null;
}

let agents: AgentRow[] = [
  {
    agent_id: "agent-01",
    name: "机房 A · 目标端",
    base_url: "http://10.20.0.11:8443",
    instance_id: "inst-3f9a11c2",
    version: "0.1.0",
    last_seen_at: new Date().toISOString(),
    status: "online",
    last_error: null,
    mysql_version: "8.0.36",
    mysql_collation: "utf8mb4_0900_ai_ci",
  },
  {
    agent_id: "agent-02",
    name: "机房 B · 备用",
    base_url: "http://10.20.0.12:8443",
    instance_id: "inst-77b0e401",
    version: "0.1.0",
    last_seen_at: new Date(Date.now() - 3_600_000).toISOString(),
    status: "offline",
    last_error: "连接被拒绝（Connection refused）",
    // 从没经手过一次目标端检查的 agent：版本一列就该是「未知」。
    mysql_version: null,
    mysql_collation: null,
  },
];

type DatasourceRow = Record<string, unknown> & {
  datasource_id: string;
  name: string;
  kind: "oracle" | "mysql";
};

let datasources: DatasourceRow[] = [
  {
    datasource_id: "ds-ora-1",
    name: "核心库（Oracle 11g）",
    kind: "oracle",
    connect_string: "10.30.0.5:1521/ORCL",
    username: "APPUSER",
    has_password: true,
  },
  {
    datasource_id: "ds-my-1",
    name: "报表库（MySQL 8.0）",
    kind: "mysql",
    agent_id: "agent-01",
    host: "10.20.0.11",
    port: 3306,
    username: "report",
    database: "warehouse",
    has_password: true,
  },
];

interface SpecShape {
  source_sql?: string;
  dblink?: string;
  owner: string;
  table: string;
  target_table: string;
  write_mode: "APPEND" | "CLEAR_THEN_IMPORT";
  primary_key: string[];
  columns: { source: string; target: string }[];
  where_clause?: string;
}

interface TaskRow {
  task_id: string;
  name: string;
  source_datasource_id: string;
  target_datasource_id: string;
  spec: SpecShape;
}

let tasks: TaskRow[] = [
  {
    task_id: "task-001",
    name: "客户主数据日更",
    source_datasource_id: "ds-ora-1",
    target_datasource_id: "ds-my-1",
    spec: {
      owner: "APPUSER",
      table: "CUSTOMER",
      target_table: "dim_customer",
      write_mode: "APPEND",
      primary_key: ["customer_id"],
      columns: [
        { source: "CUSTOMER_ID", target: "customer_id" },
        { source: "CUSTOMER_NAME", target: "customer_name" },
        { source: "STATUS", target: "status" },
        { source: "CREATED_AT", target: "created_at" },
      ],
      where_clause: "STATUS = 'A'",
    },
  },
  {
    task_id: "task-002",
    name: "订单明细月结",
    source_datasource_id: "ds-ora-1",
    target_datasource_id: "ds-my-1",
    spec: {
      owner: "APPUSER",
      table: "ORDER_ITEM",
      target_table: "fact_order_item",
      write_mode: "APPEND",
      primary_key: ["order_id", "line_no"],
      columns: [
        { source: "ORDER_ID", target: "order_id" },
        { source: "LINE_NO", target: "line_no" },
        { source: "SKU", target: "sku" },
        { source: "QTY", target: "qty" },
        { source: "AMOUNT", target: "amount" },
      ],
      where_clause: "",
    },
  },
];

/** 内存里的运行记录。字段与 `RunHistory` 对齐，缺一个界面就少一格。 */
type RunRow = Record<string, unknown> & {
  run_record_id: string;
  run_id: string | null;
  task_id: string;
  started_at: string;
  /** 只在假运行里用：到点之后才推进阶段。不上线，界面读不到它。 */
  __startedMs?: number;
};

function evidenceFor(task: TaskRow): Json {
  const source = datasources.find((d) => d.datasource_id === task.source_datasource_id);
  const target = datasources.find((d) => d.datasource_id === task.target_datasource_id);
  const agent = agents.find((a) => a.agent_id === (target?.agent_id as string));
  return {
    source: source
      ? {
          datasource_id: source.datasource_id,
          connect_string: String(source.connect_string ?? ""),
          username: String(source.username ?? ""),
          client_lib_dir: "/opt/oracle/instantclient_19_8",
        }
      : null,
    target: target
      ? {
          datasource_id: target.datasource_id,
          host: String(target.host ?? ""),
          port: Number(target.port ?? 3306),
          database: String(target.database ?? ""),
          username: String(target.username ?? ""),
        }
      : null,
    agent: agent
      ? {
          agent_id: agent.agent_id,
          name: agent.name,
          base_url: agent.base_url,
          instance_id: agent.instance_id,
        }
      : null,
    parameters: {
      target_table: task.spec.target_table,
      columns: task.spec.columns,
      primary_key: task.spec.primary_key,
      source_sql: generateSql(task.spec),
    },
  };
}

function finishedRow(
  task: TaskRow,
  overrides: Record<string, unknown>,
): RunRow {
  const startedAt = String(overrides.started_at ?? new Date().toISOString());
  return {
    run_record_id: String(overrides.run_record_id ?? newRunRecordId()),
    run_id: (overrides.run_id as string | null) ?? newRunId(),
    task_id: task.task_id,
    source_sql: generateSql(task.spec),
    evidence: evidenceFor(task),
    staging_table: `${task.spec.target_table}__stg_${overrides.run_id ?? "20260826010101_a1b2c3"}`,
    started_at: startedAt,
    finished_at: new Date(Date.parse(startedAt) + 42_000).toISOString(),
    outcome: "SUCCEEDED",
    target_table_effect: "SWAPPED",
    stage: "SUCCEEDED",
    source_rows: 128_400,
    staged_rows: 128_400,
    sink_reported_rows: 128_400,
    purged_rows: null,
    source_batches: 26,
    received_batches: 26,
    total_rows: 128_400,
    precount_ms: 310,
    fetch_ms: 18_200,
    push_ms: 19_800,
    commit_ms: 2_100,
    count_ms: 140,
    cursor_ms: 90,
    source_code: null,
    sink_code: null,
    column: null,
    value: null,
    message: null,
    failure_kind: null,
    unknown_reason: null,
    seq: 26,
    rows_pushed: 128_400,
    bytes: 41_300_000,
    ms: 38_000,
    last_ts: new Date(Date.parse(startedAt) + 42_000).toISOString(),
    mapping_issues: [],
    ...overrides,
  } as RunRow;
}

let runs: RunRow[] = [];

function seedRuns() {
  const t1 = tasks[0];
  const t2 = tasks[1];
  const hourAgo = (h: number) => new Date(Date.now() - h * 3_600_000).toISOString();
  runs = [
    finishedRow(t1, {
      run_record_id: "rr-1001",
      run_id: "20260825081500_a3f19c",
      started_at: hourAgo(26),
    }),
    finishedRow(t1, {
      run_record_id: "rr-1002",
      run_id: "20260826021500_b71e04",
      started_at: hourAgo(3),
    }),
    finishedRow(t2, {
      run_record_id: "rr-1003",
      run_id: null,
      started_at: hourAgo(9),
      staging_table: null,
      outcome: "FAILED",
      target_table_effect: "DISCARDED",
      stage: "FAILED",
      source_rows: null,
      staged_rows: null,
      sink_reported_rows: null,
      source_batches: null,
      received_batches: null,
      total_rows: null,
      precount_ms: null,
      fetch_ms: null,
      push_ms: null,
      commit_ms: null,
      count_ms: null,
      cursor_ms: null,
      sink_code: "MAPPING_PRECHECK_FAILED",
      column: "AMOUNT",
      value: null,
      message: "映射预检未通过：目标列 amount 的精度不足以容纳源端 NUMBER(18,4)",
      failure_kind: "MAPPING_PRECHECK",
      seq: 0,
      rows_pushed: 0,
      bytes: 0,
      ms: 0,
      mapping_issues: [
        {
          column: "AMOUNT",
          source: "NUMBER(18,4)",
          target: "DECIMAL(10,2)",
          rule: "insufficient_length_or_precision",
          message: "目标列精度不足，写入会被静默截断",
          suggestion: "把 amount 改成 DECIMAL(18,4)",
        },
      ],
    }),
  ];
}
// seedRuns() 在文件末尾调用：它用到的 `newRunId` 等是 const 箭头函数，不提升。

// ------------------------------------------------------------------ 小工具

let idSeq = 0;
const nextId = (prefix: string) => `${prefix}-${(++idSeq).toString().padStart(3, "0")}`;

function stamp(): string {
  const d = new Date();
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  return (
    `${d.getUTCFullYear()}${p(d.getUTCMonth() + 1)}${p(d.getUTCDate())}` +
    `${p(d.getUTCHours())}${p(d.getUTCMinutes())}${p(d.getUTCSeconds())}`
  );
}
const hex6 = () => Math.floor(Math.random() * 0xffffff).toString(16).padStart(6, "0");
const newRunId = () => `${stamp()}_${hex6()}`;
const newRunRecordId = () => `rr-${stamp()}-${hex6().slice(0, 4)}`;

/** 规格 → 现算的源端 SQL。真实实现在 Rust 那边，这里只求形状像。 */
function generateSql(spec: SpecShape): string {
  const projection =
    spec.columns.length === 0
      ? "*"
      : spec.columns
          .map((c) => `${c.source} AS ${c.target}`)
          .join(",\n       ");
  if ((spec.source_sql ?? "").trim() !== "") {
    return `SELECT ${spec.columns
      .map((c) => `q.${c.source} AS ${c.target}`)
      .join(", ")}\n  FROM ( ${spec.source_sql} ) q`;
  }
  const dblink = (spec.dblink ?? "").trim();
  const from = `${spec.owner}.${spec.table}${dblink === "" ? "" : `@${dblink}`}`;
  const where = (spec.where_clause ?? "").trim();
  return `SELECT ${projection}\n  FROM ${from}${where === "" ? "" : `\n WHERE ${where}`}`;
}

const ORACLE_TABLES = [
  "CUSTOMER",
  "ORDER_HEADER",
  "ORDER_ITEM",
  "PRODUCT",
  "PAYMENT",
  "SHIPMENT",
  "INVENTORY_SNAPSHOT",
];

const ORACLE_COLUMNS: Record<string, { name: string; data_type: string; precision: number | null; scale: number | null; length: number | null; nullable: boolean }[]> = {
  DEFAULT: [
    { name: "ID", data_type: "NUMBER", precision: 12, scale: 0, length: null, nullable: false },
    { name: "NAME", data_type: "VARCHAR2", precision: null, scale: null, length: 120, nullable: true },
    { name: "AMOUNT", data_type: "NUMBER", precision: 18, scale: 4, length: null, nullable: true },
    { name: "STATUS", data_type: "CHAR", precision: null, scale: null, length: 1, nullable: true },
    { name: "CREATED_AT", data_type: "DATE", precision: null, scale: null, length: null, nullable: true },
    { name: "UPDATED_AT", data_type: "TIMESTAMP(6)", precision: null, scale: 6, length: null, nullable: true },
  ],
  CUSTOMER: [
    { name: "CUSTOMER_ID", data_type: "NUMBER", precision: 12, scale: 0, length: null, nullable: false },
    { name: "CUSTOMER_NAME", data_type: "VARCHAR2", precision: null, scale: null, length: 200, nullable: true },
    { name: "STATUS", data_type: "CHAR", precision: null, scale: null, length: 1, nullable: true },
    { name: "CREATED_AT", data_type: "DATE", precision: null, scale: null, length: null, nullable: true },
  ],
  ORDER_ITEM: [
    { name: "ORDER_ID", data_type: "NUMBER", precision: 12, scale: 0, length: null, nullable: false },
    { name: "LINE_NO", data_type: "NUMBER", precision: 6, scale: 0, length: null, nullable: false },
    { name: "SKU", data_type: "VARCHAR2", precision: null, scale: null, length: 64, nullable: true },
    { name: "QTY", data_type: "NUMBER", precision: 12, scale: 3, length: null, nullable: true },
    { name: "AMOUNT", data_type: "NUMBER", precision: 18, scale: 4, length: null, nullable: true },
  ],
};

const columnsOf = (table: string) => ORACLE_COLUMNS[table] ?? ORACLE_COLUMNS.DEFAULT;

const TARGET_TABLES = [
  "dim_customer",
  "fact_order_item",
  "dim_product",
  "fact_payment",
];

function mysqlTypeFor(source: { data_type: string; precision: number | null; scale: number | null; length: number | null }): string {
  const t = source.data_type.toUpperCase();
  if (t.startsWith("NUMBER")) {
    if ((source.scale ?? 0) === 0) return "BIGINT";
    return `DECIMAL(${source.precision ?? 38},${source.scale ?? 0})`;
  }
  if (t.startsWith("TIMESTAMP")) return "DATETIME(6)";
  if (t === "DATE") return "DATETIME";
  if (t === "CHAR") return `CHAR(${source.length ?? 1})`;
  return `VARCHAR(${source.length ?? 255})`;
}

function targetMetadataFor(table: string): Json {
  const spec = tasks.find((t) => t.spec.target_table === table);
  const cols = spec ? spec.spec.columns : [];
  if (cols.length === 0 && !TARGET_TABLES.includes(table)) {
    // 表不存在回空清单，不是错误（ADR-0038 §9）。
    return { columns: [], keys: [] };
  }
  const source = spec ? columnsOf(spec.spec.table) : columnsOf("DEFAULT");
  const columns = (cols.length === 0 ? source.map((c) => ({ source: c.name, target: c.name.toLowerCase() })) : cols).map(
    (mapping, index) => {
      const src = source.find((c) => c.name === mapping.source) ?? source[0];
      const columnType = mysqlTypeFor(src);
      const isKey = (spec?.spec.primary_key ?? []).includes(mapping.target);
      return {
        name: mapping.target,
        column_type: columnType.toLowerCase(),
        data_type: columnType.replace(/\(.*/, "").toLowerCase(),
        precision: src.precision,
        scale: src.scale,
        length: src.length,
        datetime_precision: columnType.startsWith("DATETIME(") ? 6 : null,
        nullable: !isKey,
        character_set: columnType.startsWith("VARCHAR") || columnType.startsWith("CHAR") ? "utf8mb4" : null,
        ordinal: index + 1,
        default_value: null,
        extra: "",
      };
    },
  );
  const keys =
    spec && spec.spec.primary_key.length > 0
      ? [{ name: "PRIMARY", columns: spec.spec.primary_key }]
      : [];
  return { columns, keys };
}

function previewRows(spec: SpecShape) {
  const columns = spec.columns.map((c) => c.target);
  const rows: (string | null)[][] = [];
  for (let i = 1; i <= 10; i += 1) {
    rows.push(
      spec.columns.map((c) => {
        const name = c.source.toUpperCase();
        if (name.includes("ID") || name.includes("NO")) return String(1000 + i);
        if (name.includes("AMOUNT") || name.includes("QTY")) return `${(i * 13.5).toFixed(2)}`;
        if (name.includes("AT") || name.includes("DATE")) return `2026-08-${String(i).padStart(2, "0")} 09:30:00`;
        if (name === "STATUS") return i % 4 === 0 ? null : "A";
        return `${c.target}-${i}`;
      }),
    );
  }
  return { columns, rows, truncated: false, elapsed_ms: 128 };
}

// ------------------------------------------------------------------ 运行推进

const PREPARING_MS = 3_000;
const STREAMING_MS = 15_000;
const COMMITTING_MS = 3_000;

/** 把一条在飞的假运行推到「现在」该在的样子。 */
function advance(row: RunRow): RunRow {
  const started = row.__startedMs;
  if (started === undefined) return row;
  const elapsed = Date.now() - started;
  const total = Number(row.total_rows ?? 120_000);
  if (elapsed < PREPARING_MS) {
    return Object.assign(row, { stage: "PREPARING", seq: 0, rows_pushed: 0, bytes: 0, ms: 0 });
  }
  if (elapsed < PREPARING_MS + STREAMING_MS) {
    const ratio = (elapsed - PREPARING_MS) / STREAMING_MS;
    const pushed = Math.floor(total * ratio);
    return Object.assign(row, {
      stage: "STREAMING",
      seq: Math.max(1, Math.ceil(pushed / 5000)),
      rows_pushed: pushed,
      bytes: pushed * 320,
      ms: elapsed - PREPARING_MS,
      last_ts: new Date().toISOString(),
    });
  }
  if (elapsed < PREPARING_MS + STREAMING_MS + COMMITTING_MS) {
    return Object.assign(row, {
      stage: "COMMITTING",
      seq: Math.ceil(total / 5000),
      rows_pushed: total,
      bytes: total * 320,
      ms: STREAMING_MS,
      last_ts: new Date().toISOString(),
    });
  }
  delete row.__startedMs;
  return Object.assign(row, {
    stage: "SUCCEEDED",
    outcome: "SUCCEEDED",
    target_table_effect: "SWAPPED",
    finished_at: new Date(started + PREPARING_MS + STREAMING_MS + COMMITTING_MS).toISOString(),
    seq: Math.ceil(total / 5000),
    rows_pushed: total,
    source_rows: total,
    staged_rows: total,
    sink_reported_rows: total,
    source_batches: Math.ceil(total / 5000),
    received_batches: Math.ceil(total / 5000),
    bytes: total * 320,
    ms: STREAMING_MS,
    fetch_ms: STREAMING_MS,
    push_ms: STREAMING_MS,
    commit_ms: COMMITTING_MS,
    count_ms: 140,
    cursor_ms: 90,
  });
}

const inFlight = (row: RunRow) => row.__startedMs !== undefined;

/** 对外的行：内部字段不出门。 */
function publicRow(row: RunRow): Json {
  const { __startedMs, ...rest } = row;
  void __startedMs;
  return rest;
}

// -------------------------------------------------------------------- 路由

type Handler = (ctx: {
  body: Record<string, unknown>;
  id: string;
  query: URLSearchParams;
  token: string | null;
}) => Reply;

interface Route {
  method: string;
  pattern: string;
  public?: boolean;
  handler: Handler;
}

function findTask(id: string): TaskRow | undefined {
  return tasks.find((t) => t.task_id === id);
}

const ROUTES: Route[] = [
  // ---- 会话（全表仅有的三条公开路由）
  {
    method: "GET",
    pattern: "/api/session",
    public: true,
    handler: ({ token }) => {
      const live = token !== null && sessions.has(token);
      return ok({ authenticated: live, username: live ? MOCK_USER : null });
    },
  },
  {
    method: "POST",
    pattern: "/api/session",
    public: true,
    handler: ({ body }) => {
      if (body.username !== MOCK_USER || body.password !== password) {
        return fail(401, "账号或口令不正确");
      }
      const token = `tok-${hex6()}${hex6()}`;
      sessions.add(token);
      return {
        status: 200,
        body: { authenticated: true, username: MOCK_USER },
        cookie: `${COOKIE_NAME}=${token}; Path=/; HttpOnly; SameSite=Strict`,
      };
    },
  },
  {
    method: "DELETE",
    pattern: "/api/session",
    public: true,
    handler: ({ token }) => {
      if (token !== null) sessions.delete(token);
      return {
        status: 200,
        body: {},
        cookie: `${COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0`,
      };
    },
  },
  {
    method: "PUT",
    pattern: "/api/password",
    handler: ({ body, token }) => {
      if (body.current_password !== password) return fail(400, "当前口令不正确");
      const next = String(body.new_password ?? "");
      if (next === "") return fail(400, "新口令不能为空");
      password = next;
      // 改口令销掉除自己以外的全部票据。
      for (const other of [...sessions]) if (other !== token) sessions.delete(other);
      return ok({});
    },
  },

  // ---- agent
  { method: "GET", pattern: "/api/agents", handler: () => ok(agents) },
  {
    method: "POST",
    pattern: "/api/agents",
    handler: ({ body }) => {
      const row: AgentRow = {
        agent_id: nextId("agent"),
        name: String(body.name ?? "未命名"),
        base_url: String(body.base_url ?? ""),
        instance_id: `inst-${hex6()}${hex6()}`,
        version: "0.1.0",
        last_seen_at: new Date().toISOString(),
        status: "online",
        last_error: null,
        mysql_version: "8.0.36",
        mysql_collation: "utf8mb4_0900_ai_ci",
      };
      agents.push(row);
      return ok(row);
    },
  },
  {
    method: "POST",
    pattern: "/api/agents/{}/probe",
    handler: ({ id }) => {
      const row = agents.find((a) => a.agent_id === id);
      if (row === undefined) return fail(404, "目标端 agent 不存在");
      row.last_seen_at = new Date().toISOString();
      if (row.status === "online") row.last_error = null;
      return ok(row);
    },
  },
  {
    method: "PUT",
    pattern: "/api/agents/{}",
    handler: ({ id, body }) => {
      const row = agents.find((a) => a.agent_id === id);
      if (row === undefined) return fail(404, "目标端 agent 不存在");
      row.name = String(body.name ?? row.name);
      row.base_url = String(body.base_url ?? row.base_url);
      return ok(row);
    },
  },
  {
    method: "DELETE",
    pattern: "/api/agents/{}",
    handler: ({ id }) => {
      const row = agents.find((a) => a.agent_id === id);
      if (row === undefined) return fail(404, "目标端 agent 不存在");
      const bound = datasources.filter((d) => d.agent_id === id).map((d) => d.name);
      if (bound.length > 0) {
        return fail(409, `还有数据源绑在这台 agent 上：${bound.join("、")}`, {
          datasources: bound,
        });
      }
      agents = agents.filter((a) => a.agent_id !== id);
      return ok(row);
    },
  },

  // ---- 数据源
  { method: "GET", pattern: "/api/datasources", handler: () => ok(datasources) },
  {
    method: "POST",
    pattern: "/api/datasources",
    handler: ({ body }) => {
      const row = datasourceFrom(nextId("ds"), body);
      datasources.push(row);
      return ok(row);
    },
  },
  {
    method: "POST",
    pattern: "/api/datasources/test-connection",
    handler: ({ body }) =>
      ok({
        ok: true,
        elapsed_ms: 84,
        label:
          body.kind === "mysql"
            ? String(body.database ?? "")
            : String(body.connect_string ?? ""),
      }),
  },
  {
    method: "POST",
    pattern: "/api/datasources/{}/test-connection",
    handler: ({ id }) =>
      datasources.some((d) => d.datasource_id === id)
        ? ok({ ok: true })
        : fail(404, "数据源不存在"),
  },
  {
    method: "GET",
    pattern: "/api/datasources/{}",
    handler: ({ id }) => {
      const row = datasources.find((d) => d.datasource_id === id);
      return row === undefined ? fail(404, "数据源不存在") : ok(row);
    },
  },
  {
    method: "PUT",
    pattern: "/api/datasources/{}",
    handler: ({ id, body }) => {
      const index = datasources.findIndex((d) => d.datasource_id === id);
      if (index < 0) return fail(404, "数据源不存在");
      const previous = datasources[index];
      const row = datasourceFrom(id, body);
      row.has_password = String(body.password ?? "") !== "" || previous.has_password === true;
      datasources[index] = row;
      return ok(row);
    },
  },
  {
    method: "DELETE",
    pattern: "/api/datasources/{}",
    handler: ({ id }) => {
      const row = datasources.find((d) => d.datasource_id === id);
      if (row === undefined) return fail(404, "数据源不存在");
      const used = tasks
        .filter((t) => t.source_datasource_id === id || t.target_datasource_id === id)
        .map((t) => t.name);
      if (used.length > 0) {
        return fail(409, `还有任务在用这条数据源：${used.join("、")}`, { tasks: used });
      }
      datasources = datasources.filter((d) => d.datasource_id !== id);
      return ok(row);
    },
  },

  // ---- 构建器（源端）
  {
    method: "POST",
    pattern: "/api/builder/tables",
    handler: () => ok(ORACLE_TABLES.map((name) => ({ owner: "APPUSER", name }))),
  },
  {
    method: "POST",
    pattern: "/api/builder/dblinks",
    handler: () => ok(["FIN_LINK", "CRM_LINK"]),
  },
  {
    method: "POST",
    pattern: "/api/builder/columns",
    handler: ({ body }) => ok(columnsOf(String(body.table ?? "").toUpperCase())),
  },
  {
    method: "POST",
    pattern: "/api/builder/sql-columns",
    handler: ({ body }) => {
      const sql = String(body.source_sql ?? "");
      if (sql.includes(";")) return fail(400, "自定义 SQL 不得包含分号", { kind: "request" });
      return ok(
        columnsOf("DEFAULT").map((c) => ({
          name: c.name,
          type: c.data_type,
          precision: c.precision,
          scale: c.scale,
          length: c.length,
          support: c.data_type === "NUMBER" && c.precision === null ? "needs_precision" : "ok",
        })),
      );
    },
  },
  {
    method: "POST",
    pattern: "/api/builder/sql",
    handler: ({ body }) => ok({ source_sql: generateSql(body as unknown as SpecShape) }),
  },
  {
    method: "POST",
    pattern: "/api/builder/preview",
    handler: ({ body }) => ok(previewRows(body.spec as SpecShape)),
  },

  // ---- 目标端元数据
  { method: "POST", pattern: "/api/target/tables", handler: () => ok({ tables: TARGET_TABLES }) },
  {
    method: "POST",
    pattern: "/api/target/columns",
    handler: ({ body }) => ok(targetMetadataFor(String(body.target_table ?? ""))),
  },
  {
    method: "POST",
    pattern: "/api/target/check",
    handler: ({ body }) => {
      const table = String(body.target_table ?? "");
      if (!TARGET_TABLES.includes(table)) {
        return ok({
          ok: false,
          findings: [
            {
              column: null,
              kind: "missing_column",
              expected: `表 ${table}`,
              actual: "不存在",
              message: "目标表还不存在，请先用下面的建表语句创建它",
            },
          ],
          suggested_ddl: suggestedDdl(body.spec as SpecShape),
        });
      }
      return ok({ ok: true, findings: [], suggested_ddl: null });
    },
  },
  {
    method: "POST",
    pattern: "/api/columns",
    handler: ({ body }) => {
      const spec = body.spec as SpecShape;
      return ok({
        columns: columnsOf(String(spec?.table ?? "").toUpperCase()).map((c) => ({
          name: c.name,
          type: c.data_type,
          precision: c.precision,
          scale: c.scale,
          length: c.length,
          support: "ok",
        })),
        target_ddl: suggestedDdl(spec),
      });
    },
  },

  // ---- 任务
  { method: "GET", pattern: "/api/tasks", handler: () => ok(tasks) },
  {
    method: "POST",
    pattern: "/api/tasks",
    handler: ({ body }) => {
      const row: TaskRow = {
        task_id: nextId("task"),
        name: String(body.name ?? "未命名任务"),
        source_datasource_id: String(body.source_datasource_id ?? ""),
        target_datasource_id: String(body.target_datasource_id ?? ""),
        spec: body.spec as SpecShape,
      };
      tasks.push(row);
      return ok(row);
    },
  },
  {
    method: "GET",
    pattern: "/api/tasks/{}/curl",
    handler: ({ id }) =>
      findTask(id) === undefined
        ? fail(404, "任务不存在")
        : ok({
            command: `curl --silent --cookie-jar '/tmp/db-qbs-session-${id}.cookie' --request POST 'http://127.0.0.1:5173/api/session' --header 'Content-Type: application/json' --data '{"username":"admin","password":"改成你的口令"}' > /dev/null && curl --cookie '/tmp/db-qbs-session-${id}.cookie' --request POST 'http://127.0.0.1:5173/api/runs' --header 'Content-Type: application/json' --data '{"task_id":"${id}"}'; rm -f '/tmp/db-qbs-session-${id}.cookie'`,
          }),
  },
  {
    method: "GET",
    pattern: "/api/tasks/{}",
    handler: ({ id }) => {
      const row = findTask(id);
      return row === undefined ? fail(404, "任务不存在") : ok(row);
    },
  },
  {
    method: "PUT",
    pattern: "/api/tasks/{}",
    handler: ({ id, body }) => {
      const row = findTask(id);
      if (row === undefined) return fail(404, "任务不存在");
      row.name = String(body.name ?? row.name);
      row.source_datasource_id = String(body.source_datasource_id ?? row.source_datasource_id);
      row.target_datasource_id = String(body.target_datasource_id ?? row.target_datasource_id);
      if (body.spec !== undefined) row.spec = body.spec as SpecShape;
      return ok(row);
    },
  },
  {
    method: "DELETE",
    pattern: "/api/tasks/{}",
    handler: ({ id }) => {
      const row = findTask(id);
      if (row === undefined) return fail(404, "任务不存在");
      tasks = tasks.filter((t) => t.task_id !== id);
      return ok(row);
    },
  },

  // ---- 运行
  {
    method: "GET",
    pattern: "/api/runs",
    handler: ({ query }) => {
      const taskId = query.get("task_id");
      runs.forEach(advance);
      const rows = runs
        .filter((r) => taskId === null || taskId === "" || r.task_id === taskId)
        .map(publicRow);
      return ok(rows);
    },
  },
  {
    method: "POST",
    pattern: "/api/runs",
    handler: ({ body }) => {
      const taskId = String(body.task_id ?? "");
      const task = findTask(taskId);
      if (task === undefined) return fail(404, "任务不存在");
      runs.forEach(advance);
      if (runs.some((r) => r.task_id === taskId && inFlight(r))) {
        return fail(409, "这个任务已经有一次运行在进行中");
      }
      const runId = newRunId();
      const row = finishedRow(task, {
        run_record_id: newRunRecordId(),
        run_id: runId,
        started_at: new Date().toISOString(),
        staging_table: `${task.spec.target_table}__stg_${runId}`,
        finished_at: null,
        outcome: null,
        target_table_effect: null,
        stage: "PREPARING",
        source_rows: null,
        staged_rows: null,
        sink_reported_rows: null,
        source_batches: null,
        received_batches: null,
        total_rows: 120_000,
        precount_ms: 260,
        fetch_ms: null,
        push_ms: null,
        commit_ms: null,
        count_ms: null,
        cursor_ms: null,
        seq: 0,
        rows_pushed: 0,
        bytes: 0,
        ms: 0,
        last_ts: null,
      });
      row.__startedMs = Date.now();
      runs.push(row);
      return ok({ run_record_id: row.run_record_id });
    },
  },
  {
    method: "POST",
    pattern: "/api/runs/{}/cancel",
    handler: ({ id }) => {
      const row = runs.find((r) => r.run_record_id === id);
      if (row === undefined) return fail(404, "运行记录不存在");
      advance(row);
      if (!inFlight(row) || (row.stage !== "PREPARING" && row.stage !== "STREAMING")) {
        return fail(409, "这次运行已经过了可以中止的阶段");
      }
      delete row.__startedMs;
      Object.assign(row, {
        stage: "FAILED",
        outcome: "FAILED",
        target_table_effect: "DISCARDED",
        finished_at: new Date().toISOString(),
        failure_kind: "CANCELLED",
        message: "运行被手动中止，暂存表已丢弃",
      });
      return ok({ message: "已请求中止，暂存表已丢弃" });
    },
  },
  {
    method: "GET",
    pattern: "/api/runs/{}",
    handler: ({ id }) => {
      const row = runs.find((r) => r.run_record_id === id);
      if (row === undefined) return fail(404, "运行记录不存在");
      advance(row);
      if (inFlight(row)) {
        return ok({
          run_record_id: row.run_record_id,
          run_id: row.run_id,
          source_sql: row.source_sql,
          evidence: row.evidence,
          staging_table: row.staging_table,
          started_at: row.started_at,
          stage: row.stage,
          total_rows: row.total_rows,
          precount_ms: row.precount_ms,
          seq: row.seq,
          rows_pushed: row.rows_pushed,
          bytes: row.bytes,
          ms: row.ms,
          last_ts: row.last_ts,
          live: true,
        });
      }
      return ok({ ...(publicRow(row) as Record<string, unknown>), live: false });
    },
  },
];

function datasourceFrom(id: string, body: Record<string, unknown>): DatasourceRow {
  if (body.kind === "mysql") {
    return {
      datasource_id: id,
      name: String(body.name ?? ""),
      kind: "mysql",
      agent_id: String(body.agent_id ?? ""),
      host: String(body.host ?? ""),
      port: Number(body.port ?? 3306),
      username: String(body.username ?? ""),
      database: String(body.database ?? ""),
      has_password: String(body.password ?? "") !== "",
    };
  }
  return {
    datasource_id: id,
    name: String(body.name ?? ""),
    kind: "oracle",
    connect_string: String(body.connect_string ?? ""),
    username: String(body.username ?? ""),
    has_password: String(body.password ?? "") !== "",
  };
}

function suggestedDdl(spec: SpecShape | undefined): string {
  if (spec === undefined) return "";
  const source = columnsOf(String(spec.table ?? "").toUpperCase());
  const lines = spec.columns.map((mapping) => {
    const src = source.find((c) => c.name === mapping.source) ?? source[0];
    const isKey = spec.primary_key.includes(mapping.target);
    return `  \`${mapping.target}\` ${mysqlTypeFor(src)}${isKey ? " NOT NULL" : " NULL"}`;
  });
  if (spec.primary_key.length > 0) {
    lines.push(`  PRIMARY KEY (${spec.primary_key.map((k) => `\`${k}\``).join(", ")})`);
  }
  return `CREATE TABLE \`${spec.target_table}\` (\n${lines.join(",\n")}\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;`;
}

// ------------------------------------------------------------- 匹配与分发

/** 字面量优先、占位符其次——与 Rust 那边的两趟匹配同规则。 */
function match(method: string, path: string): { route: Route; id: string } | null {
  const literal = ROUTES.find((r) => r.method === method && r.pattern === path);
  if (literal !== undefined) return { route: literal, id: "" };
  const segments = path.split("/");
  for (const route of ROUTES) {
    if (route.method !== method || !route.pattern.includes("{}")) continue;
    const pattern = route.pattern.split("/");
    if (pattern.length !== segments.length) continue;
    let id = "";
    let hit = true;
    for (let i = 0; i < pattern.length; i += 1) {
      if (pattern[i] === "{}") {
        if (segments[i] === "") {
          hit = false;
          break;
        }
        id = decodeURIComponent(segments[i]);
      } else if (pattern[i] !== segments[i]) {
        hit = false;
        break;
      }
    }
    if (hit) return { route, id };
  }
  return null;
}

function tokenFrom(header: string | string[] | undefined): string | null {
  const raw = Array.isArray(header) ? header.join("; ") : (header ?? "");
  for (const part of raw.split(";")) {
    const [name, ...rest] = part.trim().split("=");
    if (name === COOKIE_NAME) {
      const value = rest.join("=");
      return value === "" ? null : value;
    }
  }
  return null;
}

/**
 * vite 插件。**只有 `VITE_MOCK=1`（或 `--mode mock`）时才挂**，
 * 其余情况返回一个什么也不做的插件，dev server 行为与从前一致。
 */
export function mockApi(enabled: boolean) {
  return {
    name: "db-qbs-mock-api",
    apply: "serve" as const,
    configureServer(server: {
      middlewares: {
        use(handler: (req: MockReq, res: MockRes, next: () => void) => void): void;
      };
    }) {
      if (!enabled) return;
      // eslint-disable-next-line no-console
      console.log(
        "\n  [36m➜[0m  [1mmock 后端已挂上[0m：/api/* 由 mock/api.ts 应答，登录 admin / admin\n",
      );
      server.middlewares.use((req, res, next) => {
        const rawUrl = req.url ?? "/";
        if (!rawUrl.startsWith("/api/")) {
          next();
          return;
        }
        const url = new URL(rawUrl, "http://127.0.0.1");
        const method = (req.method ?? "GET").toUpperCase();
        const token = tokenFrom(req.headers.cookie);
        const hit = match(method, url.pathname);

        const send = (reply: Reply) => {
          const payload = JSON.stringify(reply.body);
          res.statusCode = reply.status;
          res.setHeader("Content-Type", "application/json; charset=utf-8");
          res.setHeader("Cache-Control", "no-store");
          if (reply.cookie !== undefined) res.setHeader("Set-Cookie", reply.cookie);
          res.end(payload);
        };

        // 认证是路由表上的一列，在分发之前判；认不出的路径在未登录时也回 401。
        const isPublic = hit?.route.public === true;
        if (!isPublic && (token === null || !sessions.has(token))) {
          send(fail(401, "会话已失效，请重新登录"));
          return;
        }
        if (hit === null) {
          send(fail(404, "没有这个接口"));
          return;
        }

        // 用字符串收而不是 Buffer：这个文件不依赖 @types/node，
        // 而 `setEncoding` 让多字节字符不会被切在两个 chunk 中间。
        req.setEncoding?.("utf8");
        const chunks: string[] = [];
        req.on("data", (chunk) => chunks.push(String(chunk)));
        req.on("end", () => {
          let body: Record<string, unknown> = {};
          const raw = chunks.join("");
          if (raw !== "") {
            try {
              const parsed = JSON.parse(raw);
              body = (typeof parsed === "object" && parsed !== null
                ? parsed
                : {}) as Record<string, unknown>;
            } catch {
              send(fail(400, "请求体不是合法的 JSON", { kind: "request" }));
              return;
            }
          }
          try {
            send(hit.route.handler({ body, id: hit.id, query: url.searchParams, token }));
          } catch (error) {
            send(fail(500, `mock 后端出错：${String(error)}`));
          }
        });
      });
    },
  };
}

// 种子数据在这里生成——上面所有辅助函数此刻都已初始化。
seedRuns();
