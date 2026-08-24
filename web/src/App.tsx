import {
  ChevronDown,
  ChevronRight,
  Database,
  LoaderCircle,
  Menu,
  PanelLeftClose,
  Plus,
  Radio,
  RefreshCw,
  Search,
  Server,
  Settings,
  TableProperties,
  Trash2,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";

import {
  createTask,
  deleteTask,
  emptySpec,
  fetchBuilderColumns,
  fetchBuilderDblinks,
  fetchBuilderSqlColumns,
  fetchBuilderTables,
  fetchTargetColumns,
  fetchTargetTables,
  listAgents,
  listDatasources,
  generateBuilderSql,
  listRunHistory,
  listTasks,
  taskInputFrom,
  updateTask,
} from "./api";
import type {
  Agent,
  BuilderColumn,
  BuilderSql,
  BuilderTable,
  Condition,
  RunHistory,
  RunParams,
  Task,
  Datasource,
  TargetColumn,
  TargetTableMetadata,
  TaskInput,
  TaskSpec,
} from "./api";
import { messageFrom } from "./errors";
import { AgentScreen } from "./AgentScreen";
import { JobCenterScreen } from "./JobCenterScreen";
import { latestRunByTask } from "./listing";
import { RunScreen } from "./RunScreen";
import { SettingsScreen } from "./SettingsScreen";
import {
  COMPARISONS,
  comparisonSymbol,
  defaultParameterName,
  defaultValueType,
  renameTargetField,
  targetFieldOf,
  VALUE_TYPE_LABELS,
} from "./spec";
import { StartRunDialog } from "./StartRunDialog";
import { DatasourceScreen } from "./DatasourceScreen";
import { FormField, Modal, ModalFooter } from "./ui";

type DialogState =
  | { kind: "create" }
  | { kind: "edit"; task: Task }
  | { kind: "rename"; task: Task }
  | { kind: "delete"; task: Task }
  /**
   * `rerun` 非空 = 从运行历史点「重跑」进来的：同一个发起对话框，只是按那次的运行参数
   * 预填（ADR-0041 增补 2）。重跑本身不是一种新的发起，所以这里不另开一个 kind。
   */
  | { kind: "start"; task: Task; rerun?: { runRecordId: string; runParams: RunParams } }
  | null;

/**
 * 导航四项（ADR-0044 §6 在 ADR-0043 §2 的三项上加了第一项）：
 * **目标端 Agent · 作业中心 · 数据源 · 系统设置**。
 * 「运行历史」独立屏随作业中心的合并整屏取消。
 *
 * agent 排在最前不是排版偏好：一条 MySQL 数据源必须先有一台已注册的 agent 才建得出来，
 * 所以新装一台机器时，这一屏是第一站。
 */
type Page = "agents" | "jobs" | "datasources" | "settings";

/** 旧的运行历史地址。**重定向而不是 404**：它还在旧链接与旧文档里流通，接住比让人撞墙便宜。 */
const RETIRED_HISTORY_HASHES = ["#history", "#/history"];

function pageFromHash(hash: string): Page {
  if (hash === "#agents") {
    return "agents";
  }
  if (hash === "#datasources") {
    return "datasources";
  }
  if (hash === "#settings") {
    return "settings";
  }
  return "jobs";
}

const SIDER_STORAGE_KEY = "db-qbs.sider-collapsed";

/**
 * 侧栏折叠状态**记在 `localStorage`**（ADR-0043 §8）：它是每个人对自己屏幕宽度的
 * 一次性偏好，每次进来重置等于每次重做一遍同一个决定。
 *
 * 读不到就当没折叠——隐私模式下 `localStorage` 会直接抛，不能让它把整屏带崩。
 */
function readCollapsed(): boolean {
  try {
    return window.localStorage.getItem(SIDER_STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

function writeCollapsed(collapsed: boolean) {
  try {
    window.localStorage.setItem(SIDER_STORAGE_KEY, collapsed ? "1" : "0");
  } catch {
    // 存不下就算了：折叠这一次照样生效，只是下次进来记不住。
  }
}

const NAV_ITEMS: readonly { page: Page; label: string }[] = [
  { page: "agents", label: "目标端 Agent" },
  { page: "jobs", label: "作业中心" },
  { page: "datasources", label: "数据源" },
  { page: "settings", label: "系统设置" },
];

function navIcon(page: Page, size: number) {
  switch (page) {
    case "agents":
      return <Radio size={size} aria-hidden="true" />;
    case "jobs":
      return <Database size={size} aria-hidden="true" />;
    case "datasources":
      return <Server size={size} aria-hidden="true" />;
    case "settings":
      return <Settings size={size} aria-hidden="true" />;
  }
}

const emptyTask: TaskInput = {
  name: "",
  source_datasource_id: "",
  target_datasource_id: "",
  spec: emptySpec(),
};

export function App() {
  const [page, setPage] = useState<Page>(() =>
    pageFromHash(window.location.hash),
  );
  const [collapsed, setCollapsed] = useState<boolean>(readCollapsed);
  const [tasks, setTasks] = useState<Task[] | null>(null);
  // 数据源清单（ADR-0037）。管理屏在导航第二项——增删改之后要重读，
  // 所以这里不再是「读一次就完」。
  const [datasources, setDatasources] = useState<Datasource[]>([]);
  const [datasourcesLoading, setDatasourcesLoading] = useState(true);
  /**
   * 目标端 agent 注册表（ADR-0044）。**与数据源同一次读取**：数据源屏那一列要显示
   * 「这条库走哪台 agent、它在不在线」，两半分开读会出现一半新一半旧的画面。
   */
  const [agents, setAgents] = useState<Agent[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(true);
  /**
   * 作业中心那一行里「最近一次运行」的数据源。
   *
   * 首屏仍与任务清单同一次读取；只有存在未结束 run 时，才短轮询运行历史这一半，
   * 用来驱动迁移进度列。任务定义不跟着轮询，避免筛选与编辑入口被后台刷新扰动。
   */
  const [runHistory, setRunHistory] = useState<RunHistory[]>([]);
  const [dialog, setDialog] = useState<DialogState>(null);
  const [activeRun, setActiveRun] = useState<{
    task: Task;
    runRecordId: string;
  } | null>(null);

  const loadList = useCallback(async () => {
    setRefreshing(true);
    try {
      // 两份一起读、一起判成败：作业中心的一行是「任务 + 它最近一次运行」，
      // 少了任何一半这张表都不成立。
      const [nextTasks, nextRuns] = await Promise.all([
        listTasks(),
        listRunHistory({}),
      ]);
      setTasks(nextTasks);
      setRunHistory(nextRuns);
      setLoadError(null);
    } catch (error) {
      setLoadError(messageFrom(error));
    } finally {
      setRefreshing(false);
    }
  }, []);

  const refreshRunHistory = useCallback(async () => {
    const nextRuns = await listRunHistory({});
    setRunHistory(nextRuns);
    setLoadError(null);
  }, []);

  useEffect(() => {
    void loadList();
  }, [loadList]);

  const hasLiveRun = useMemo(
    () => runHistory.some((run) => isLiveRunHistory(run)),
    [runHistory],
  );

  useEffect(() => {
    if (!hasLiveRun) {
      return;
    }
    let requestInFlight = false;
    const poll = window.setInterval(() => {
      if (document.visibilityState !== "visible" || requestInFlight) {
        return;
      }
      requestInFlight = true;
      void refreshRunHistory()
        .catch((error) => setLoadError(messageFrom(error)))
        .finally(() => {
          requestInFlight = false;
        });
    }, 1000);
    return () => window.clearInterval(poll);
  }, [hasLiveRun, refreshRunHistory]);

  const loadDatasources = useCallback(async () => {
    setDatasourcesLoading(true);
    try {
      // 读不到数据源不该把整个作业中心打成错误——构建器会以「没有可选的数据源」自陈。
      const [nextDatasources, nextAgents] = await Promise.all([
        listDatasources(),
        listAgents(),
      ]);
      setDatasources(nextDatasources);
      setAgents(nextAgents);
    } catch {
      setDatasources([]);
      setAgents([]);
    } finally {
      setDatasourcesLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadDatasources();
  }, [loadDatasources]);

  const refreshAgents = useCallback(async () => {
    try {
      setAgents(await listAgents());
    } catch {
      // Agent 状态刷新是辅助信息，失败时保留最近一次成功读取的状态。
    }
  }, []);

  useEffect(() => {
    let requestInFlight = false;
    const poll = window.setInterval(() => {
      if (document.visibilityState !== "visible" || requestInFlight) {
        return;
      }
      requestInFlight = true;
      void refreshAgents().finally(() => {
        requestInFlight = false;
      });
    }, 15000);
    return () => window.clearInterval(poll);
  }, [refreshAgents]);

  useEffect(() => {
    function handleHashChange() {
      // 旧的 `#history` 打进来就地换成作业中心的地址：**不留一个还能回去的空屏**。
      if (RETIRED_HISTORY_HASHES.includes(window.location.hash)) {
        window.location.replace("#jobs");
        return;
      }
      setPage(pageFromHash(window.location.hash));
    }
    handleHashChange();
    window.addEventListener("hashchange", handleHashChange);
    return () => window.removeEventListener("hashchange", handleHashChange);
  }, []);

  const latestRuns = useMemo(() => latestRunByTask(runHistory), [runHistory]);

  function toggleSider() {
    setCollapsed((current) => {
      writeCollapsed(!current);
      return !current;
    });
  }

  function openCreateDialog() {
    setDialog({ kind: "create" });
  }

  function closeDialog() {
    setDialog(null);
  }

  function navigate(nextPage: Page) {
    setActiveRun(null);
    setPage(nextPage);
    window.location.hash = nextPage;
  }

  async function handleCreate(input: TaskInput) {
    const created = await createTask(input);
    setTasks((currentTasks) => [...(currentTasks ?? []), created]);
  }

  async function handleUpdate(task: Task, input: TaskInput) {
    const updated = await updateTask(task.task_id, input);
    setTasks(
      (currentTasks) =>
        currentTasks?.map((currentTask) =>
          currentTask.task_id === updated.task_id ? updated : currentTask,
        ) ?? [updated],
    );
  }

  async function handleDelete(task: Task) {
    await deleteTask(task.task_id);
    setTasks(
      (currentTasks) =>
        currentTasks?.filter(
          (currentTask) => currentTask.task_id !== task.task_id,
        ) ?? [],
    );
  }

  // 运行详情不属于任何导航项——面包屑早就这么认了，导航高亮以前还停在上一页。
  const navPage = activeRun !== null ? null : page;
  const foldLabel = collapsed ? "展开侧边栏" : "折叠侧边栏";

  return (
    <div className={`app-shell ${collapsed ? "is-collapsed" : ""}`}>
      <aside className="sidebar">
        <div className="product-brand">
          <span className="brand-mark">Q</span>
          <span className="brand-text">db-qbs</span>
        </div>
        <nav aria-label="主导航">
          {NAV_ITEMS.map((item) => (
            <a
              key={item.page}
              className={`nav-item ${navPage === item.page ? "is-active" : ""}`}
              href={`#${item.page}`}
              // 折叠态只剩图标，菜单名靠 `title` 报出来（ADR-0043 §8）。
              title={item.label}
              aria-current={navPage === item.page ? "page" : undefined}
              onClick={(event) => {
                event.preventDefault();
                navigate(item.page);
              }}
            >
              {navIcon(item.page, 16)}
              <span className="nav-text">{item.label}</span>
            </a>
          ))}
        </nav>
      </aside>

      <main className="main-column">
        <header className="topbar">
          {/* 折叠触发器在顶栏最左（参照物的 `menu-fold ⇄ menu-unfold`）。 */}
          <button
            className="fold-toggle"
            type="button"
            title={foldLabel}
            aria-label={foldLabel}
            aria-expanded={!collapsed}
            onClick={toggleSider}
          >
            {collapsed ? (
              <Menu size={17} aria-hidden="true" />
            ) : (
              <PanelLeftClose size={17} aria-hidden="true" />
            )}
          </button>
          <span className="mobile-brand">db-qbs</span>
          <nav className="mobile-nav" aria-label="主导航">
            {NAV_ITEMS.map((item) => (
              <button
                key={item.page}
                className={navPage === item.page ? "is-active" : ""}
                type="button"
                aria-current={navPage === item.page ? "page" : undefined}
                onClick={() => navigate(item.page)}
              >
                {navIcon(item.page, 14)}
                {item.label}
              </button>
            ))}
          </nav>
          <span className="breadcrumb">
            数据导入 <span aria-hidden="true">/</span>{" "}
            <strong>{activeRun !== null ? "运行详情" : pageLabel(page)}</strong>
          </span>
          <span className="topbar-right">
            <span className="environment">当前工作台</span>
          </span>
        </header>

        <div className="content">
          {loadError !== null && page === "jobs" && (
            <div className="notice is-error" role="alert">
              <span>{loadError}</span>
              <button
                className="text-button"
                type="button"
                onClick={() => void loadList()}
              >
                重新加载
              </button>
            </div>
          )}

          {activeRun !== null && (
            <RunScreen
              task={activeRun.task}
              runRecordId={activeRun.runRecordId}
              onBack={() => {
                setActiveRun(null);
                void loadList();
              }}
              onRelaunch={() => setDialog({ kind: "start", task: activeRun.task })}
              onEditTask={() => setDialog({ kind: "edit", task: activeRun.task })}
            />
          )}

          {activeRun === null && page === "jobs" && (
            <JobCenterScreen
              tasks={tasks}
              datasources={datasources}
              latestRuns={latestRuns}
              refreshing={refreshing}
              onRefresh={() => void loadList()}
              onCreate={openCreateDialog}
              onEdit={(task) => setDialog({ kind: "edit", task })}
              onRename={(task) => setDialog({ kind: "rename", task })}
              onDelete={(task) => setDialog({ kind: "delete", task })}
              onStart={(task) => setDialog({ kind: "start", task })}
              onRerun={(task, row) =>
                setDialog({
                  kind: "start",
                  task,
                  rerun: {
                    runRecordId: row.run_record_id,
                    runParams: row.run_params,
                  },
                })
              }
              onChanged={() => void loadList()}
            />
          )}

          {activeRun === null && page === "agents" && (
            <AgentScreen
              agents={agents}
              datasources={datasources}
              loading={datasourcesLoading}
              onChanged={loadDatasources}
            />
          )}

          {activeRun === null && page === "datasources" && (
            <DatasourceScreen
              datasources={datasources}
              agents={agents}
              tasks={tasks ?? []}
              loading={datasourcesLoading}
              onChanged={loadDatasources}
            />
          )}

          {activeRun === null && page === "settings" && <SettingsScreen />}
        </div>

        {page === "jobs" && dialog?.kind === "create" && (
          <TaskFormDialog
            title="新建任务"
            initial={emptyTask}
            datasources={datasources}
            submitLabel="新建"
            onClose={closeDialog}
            onSubmit={handleCreate}
          />
        )}
        {page === "jobs" && dialog?.kind === "edit" && (
          <TaskFormDialog
            title={`编辑 · ${dialog.task.name}`}
            initial={taskInputFrom(dialog.task)}
            datasources={datasources}
            submitLabel="保存"
            hideName
            onClose={closeDialog}
            onSubmit={(input) => handleUpdate(dialog.task, input)}
          />
        )}
        {page === "jobs" && dialog?.kind === "rename" && (
          <RenameDialog
            task={dialog.task}
            onClose={closeDialog}
            onSubmit={(input) => handleUpdate(dialog.task, input)}
          />
        )}
        {page === "jobs" && dialog?.kind === "delete" && (
          <DeleteDialog
            task={dialog.task}
            onClose={closeDialog}
            onDelete={() => handleDelete(dialog.task)}
          />
        )}
        {dialog?.kind === "start" && (
          <StartRunDialog
            task={dialog.task}
            rerun={dialog.rerun}
            onClose={closeDialog}
            onStarted={(runRecordId) => {
              setActiveRun({ task: dialog.task, runRecordId });
              closeDialog();
            }}
          />
        )}
      </main>
    </div>
  );
}

function pageLabel(page: Page): string {
  switch (page) {
    case "agents":
      return "目标端 Agent";
    case "jobs":
      return "作业中心";
    case "datasources":
      return "数据源";
    case "settings":
      return "系统设置";
  }
}

type TargetMetaState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "failed"; message: string }
  | { kind: "ready"; table: string; data: TargetTableMetadata };

type SourceQueryMode = "table" | "sql";

function normalizeTaskSpecForEditor(spec: TaskSpec): TaskSpec {
  const isSql = Boolean(spec.source_sql?.trim());
  return {
    ...spec,
    conditions: isSql
      ? []
      : spec.conditions.map((condition) => ({
          ...condition,
          value_source: "constant",
        })),
    order_by: [],
  };
}

/**
 * 任务定义编辑器 —— 构建器本身就是任务定义（ADR-0036 §1）。
 *
 * 它不再是「一次性生成四个字段、选择态丢弃」的向导：结构化规格是唯一真相源，
 * 选表、勾列、勾主键、条件**全部原样存进任务定义**，再次打开还在（所有者裁定 6）。
 * SQL 是规格的派生面，只读展示，v1 没有编辑入口。
 */
function TaskFormDialog({
  title,
  initial,
  datasources,
  submitLabel,
  hideName = false,
  onClose,
  onSubmit,
}: {
  title: string;
  initial: TaskInput;
  datasources: Datasource[];
  submitLabel: string;
  hideName?: boolean;
  onClose: () => void;
  onSubmit: (input: TaskInput) => Promise<void>;
}) {
  const [name, setName] = useState(initial.name);
  const [sourceDatasourceId, setSourceDatasourceId] = useState(
    initial.source_datasource_id,
  );
  const [targetDatasourceId, setTargetDatasourceId] = useState(
    initial.target_datasource_id,
  );
  const [sourceQueryMode, setSourceQueryMode] = useState<SourceQueryMode>(
    initial.spec.source_sql?.trim() === "" || initial.spec.source_sql === undefined
      ? "table"
      : "sql",
  );
  const [spec, setSpec] = useState<TaskSpec>(() =>
    normalizeTaskSpecForEditor(initial.spec),
  );
  const [tables, setTables] = useState<BuilderTable[]>([]);
  const [dblinks, setDblinks] = useState<string[]>([]);
  const [dblinksLoading, setDblinksLoading] = useState(false);
  const [columns, setColumns] = useState<BuilderColumn[]>([]);
  const [loading, setLoading] = useState<"tables" | "columns" | null>(null);
  const [builderError, setBuilderError] = useState<string | null>(null);
  const [sourceTableFilter, setSourceTableFilter] = useState("");
  const [sourceExpandedOwners, setSourceExpandedOwners] = useState<Set<string>>(
    () => new Set(initial.spec.owner === "" ? [] : [initial.spec.owner]),
  );
  const [targetTableFilter, setTargetTableFilter] = useState(
    initial.spec.target_table,
  );
  const [targetTreeOpen, setTargetTreeOpen] = useState(true);
  const [sql, setSql] = useState<BuilderSql | null>(null);
  const [sqlError, setSqlError] = useState<string | null>(null);
  // 目标端元数据（ADR-0038 §3/§8）。**结果纯瞬态**：只活在这里，不进任务定义、
  // 不进 SQLite，刷新即丢。映射关系本身要存，目标表结构快照不存。
  const [targetTables, setTargetTables] = useState<string[]>([]);
  const [targetTablesLoading, setTargetTablesLoading] = useState(false);
  const [targetTablesError, setTargetTablesError] = useState<string | null>(null);
  const [targetMeta, setTargetMeta] = useState<TargetMetaState>({ kind: "idle" });
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const oracleDatasources = datasources.filter(
    (datasource) => datasource.kind === "oracle",
  );
  const mysqlDatasources = datasources.filter(
    (datasource) => datasource.kind === "mysql",
  );
  const tableKey = spec.owner === "" ? "" : tableKeyFor({ owner: spec.owner, name: spec.table });
  const sourceTableGroups = useMemo(
    () => groupSourceTables(tables, sourceTableFilter),
    [tables, sourceTableFilter],
  );
  const filteredTargetTables = useMemo(
    () => filterNames(targetTables, targetTableFilter),
    [targetTables, targetTableFilter],
  );
  const sourceTableLabel =
    spec.owner === "" ? "" : `${spec.owner}.${spec.table}`;
  const selectedTargetDatasource = mysqlDatasources.find(
    (datasource) => datasource.datasource_id === targetDatasourceId,
  );
  const dictionary = useMemo(
    () => new Map(columns.map((column) => [column.name, column])),
    [columns],
  );
  // 编辑既有任务时字典还没读，但选中的列名规格里就有——照样列出来，
  // 只是字典那几列显示为未知。不读一次 Oracle 就改不了条件，是没必要的门槛。
  const columnNames = useMemo(() => {
    const names = columns.map((column) => column.name);
    for (const mapping of spec.columns) {
      if (!names.includes(mapping.source)) {
        names.push(mapping.source);
      }
    }
    return names;
  }, [columns, spec.columns]);
  const selectedSourceColumns = useMemo(
    () => spec.columns.map((mapping) => mapping.source),
    [spec.columns],
  );
  const allSourceColumnsSelected =
    columnNames.length > 0 &&
    columnNames.every((column) => selectedSourceColumns.includes(column));
  const mappedTargetsComplete = spec.columns.every(
    (mapping) => mapping.target.trim() !== "",
  );
  const sourceQueryComplete =
    sourceQueryMode === "sql"
      ? Boolean(spec.source_sql?.trim())
      : spec.owner !== "";
  const specComplete =
    sourceQueryComplete &&
    spec.columns.length > 0 &&
    mappedTargetsComplete &&
    spec.primary_key.length > 0;

  useEffect(() => {
    if (sourceDatasourceId === "" || sourceQueryMode === "sql") {
      setDblinks([]);
      setDblinksLoading(false);
      return;
    }
    let active = true;
    setDblinksLoading(true);
    void fetchBuilderDblinks(sourceDatasourceId)
      .then((nextDblinks) => {
        if (active) {
          setDblinks(nextDblinks);
        }
      })
      .catch(() => {
        // DBLINK suggestions are optional; manual input remains available when discovery fails.
        if (active) {
          setDblinks([]);
        }
      })
      .finally(() => {
        if (active) {
          setDblinksLoading(false);
        }
      });
    return () => {
      active = false;
    };
  }, [sourceDatasourceId, sourceQueryMode]);

  // 目标表清单跟着目标数据源走。取不到不打断建任务——`datalist` 空掉即可，
  // 目标表名**仍然能手打**（ADR-0039 §5：记得全名的人不必翻列表）。
  useEffect(() => {
    if (targetDatasourceId === "") {
      setTargetTables([]);
      setTargetTablesError(null);
      setTargetMeta({ kind: "idle" });
      return;
    }
    void loadTargetTables();
  }, [targetDatasourceId]);

  // 目标列参考表按需取：**每次都真连一次 MySQL**（ADR-0038 §8 不缓存）。
  // 触发点是目标表名填完（失焦 / 从下拉选中），不是每敲一个字符——那会把库敲穿。
  async function loadTargetTables() {
    if (targetDatasourceId === "") {
      setTargetTables([]);
      setTargetTablesError(null);
      return;
    }
    setTargetTablesLoading(true);
    setTargetTablesError(null);
    try {
      setTargetTables(await fetchTargetTables(targetDatasourceId));
    } catch (loadError) {
      setTargetTables([]);
      setTargetTablesError(messageFrom(loadError));
    } finally {
      setTargetTablesLoading(false);
    }
  }

  function loadTargetColumns(force = false) {
    const table = spec.target_table.trim();
    if (targetDatasourceId === "" || table === "") {
      setTargetMeta({ kind: "idle" });
      return;
    }
    if (!force && targetMeta.kind === "ready" && targetMeta.table === table) {
      return;
    }
    setTargetMeta({ kind: "loading" });
    void fetchTargetColumns(targetDatasourceId, table)
      .then((data) => setTargetMeta({ kind: "ready", table, data }))
      .catch((loadError) =>
        setTargetMeta({ kind: "failed", message: messageFrom(loadError) }),
      );
  }

  function updateTargetTable(nextTable: string) {
    updateSpec({
      target_table: nextTable,
      columns: spec.columns.map((mapping) => ({ ...mapping, target: "" })),
      primary_key: [],
    });
    setTargetMeta({ kind: "idle" });
  }

  // SQL 现算：规格一变就换一份（ADR-0036 §2 不存 SQL）。规格还没成形时不去打扰后端——
  // 它会拿 `validate()` 的报错回话，那不是「SQL 生成失败」，只是还没填完。
  useEffect(() => {
    if (!specComplete) {
      setSql(null);
      setSqlError(null);
      return;
    }
    let active = true;
    const timeout = window.setTimeout(() => {
      void generateBuilderSql(spec)
        .then((generated) => {
          if (active) {
            setSql(generated);
            setSqlError(null);
          }
        })
        .catch((generateError) => {
          if (active) {
            setSql(null);
            setSqlError(messageFrom(generateError));
          }
        });
    }, 250);
    return () => {
      active = false;
      window.clearTimeout(timeout);
    };
  }, [spec, specComplete]);

  function updateSpec(change: Partial<TaskSpec>) {
    setSpec((current) => ({ ...current, ...change }));
  }

  function switchSourceQueryMode(nextMode: SourceQueryMode) {
    if (nextMode === sourceQueryMode) {
      return;
    }
    setSourceQueryMode(nextMode);
    setTables([]);
    setDblinks([]);
    setColumns([]);
    setSourceTableFilter("");
    setSourceExpandedOwners(new Set());
    updateSpec({
      source_sql: nextMode === "sql" ? "" : undefined,
      dblink: undefined,
      owner: "",
      table: "",
      columns: [],
      primary_key: [],
      conditions: [],
      order_by: [],
    });
  }

  async function loadTables() {
    if (sourceQueryMode !== "table") {
      return;
    }
    setLoading("tables");
    setBuilderError(null);
    try {
      const nextTables = await fetchBuilderTables(
        sourceDatasourceId,
        spec.dblink?.trim() ?? "",
      );
      setTables(nextTables);
      if (spec.owner !== "") {
        setSourceExpandedOwners((current) => new Set(current).add(spec.owner));
      }
    } catch (loadError) {
      setBuilderError(messageFrom(loadError));
    } finally {
      setLoading(null);
    }
  }

  async function loadColumns() {
    if (sourceQueryMode === "sql") {
      const sourceSql = spec.source_sql?.trim() ?? "";
      if (sourceDatasourceId === "" || sourceSql === "") {
        return;
      }
      setLoading("columns");
      setBuilderError(null);
      try {
        const nextColumns = await fetchBuilderSqlColumns({
          datasource_id: sourceDatasourceId,
          source_sql: sourceSql,
        });
        setColumns(
          nextColumns.map((column) => ({
            name: column.name,
            data_type: column.type,
            precision: column.precision,
            scale: column.scale,
            length: column.length,
            // SQL describe does not expose column nullability; keep the field neutral in the UI.
            nullable: true,
          })),
        );
        updateSpec({
          columns: nextColumns.map((column) => ({
            source: column.name,
            target: column.name,
          })),
          primary_key: [],
          conditions: [],
          order_by: [],
        });
      } catch (loadError) {
        setBuilderError(messageFrom(loadError));
      } finally {
        setLoading(null);
      }
      return;
    }
    if (spec.owner === "" || spec.table === "") {
      return;
    }
    setLoading("columns");
    setBuilderError(null);
    try {
      setColumns(
        await fetchBuilderColumns({
          datasource_id: sourceDatasourceId,
          dblink: spec.dblink?.trim() ?? "",
          owner: spec.owner,
          table: spec.table,
        }),
      );
    } catch (loadError) {
      setBuilderError(messageFrom(loadError));
    } finally {
      setLoading(null);
    }
  }

  function selectTable(key: string) {
    const table = tables.find((candidate) => tableKeyFor(candidate) === key);
    setColumns([]);
    // 换表就等于换一份规格：列、主键、条件全是上一张表的列名，留着只会生成一段引用
    // 不存在列的 SQL。目标表名是用户自己写的，不跟着清。
    updateSpec({
      owner: table?.owner ?? "",
      table: table?.name ?? "",
      columns: [],
      primary_key: [],
      conditions: [],
      order_by: [],
    });
    if (table !== undefined) {
      setSourceExpandedOwners((current) => new Set(current).add(table.owner));
    }
  }

  // 勾源列只表达“本次要搬这列”。目标字段要等目标表列读取后，在字段映射区明确选择。
  function toggleColumn(column: string) {
    if (sourceQueryMode === "sql") {
      return;
    }
    const mapping = spec.columns.find((candidate) => candidate.source === column);
    if (mapping === undefined) {
      updateSpec({ columns: [...spec.columns, { source: column, target: "" }] });
      return;
    }
    // 主键存的是目标字段，取消源列时按当前目标名摘掉。
    updateSpec({
      columns: spec.columns.filter((candidate) => candidate.source !== column),
      primary_key: spec.primary_key.filter((name) => name !== mapping.target),
    });
  }

  function toggleAllColumns() {
    if (sourceQueryMode === "sql") {
      return;
    }
    if (allSourceColumnsSelected) {
      updateSpec({ columns: [], primary_key: [], conditions: [], order_by: [] });
      return;
    }
    const current = new Map(spec.columns.map((mapping) => [mapping.source, mapping]));
    updateSpec({
      columns: columnNames.map(
        (source) => current.get(source) ?? { source, target: "" },
      ),
    });
  }

  function toggleSourceOwner(owner: string) {
    setSourceExpandedOwners((current) => {
      const next = new Set(current);
      if (next.has(owner)) {
        next.delete(owner);
      } else {
        next.add(owner);
      }
      return next;
    });
  }

  function selectTargetTable(table: string) {
    updateTargetTable(table);
    setTargetTableFilter(table);
  }

  // 改目标名时主键跟着走（ADR-0039 增补 1）。换名字空间的单点在 `spec.ts`，
  // 这里不再各处 `find()`。
  function renameTarget(column: string, nextTarget: string) {
    updateSpec(renameTargetField(spec, column, nextTarget));
  }

  function fillSameNameTargets() {
    if (targetMeta.kind !== "ready") {
      return;
    }
    const targetByUpper = new Map(
      targetMeta.data.columns.map((column) => [
        column.name.toUpperCase(),
        column.name,
      ]),
    );
    const next = spec.columns.reduce<Pick<TaskSpec, "columns" | "primary_key">>(
      (current, mapping) => {
        const target = targetByUpper.get(mapping.source.toUpperCase());
        return target === undefined
          ? current
          : renameTargetField(current, mapping.source, target);
      },
      { columns: spec.columns, primary_key: spec.primary_key },
    );
    updateSpec(next);
  }

  function toggleKey(column: string) {
    const target = targetFieldOf(spec.columns, column);
    if (target === undefined || target.trim() === "") {
      return;
    }
    updateSpec({
      primary_key: spec.primary_key.includes(target)
        ? spec.primary_key.filter((name) => name !== target)
        : [...spec.primary_key, target],
    });
  }

  function addCondition() {
    const column = columnNames[0];
    if (column === undefined) {
      return;
    }
    updateSpec({
      conditions: [
        ...spec.conditions,
        {
          column,
          operator: "eq",
          value_type: defaultValueType(dictionary.get(column)?.data_type),
          parameter: defaultParameterName(
            column,
            spec.conditions.map((condition) => condition.parameter),
          ),
          value_source: "constant",
          constant: "",
        },
      ],
    });
  }

  function updateCondition(index: number, change: Partial<Condition>) {
    updateSpec({
      conditions: spec.conditions.map((condition, position) =>
        position === index
          ? { ...condition, ...change, value_source: "constant" }
          : condition,
      ),
    });
  }

  function removeCondition(index: number) {
    updateSpec({
      conditions: spec.conditions.filter((_, position) => position !== index),
    });
  }

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      await onSubmit({
        name,
        source_datasource_id: sourceDatasourceId,
        target_datasource_id: targetDatasourceId,
        spec: normalizeTaskSpecForEditor(spec),
      });
      onClose();
    } catch (submitError) {
      setError(messageFrom(submitError));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Modal title={title} onClose={onClose} busy={submitting} wide>
      <form onSubmit={(event) => void handleSubmit(event)}>
        <div className="modal-body form-stack">
          {!hideName && (
            <FormField label="任务名称">
              <input
                autoFocus
                required
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
            </FormField>
          )}

          <section className="builder-guide" aria-labelledby="builder-datasource-title">
            <header>
              <div>
                <strong id="builder-datasource-title">数据源</strong>
                <span>选择本任务使用的源端和目标端连接</span>
              </div>
            </header>
            <div className="builder-controls">
              <FormField label="源端（Oracle）">
                <select
                  required
                  value={sourceDatasourceId}
                  onChange={(event) => {
                    setSourceDatasourceId(event.target.value);
                    // 换源端等于换一个库：表清单与列字典都是上一个库的，留着会选出不存在的表。
                    setTables([]);
                    setColumns([]);
                    setDblinks([]);
                    setSourceTableFilter("");
                    setSourceExpandedOwners(new Set());
                    updateSpec({
                      source_sql: sourceQueryMode === "sql" ? "" : undefined,
                      dblink: undefined,
                      owner: "",
                      table: "",
                      columns: [],
                      primary_key: [],
                      conditions: [],
                      order_by: [],
                    });
                  }}
                >
                  <option value="">
                    {oracleDatasources.length === 0 ? "尚无 Oracle 数据源" : "请选择"}
                  </option>
                  {oracleDatasources.map((datasource) => (
                    <option
                      key={datasource.datasource_id}
                      value={datasource.datasource_id}
                    >
                      {datasource.name}
                    </option>
                  ))}
                </select>
                {oracleDatasources.length === 0 && <DatasourceHint />}
              </FormField>
              {sourceQueryMode === "table" && (
                <FormField
                  label="源端 DBLINK（可选）"
                  badge={
                    dblinksLoading
                      ? "读取中"
                      : dblinks.length > 0
                        ? String(dblinks.length) + " 个可选"
                        : undefined
                  }
                  neutralBadge
                >
                  <input
                    list="source-dblinks"
                    value={spec.dblink ?? ""}
                    disabled={sourceDatasourceId === ""}
                    placeholder={dblinksLoading ? "正在读取" : "输入或选择，如 FA"}
                    onChange={(event) => {
                      const dblink = event.target.value.trim();
                      setTables([]);
                      setColumns([]);
                      setSourceTableFilter("");
                      setSourceExpandedOwners(new Set());
                      updateSpec({
                        dblink: dblink === "" ? undefined : dblink,
                        owner: "",
                        table: "",
                        columns: [],
                        primary_key: [],
                        conditions: [],
                        order_by: [],
                      });
                    }}
                  />
                  <datalist id="source-dblinks">
                    {dblinks.map((dblink) => (
                      <option key={dblink} value={dblink} />
                    ))}
                  </datalist>
                </FormField>
              )}
              <FormField label="目标端（MySQL）">
                <select
                  required
                  value={targetDatasourceId}
                  onChange={(event) => {
                    setTargetDatasourceId(event.target.value);
                    updateSpec({
                      target_table: "",
                      columns: spec.columns.map((mapping) => ({
                        ...mapping,
                        target: "",
                      })),
                      primary_key: [],
                    });
                    setTargetTableFilter("");
                    setTargetTables([]);
                    setTargetTablesError(null);
                    setTargetMeta({ kind: "idle" });
                  }}
                >
                  <option value="">
                    {mysqlDatasources.length === 0 ? "尚无 MySQL 数据源" : "请选择"}
                  </option>
                  {mysqlDatasources.map((datasource) => (
                    <option
                      key={datasource.datasource_id}
                      value={datasource.datasource_id}
                    >
                      {datasource.name}
                    </option>
                  ))}
                </select>
                {mysqlDatasources.length === 0 && <DatasourceHint />}
              </FormField>
            </div>
            <div className="builder-query-mode">
              <span>源端查询方式</span>
              <div className="builder-mode-buttons" role="tablist" aria-label="源端查询方式">
                <button
                  className={sourceQueryMode === "table" ? "button is-primary" : "button"}
                  type="button"
                  role="tab"
                  aria-selected={sourceQueryMode === "table"}
                  onClick={() => switchSourceQueryMode("table")}
                >
                  按表选择
                </button>
                <button
                  className={sourceQueryMode === "sql" ? "button is-primary" : "button"}
                  type="button"
                  role="tab"
                  aria-selected={sourceQueryMode === "sql"}
                  onClick={() => switchSourceQueryMode("sql")}
                >
                  自定义 SQL
                </button>
              </div>
            </div>
          </section>

          <section className="builder-guide" aria-labelledby="builder-source-title">
            <header>
              <div>
                <strong id="builder-source-title">
                  {sourceQueryMode === "table" ? "源表" : "源 SQL"}
                </strong>
                <span>
                  {sourceQueryMode === "table"
                    ? "按库展开表，输入关键字可筛选"
                    : "输入一条只读 SELECT，读取结果列后继续做目标映射"}
                </span>
              </div>
              <button
                className="button is-ghost"
                type="button"
                onClick={() => void (sourceQueryMode === "table" ? loadTables() : loadColumns())}
                disabled={
                  loading !== null ||
                  sourceDatasourceId === "" ||
                  (sourceQueryMode === "sql" && !Boolean(spec.source_sql?.trim()))
                }
              >
                {loading !== null ? (
                  <LoaderCircle className="is-spinning" size={15} />
                ) : sourceQueryMode === "table" ? (
                  <RefreshCw size={15} />
                ) : (
                  <TableProperties size={15} />
                )}
                {loading !== null
                  ? "读取中"
                  : sourceQueryMode === "table"
                    ? "读取表"
                    : "读取列"}
              </button>
            </header>
            {sourceQueryMode === "sql" ? (
              <div className="source-sql-editor">
                <FormField label="自定义 SQL">
                  <textarea
                    required
                    rows={8}
                    value={spec.source_sql ?? ""}
                    placeholder={"SELECT *\nFROM APP.T_CUSTOMER@POC_LINK_A\nWHERE STATUS = 1"}
                    onChange={(event) => {
                      setColumns([]);
                      updateSpec({
                        source_sql: event.target.value,
                        columns: [],
                        primary_key: [],
                        conditions: [],
                        order_by: [],
                      });
                    }}
                  />
                </FormField>
              </div>
            ) : (
            <div className="tree-picker">
              <div className="tree-picker-toolbar">
                <label className="tree-search">
                  <Search size={15} aria-hidden="true" />
                  <input
                    value={sourceTableFilter}
                    placeholder="搜索库 / 表名"
                    disabled={tables.length === 0}
                    onChange={(event) => setSourceTableFilter(event.target.value)}
                  />
                </label>
                <span className="tree-count">
                  {tables.length === 0 ? "未读取" : `${tables.length} 张表`}
                </span>
              </div>
              {tables.length === 0 ? (
                <p className="tree-empty">
                  先选择源端数据源并读取表。
                </p>
              ) : (
                <div className="schema-tree" role="tree" aria-label="Oracle 表">
                  {sourceTableGroups.map((group) => {
                    const expanded =
                      sourceTableFilter.trim() !== "" ||
                      sourceExpandedOwners.has(group.owner) ||
                      group.owner === spec.owner;
                    return (
                      <div className="schema-node" key={group.owner}>
                        <button
                          className="schema-row"
                          type="button"
                          aria-expanded={expanded}
                          onClick={() => toggleSourceOwner(group.owner)}
                        >
                          {expanded ? (
                            <ChevronDown size={14} aria-hidden="true" />
                          ) : (
                            <ChevronRight size={14} aria-hidden="true" />
                          )}
                          <span className="schema-name">{group.owner}</span>
                          <span className="schema-count">{group.tables.length}</span>
                        </button>
                        {expanded && (
                          <div className="table-node-list">
                            {group.tables.map((table) => {
                              const key = tableKeyFor(table);
                              return (
                                <button
                                  className={`table-node ${
                                    key === tableKey ? "is-selected" : ""
                                  }`}
                                  key={key}
                                  type="button"
                                  onClick={() => selectTable(key)}
                                >
                                  <span className="mono">{table.name}</span>
                                </button>
                              );
                            })}
                          </div>
                        )}
                      </div>
                    );
                  })}
                  {sourceTableGroups.length === 0 && (
                    <p className="tree-empty">没有匹配的库表。</p>
                  )}
                </div>
              )}
              <div className="builder-selected-row">
                <span>
                  当前源表：
                  <strong className="mono">{sourceTableLabel || "未选择"}</strong>
                </span>
                <button
                  className="button is-ghost"
                  type="button"
                  onClick={() => void loadColumns()}
                  disabled={
                    spec.owner === "" || loading !== null || sourceDatasourceId === ""
                  }
                >
                  {loading === "columns" ? (
                    <LoaderCircle className="is-spinning" size={15} />
                  ) : (
                    <TableProperties size={15} />
                  )}
                  {loading === "columns" ? "读取中" : "读取列"}
                </button>
              </div>
            </div>
            )}
            {builderError !== null && (
              <div className="form-error" role="alert">
                {builderError}
              </div>
            )}
            {columnNames.length > 0 && (
              <div className="builder-columns table-wrap">
                <table className="data-grid">
                  <thead>
                    <tr>
                      <th>
                        <label className="select-all-cell">
                          <input
                            type="checkbox"
                            checked={allSourceColumnsSelected}
                            onChange={toggleAllColumns}
                            aria-label="全选源表字段"
                          />
                          全选
                        </label>
                      </th>
                      <th>列名</th>
                      <th>字典类型</th>
                      {/* ADR-0039 §7：单位是字符。这一栏同时承载 NUMBER 的 (p,s)，
                          所以留着「精度 /」那半句——把它整个换成「长度（字符）」会让
                          DECIMAL(10,2) 那种值读起来像一个长度。表下那句说明补齐口径。 */}
                      <th>精度 / 长度（字符）</th>
                      <th>可空</th>
                    </tr>
                  </thead>
                  <tbody>
                    {columnNames.map((columnName) => {
                      const column = dictionary.get(columnName);
                      const target = targetFieldOf(spec.columns, columnName);
                      const selected = target !== undefined;
                      return (
                        <tr key={columnName}>
                          <td>
                            <input
                              type="checkbox"
                              checked={selected}
                              disabled={sourceQueryMode === "sql"}
                              onChange={() => toggleColumn(columnName)}
                              aria-label={`选择 ${columnName}`}
                            />
                          </td>
                          <td className="mono">{columnName}</td>
                          <td className="mono">{column?.data_type ?? "—"}</td>
                          <td className="mono">
                            {column === undefined ? "—" : columnShape(column)}
                          </td>
                          <td>
                            {sourceQueryMode === "sql"
                              ? "—"
                              : column === undefined
                                ? "—"
                                : column.nullable
                                  ? "是"
                                  : "否"}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            )}
            {/* ADR-0039 §7 的静态说明。**零判定、零元数据**——只说清口径，不去换算，
                也不因为某一列可能撞上就着色。 */}
            {columnNames.length > 0 && (
              <p className="target-side-note">
                长度栏的单位是<strong>字符</strong>。MySQL 使用 <code>utf8mb4</code> 时，
                1 个汉字通常按 3 字节计算。
              </p>
            )}
            <footer>
              <span>
                {spec.columns.length} 列已选 · 目标字段与主键在读取目标列后设置
              </span>
              <span className="builder-key-note">
                主键用于去重，必须至少选一列。
              </span>
            </footer>
          </section>

          <section className="builder-guide" aria-labelledby="builder-target-title">
            <header>
              <div>
                <strong id="builder-target-title">目标表</strong>
                <span>目标表需已在目标库中存在，可搜索后选择</span>
              </div>
            </header>
            <div className="target-tree-shell">
              <div className="target-table-controls">
                <label className="tree-search">
                  <Search size={15} aria-hidden="true" />
                  <input
                    required
                    value={targetTableFilter}
                    placeholder={
                      targetDatasourceId === ""
                        ? "先选目标端数据源"
                        : "搜索或输入目标表"
                    }
                    onChange={(event) => {
                      setTargetTableFilter(event.target.value);
                      updateTargetTable(event.target.value);
                    }}
                    onBlur={() => loadTargetColumns()}
                  />
                </label>
                <button
                  className="button is-ghost"
                  type="button"
                  onClick={() => void loadTargetTables()}
                  disabled={targetDatasourceId === "" || targetTablesLoading}
                >
                  {targetTablesLoading ? (
                    <LoaderCircle className="is-spinning" size={15} />
                  ) : (
                    <RefreshCw size={15} />
                  )}
                  {targetTablesLoading ? "读取中" : "读取表"}
                </button>
                <button
                  className="button is-ghost"
                  type="button"
                  onClick={() => loadTargetColumns(true)}
                  disabled={
                    targetDatasourceId === "" ||
                    spec.target_table.trim() === "" ||
                    targetMeta.kind === "loading"
                  }
                >
                  {targetMeta.kind === "loading" ? (
                    <LoaderCircle className="is-spinning" size={15} />
                  ) : (
                    <TableProperties size={15} />
                  )}
                  {targetMeta.kind === "loading" ? "读取中" : "读取列"}
                </button>
              </div>
              <div className="schema-tree is-target" role="tree" aria-label="目标表">
                <button
                  className="schema-row"
                  type="button"
                  aria-expanded={targetTreeOpen}
                  onClick={() => setTargetTreeOpen((open) => !open)}
                >
                  {targetTreeOpen ? (
                    <ChevronDown size={14} aria-hidden="true" />
                  ) : (
                    <ChevronRight size={14} aria-hidden="true" />
                  )}
                  <span className="schema-name">
                    {targetTreeLabel(selectedTargetDatasource)}
                  </span>
                  <span className="schema-count">{filteredTargetTables.length}</span>
                </button>
                {targetTreeOpen && (
                  <div className="table-node-list">
                    {filteredTargetTables.map((table) => (
                      <button
                        className={`table-node ${
                          table === spec.target_table ? "is-selected" : ""
                        }`}
                        key={table}
                        type="button"
                        onClick={() => selectTargetTable(table)}
                      >
                        <span className="mono">{table}</span>
                      </button>
                    ))}
                    {targetTables.length === 0 && (
                      <p className="tree-empty">先读取目标表。</p>
                    )}
                    {targetTables.length > 0 && filteredTargetTables.length === 0 && (
                      <p className="tree-empty">没有匹配的目标表。</p>
                    )}
                  </div>
                )}
              </div>
              <div className="builder-selected-row">
                <span>
                  当前目标表：
                  <strong className="mono">{spec.target_table || "未选择"}</strong>
                </span>
              </div>
            </div>
            {targetTablesError !== null && (
              <div className="form-error" role="alert">
                {targetTablesError}
              </div>
            )}
            <TargetColumnReference
              state={targetMeta}
              spec={spec}
              selectedSources={selectedSourceColumns}
              onReload={() => loadTargetColumns(true)}
              onTargetChange={renameTarget}
              onToggleKey={toggleKey}
              onFillSameName={fillSameNameTargets}
            />
          </section>

          {sourceQueryMode === "table" && (
            <ConditionEditor
              conditions={spec.conditions}
              columnNames={columnNames}
              dictionary={dictionary}
              onAdd={addCondition}
              onChange={updateCondition}
              onRemove={removeCondition}
            />
          )}

          <GeneratedSql sql={sql} error={sqlError} ready={specComplete} />

          {error !== null && (
            <div className="form-error" role="alert">
              {error}
            </div>
          )}
        </div>
        <ModalFooter
          onClose={onClose}
          busy={submitting}
          submitLabel={submitLabel}
          submitDisabled={!specComplete}
        />
      </form>
    </Modal>
  );
}

/**
 * 过滤条件控件：字段 + 比较符 + 值，可有若干条（ADR-0035 §3）。
 *
 * 一条都没有时整表取数——**这是允许的**，量级风险归台架（#122）去证，不在这里挡。
 * 比较符严格只有 `>` `<` `=`：`>=` / `<=` 与 `IN` / `BETWEEN` / `LIKE` 第一版都不做。
 */
function ConditionEditor({
  conditions,
  columnNames,
  dictionary,
  onAdd,
  onChange,
  onRemove,
}: {
  conditions: Condition[];
  columnNames: string[];
  dictionary: ReadonlyMap<string, BuilderColumn>;
  onAdd: () => void;
  onChange: (index: number, change: Partial<Condition>) => void;
  onRemove: (index: number) => void;
}) {
  return (
    <section className="spec-editor" aria-labelledby="conditions-title">
      <header>
        <div>
          <strong id="conditions-title">过滤条件</strong>
          <span>一条都没有就是整表取数</span>
        </div>
        <button
          className="button is-ghost"
          type="button"
          onClick={onAdd}
          disabled={columnNames.length === 0}
        >
          <Plus size={15} aria-hidden="true" />
          添加条件
        </button>
      </header>
      {conditions.length === 0 ? (
        <p className="spec-empty">没有条件：本任务每次取整张表。</p>
      ) : (
        <ul className="condition-list">
          {conditions.map((condition, index) => (
            <li key={index} className="condition-row">
              <label>
                <span>字段</span>
                <select
                  value={condition.column}
                  onChange={(event) => {
                    const column = event.target.value;
                    onChange(index, {
                      column,
                      value_type: defaultValueType(dictionary.get(column)?.data_type),
                    });
                  }}
                >
                  {columnOptions(columnNames, condition.column)}
                </select>
              </label>
              <label>
                <span>比较</span>
                <select
                  value={condition.operator}
                  onChange={(event) =>
                    onChange(index, {
                      operator: event.target.value as Condition["operator"],
                    })
                  }
                >
                  {COMPARISONS.map((operator) => (
                    <option key={operator} value={operator}>
                      {comparisonSymbol(operator)}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>值类型</span>
                <select
                  value={condition.value_type}
                  onChange={(event) =>
                    onChange(index, {
                      value_type: event.target.value as Condition["value_type"],
                    })
                  }
                >
                  {Object.entries(VALUE_TYPE_LABELS).map(([value, label]) => (
                    <option key={value} value={value}>
                      {label}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>值</span>
                <input
                  value={condition.constant}
                  required
                  onChange={(event) => onChange(index, { constant: event.target.value })}
                />
              </label>
              <button
                className="icon-button is-danger"
                type="button"
                title="删除这条条件"
                aria-label={`删除条件 ${condition.parameter}`}
                onClick={() => onRemove(index)}
              >
                <Trash2 size={15} aria-hidden="true" />
              </button>
            </li>
          ))}
        </ul>
      )}
      <small className="spec-note">
        过滤值为固定配置，保存后随任务执行。
      </small>
    </section>
  );
}

/**
 * 构建 SQL 的只读展示。
 *
 * v1 **没有编辑入口**（ADR-0036 §1）：SQL 是规格的派生面，现算现看，不存进任务定义，
 * 也没有把它传回后端的路。这里连 `textarea` 都不给——给了就是在暗示可以改。
 */
function GeneratedSql({
  sql,
  error,
  ready,
}: {
  sql: BuilderSql | null;
  error: string | null;
  ready: boolean;
}) {
  return (
    <section className="generated-sql" aria-labelledby="generated-sql-title">
      <header>
        <div>
          <strong id="generated-sql-title">构建 SQL</strong>
          <span>只读预览</span>
        </div>
      </header>
      {error !== null ? (
        <div className="form-error" role="alert">
          {error}
        </div>
      ) : sql === null ? (
        <p className="spec-empty">
          {ready
            ? "正在生成..."
            : "先选源表和源列，再读取目标列完成映射与主键。"}
        </p>
      ) : (
        <>
          <pre className="ddl-output">{sql.source_sql}</pre>
          <div className="run-parameter-list">
            <strong>运行参数</strong>
            {sql.run_parameters.length === 0 ? (
              <span>无——发起运行时不需要填任何值。</span>
            ) : (
              <ul>
                {sql.run_parameters.map((parameter) => (
                  <li key={parameter.parameter}>
                    <span className="mono">{parameter.parameter}</span>
                    <span>
                      {parameter.column} · {VALUE_TYPE_LABELS[parameter.value_type]}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </>
      )}
    </section>
  );
}

function columnOptions(columnNames: string[], current: string) {
  const names = columnNames.includes(current) ? columnNames : [current, ...columnNames];
  return names.map((name) => (
    <option key={name} value={name}>
      {name}
    </option>
  ));
}

/**
 * 目标表列参考（ADR-0038 §3、ADR-0039 §5）。
 *
 * **只亮不判**：`PRIMARY` / `UNIQUE u_code` 摆在「约束」栏供参考，**主键仍由用户勾**——
 * 按哪一条唯一约束去重是业务判断，不是元数据事实。「未映射 + 非空 + 无默认值」的列
 * （预检会拒的那一类）在这里**不拦**，只是整行压暗。**构建器放行、预检拒，是刻意的**：
 * `information_schema` 与运行那一刻的目标表可以不一致。
 */
function TargetColumnReference({
  state,
  spec,
  selectedSources,
  onReload,
  onTargetChange,
  onToggleKey,
  onFillSameName,
}: {
  state: TargetMetaState;
  spec: TaskSpec;
  selectedSources: string[];
  onReload: () => void;
  onTargetChange: (source: string, target: string) => void;
  onToggleKey: (source: string) => void;
  onFillSameName: () => void;
}) {
  if (state.kind === "idle") {
    return null;
  }
  return (
    <section className="column-fetch-section" aria-labelledby="target-columns-title">
      <header>
        <div>
          <strong id="target-columns-title">目标表列参考</strong>
          <span>目标表结构供参考，主键由你选择</span>
        </div>
        <button
          className="button is-ghost"
          type="button"
          onClick={onReload}
          disabled={state.kind === "loading"}
        >
          {state.kind === "loading" ? "读取中" : "重新读取"}
        </button>
      </header>
      {state.kind === "loading" && (
        <div className="loading-state" aria-live="polite">
          正在读取目标表列...
        </div>
      )}
      {state.kind === "failed" && (
        <div className="form-error" role="alert">
          {state.message}
        </div>
      )}
      {state.kind === "ready" && state.data.columns.length === 0 && (
        <p className="column-fetch-hint">
          目标库中没有 <code>{state.table}</code> 这张表。请在目标库中建好这张表，
          然后点「重新读取」。
        </p>
      )}
      {state.kind === "ready" && state.data.columns.length > 0 && (
        <>
          <FieldMappingEditor
            spec={spec}
            selectedSources={selectedSources}
            targetMeta={state.data}
            onTargetChange={onTargetChange}
            onToggleKey={onToggleKey}
            onFillSameName={onFillSameName}
          />
          <div className="table-wrap">
            <table className="data-grid">
              <thead>
                <tr>
                  <th>目标表列</th>
                  <th>类型</th>
                  <th>长度（字符）</th>
                  <th>可空</th>
                  <th>默认值</th>
                  <th>约束</th>
                  <th>映射自</th>
                </tr>
              </thead>
              <tbody>
                {state.data.columns.map((column) => {
                  const source = mappedSourceOf(spec, column.name);
                  return (
                    <tr
                      key={column.name}
                      className={source === undefined ? "is-unmapped" : ""}
                    >
                      <td className="mono">{column.name}</td>
                      <td className="mono">{column.column_type}</td>
                      <td className="mono">{column.length ?? "—"}</td>
                      <td>{column.nullable ? "是" : "否"}</td>
                      <td className="mono">{column.default_value ?? "—"}</td>
                      <td className="mono">
                        {constraintsOf(state.data, column) || "—"}
                      </td>
                      <td className="mono">{source ?? "（未映射）"}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </>
      )}
      <footer>
        <span>
          目标表结构仅用于本次配置参考，不会写入任务定义。
        </span>
        <span className="builder-key-note">
          长度栏同为字符；映射预检按字节判。
        </span>
      </footer>
    </section>
  );
}

function FieldMappingEditor({
  spec,
  selectedSources,
  targetMeta,
  onTargetChange,
  onToggleKey,
  onFillSameName,
}: {
  spec: TaskSpec;
  selectedSources: string[];
  targetMeta: TargetTableMetadata;
  onTargetChange: (source: string, target: string) => void;
  onToggleKey: (source: string) => void;
  onFillSameName: () => void;
}) {
  const targetColumns = targetMeta.columns;
  const targetByUpper = new Map(
    targetColumns.map((column) => [column.name.toUpperCase(), column]),
  );

  return (
    <section className="field-mapping-section" aria-labelledby="field-mapping-title">
      <header>
        <div>
          <strong id="field-mapping-title">字段映射</strong>
          <span>读取目标列后再把源列绑定到目标列</span>
        </div>
        <button
          className="button is-ghost"
          type="button"
          onClick={onFillSameName}
          disabled={selectedSources.length === 0 || targetColumns.length === 0}
        >
          同名填充
        </button>
      </header>
      {selectedSources.length === 0 ? (
        <p className="column-fetch-hint">
          先在源表里勾选要搬的列，再到这里选择目标字段。
        </p>
      ) : (
        <div className="table-wrap">
          <table className="data-grid">
            <thead>
              <tr>
                <th>源列</th>
                <th>目标字段</th>
                <th>目标类型</th>
                <th>约束</th>
                <th>主键</th>
              </tr>
            </thead>
            <tbody>
              {selectedSources.map((source) => {
                const target = targetFieldOf(spec.columns, source) ?? "";
                const targetColumn = targetByUpper.get(target.toUpperCase());
                const targetExists =
                  target === "" || targetColumn !== undefined;
                return (
                  <tr key={source} className={!targetExists ? "is-unmapped" : ""}>
                    <td className="mono">{source}</td>
                    <td>
                      <select
                        className="cell-input mono"
                        value={target}
                        aria-label={`${source} 的目标字段`}
                        onChange={(event) =>
                          onTargetChange(source, event.target.value)
                        }
                      >
                        <option value="">请选择目标字段</option>
                        {!targetExists && (
                          <option value={target}>
                            当前：{target}（目标表未返回）
                          </option>
                        )}
                        {targetColumns.map((column) => (
                          <option key={column.name} value={column.name}>
                            {column.name}
                          </option>
                        ))}
                      </select>
                    </td>
                    <td className="mono">{targetColumn?.column_type ?? "—"}</td>
                    <td className="mono">
                      {targetColumn === undefined
                        ? "—"
                        : constraintsOf(targetMeta, targetColumn) || "—"}
                    </td>
                    <td>
                      <input
                        type="checkbox"
                        checked={target !== "" && spec.primary_key.includes(target)}
                        disabled={target === ""}
                        onChange={() => onToggleKey(source)}
                        aria-label={`把 ${source} 映射的目标字段设为主键列`}
                      />
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      <footer>
        <span>
          {spec.columns.length} 列已选 ·{" "}
          {spec.columns.filter((mapping) => mapping.target.trim() !== "").length}
          {" "}列已映射 · {spec.primary_key.length} 列作主键
        </span>
      </footer>
    </section>
  );
}

/** 目标列被哪一个源列映射过来——没有就是未映射。列名大小写无关（ADR-0038 §4 同一口径）。 */
function mappedSourceOf(spec: TaskSpec, targetColumn: string): string | undefined {
  const wanted = targetColumn.toUpperCase();
  return spec.columns.find((mapping) => mapping.target.toUpperCase() === wanted)
    ?.source;
}

interface SourceTableGroup {
  owner: string;
  tables: BuilderTable[];
}

function groupSourceTables(
  tables: readonly BuilderTable[],
  query: string,
): SourceTableGroup[] {
  const wanted = normalizeFilter(query);
  const grouped = new Map<string, BuilderTable[]>();
  for (const table of tables) {
    const ownerMatch = normalizeFilter(table.owner).includes(wanted);
    const tableMatch = normalizeFilter(table.name).includes(wanted);
    const fullMatch = normalizeFilter(`${table.owner}.${table.name}`).includes(wanted);
    if (wanted !== "" && !ownerMatch && !tableMatch && !fullMatch) {
      continue;
    }
    const bucket = grouped.get(table.owner) ?? [];
    bucket.push(table);
    grouped.set(table.owner, bucket);
  }
  return [...grouped.entries()].map(([owner, ownerTables]) => ({
    owner,
    tables: ownerTables,
  }));
}

function filterNames(names: readonly string[], query: string): string[] {
  const wanted = normalizeFilter(query);
  if (wanted === "") {
    return [...names];
  }
  return names.filter((name) => normalizeFilter(name).includes(wanted));
}

function normalizeFilter(value: string): string {
  return value.trim().toLocaleUpperCase();
}

function targetTreeLabel(datasource: Datasource | undefined): string {
  if (datasource === undefined) {
    return "目标库";
  }
  if (datasource.kind === "mysql") {
    return datasource.database;
  }
  return datasource.name;
}

function isLiveRunHistory(run: RunHistory): boolean {
  return (
    run.finished_at === null &&
    run.outcome === null &&
    run.unknown_reason === null
  );
}

/** 覆盖这一列的唯一性约束，`PRIMARY` 原样、其余写成 `UNIQUE <名字>`。 */
function constraintsOf(data: TargetTableMetadata, column: TargetColumn): string {
  const wanted = column.name.toUpperCase();
  return data.keys
    .filter((key) => key.columns.some((name) => name.toUpperCase() === wanted))
    .map((key) => (key.name === "PRIMARY" ? "PRIMARY" : `UNIQUE ${key.name}`))
    .join(" · ");
}

/**
 * 任务列表里那一列条件的单行读法：`列 符 固定值`；历史任务里的运行参数条件仍按参数名展示。
 *
 * 一条条件都没有时明写「整表」——留空会被读成「没配好」，而整表取数是允许的形态。
 */
/**
 * 一个数据源都没有时给一条路，而不是一个空下拉（ADR-0039 §8）。
 *
 * **不做「就地弹出新建数据源」**：对话框套对话框会让同一套表单有两个入口，
 * 两处的「测通才让存」行为一旦分岔就是最难查的一类不一致。代价是建任务被打断一次，
 * 但按所有者裁定 1（现场 3~5 个数据源）这件事一共只发生几次。
 */
function DatasourceHint() {
  return (
    <a className="text-button" href="#datasources">
      去「数据源」建一个 →
    </a>
  );
}

function tableKeyFor(table: BuilderTable): string {
  return `${table.owner}\u0000${table.name}`;
}

function columnShape(column: Pick<BuilderColumn, "precision" | "scale" | "length">): string {
  if (column.precision !== null) {
    if (column.scale === null) {
      return `(${column.precision})`;
    }
    return `(${column.precision},${column.scale})`;
  }
  if (column.length === null) {
    return "-";
  }
  return `(${column.length})`;
}

function RenameDialog({
  task,
  onClose,
  onSubmit,
}: {
  task: Task;
  onClose: () => void;
  onSubmit: (input: TaskInput) => Promise<void>;
}) {
  const [name, setName] = useState(task.name);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      await onSubmit(taskInputFrom(task, { name }));
      onClose();
    } catch (submitError) {
      setError(messageFrom(submitError));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Modal title="任务改名" onClose={onClose} busy={submitting} narrow>
      <form onSubmit={(event) => void handleSubmit(event)}>
        <div className="modal-body form-stack">
          <FormField label="任务名称">
            <input
              autoFocus
              required
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </FormField>
          {error !== null && (
            <div className="form-error" role="alert">
              {error}
            </div>
          )}
        </div>
        <ModalFooter
          onClose={onClose}
          busy={submitting}
          submitLabel="保存名称"
        />
      </form>
    </Modal>
  );
}

function DeleteDialog({
  task,
  onClose,
  onDelete,
}: {
  task: Task;
  onClose: () => void;
  onDelete: () => Promise<void>;
}) {
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  async function handleDelete() {
    setSubmitting(true);
    setError(null);
    try {
      await onDelete();
      onClose();
    } catch (deleteError) {
      setError(messageFrom(deleteError));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Modal title="删除任务" onClose={onClose} busy={submitting} narrow>
      <div className="modal-body delete-copy">
        <p>
          确认删除任务“<strong>{task.name}</strong>”？
        </p>
        <span className="task-id">{task.task_id}</span>
        {error !== null && (
          <div className="form-error" role="alert">
            {error}
          </div>
        )}
      </div>
      <footer className="modal-footer">
        <button
          className="button is-ghost"
          type="button"
          onClick={onClose}
          disabled={submitting}
        >
          取消
        </button>
        <button
          className="button is-danger"
          type="button"
          onClick={() => void handleDelete()}
          disabled={submitting}
        >
          {submitting ? "正在删除" : "删除"}
        </button>
      </footer>
    </Modal>
  );
}
