import {
  Bell,
  CalendarClock,
  Clock3,
  Copy,
  Database,
  LoaderCircle,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  TableProperties,
  Tag,
  Trash2,
  X,
} from "lucide-react";
import { Fragment, useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent, ReactNode } from "react";

import {
  createTask,
  deleteTask,
  emptySpec,
  fetchBuilderColumns,
  fetchBuilderTables,
  fetchColumns,
  generateBuilderSql,
  listTasks,
  taskInputFrom,
  updateTask,
} from "./api";
import type {
  BuilderColumn,
  BuilderSql,
  BuilderTable,
  ColumnFetchResult,
  Condition,
  FetchedColumn,
  OrderTerm,
  Task,
  TaskInput,
  TaskSpec,
  TargetDdlIssue,
} from "./api";
import { messageFrom } from "./errors";
import { HistoryScreen } from "./HistoryScreen";
import {
  ddlTextSegments,
  fetchedColumnType,
  targetDdlFailureFrom,
} from "./m3";
import { RunScreen } from "./RunScreen";
import {
  COMPARISONS,
  comparisonSymbol,
  defaultParameterName,
  defaultValueType,
  VALUE_TYPE_LABELS,
} from "./spec";
import { StartRunDialog } from "./StartRunDialog";

type DialogState =
  | { kind: "create" }
  | { kind: "edit"; task: Task }
  | { kind: "rename"; task: Task }
  | { kind: "delete"; task: Task }
  | { kind: "start"; task: Task }
  | null;

type Page = "tasks" | "history";

function pageFromHash(hash: string): Page {
  return hash === "#history" ? "history" : "tasks";
}

const emptyTask: TaskInput = { name: "", spec: emptySpec() };

export function App() {
  const [page, setPage] = useState<Page>(() =>
    pageFromHash(window.location.hash),
  );
  const [tasks, setTasks] = useState<Task[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(true);
  const [query, setQuery] = useState("");
  const [dialog, setDialog] = useState<DialogState>(null);
  const [activeRun, setActiveRun] = useState<{
    task: Task;
    runRecordId: string;
  } | null>(null);

  const loadTasks = useCallback(async () => {
    setRefreshing(true);
    try {
      setTasks(await listTasks());
      setLoadError(null);
    } catch (error) {
      setLoadError(messageFrom(error));
    } finally {
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void loadTasks();
  }, [loadTasks]);

  useEffect(() => {
    function handleHashChange() {
      setPage(pageFromHash(window.location.hash));
    }
    window.addEventListener("hashchange", handleHashChange);
    return () => window.removeEventListener("hashchange", handleHashChange);
  }, []);

  const filteredTasks = useMemo(() => {
    if (tasks === null) {
      return [];
    }

    const normalizedQuery = query.trim().toLocaleLowerCase("zh-CN");
    if (normalizedQuery === "") {
      return tasks;
    }

    return tasks.filter((task) =>
      [
        task.name,
        task.task_id,
        `${task.spec.owner}.${task.spec.table}`,
        task.spec.target_table,
      ]
        .join(" ")
        .toLocaleLowerCase("zh-CN")
        .includes(normalizedQuery),
    );
  }, [query, tasks]);

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

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="product-brand">
          <span className="brand-mark">Q</span>
          db-qbs
        </div>
        <nav aria-label="主导航">
          <a
            className={`nav-item ${navPage === "tasks" ? "is-active" : ""}`}
            href="#tasks"
            aria-current={navPage === "tasks" ? "page" : undefined}
            onClick={(event) => {
              event.preventDefault();
              navigate("tasks");
            }}
          >
            <Database size={15} aria-hidden="true" />
            任务
          </a>
          <a
            className={`nav-item ${navPage === "history" ? "is-active" : ""}`}
            href="#history"
            aria-current={navPage === "history" ? "page" : undefined}
            onClick={(event) => {
              event.preventDefault();
              navigate("history");
            }}
          >
            <Clock3 size={15} aria-hidden="true" />
            运行历史
          </a>
          <p className="nav-section">非 V1 范围</p>
          <span className="nav-item is-disabled">
            <CalendarClock size={15} aria-hidden="true" />
            定时调度
            <span className="nav-badge">M3+</span>
          </span>
          <span className="nav-item is-disabled">
            <Bell size={15} aria-hidden="true" />
            告警
            <span className="nav-badge">M3+</span>
          </span>
        </nav>
      </aside>

      <main className="main-column">
        <header className="topbar">
          <span className="mobile-brand">db-qbs</span>
          <nav className="mobile-nav" aria-label="主导航">
            <button
              className={navPage === "tasks" ? "is-active" : ""}
              type="button"
              aria-current={navPage === "tasks" ? "page" : undefined}
              onClick={() => navigate("tasks")}
            >
              <Database size={14} aria-hidden="true" />任务
            </button>
            <button
              className={navPage === "history" ? "is-active" : ""}
              type="button"
              aria-current={navPage === "history" ? "page" : undefined}
              onClick={() => navigate("history")}
            >
              <Clock3 size={14} aria-hidden="true" />历史
            </button>
          </nav>
          <span className="breadcrumb">
            数据导入 <span aria-hidden="true">/</span>{" "}
            <strong>{activeRun !== null ? "运行详情" : pageLabel(page)}</strong>
          </span>
          <span className="environment">source · 当前实例</span>
        </header>

        <div className="content">
          {loadError !== null && page === "tasks" && (
            <div className="notice is-error" role="alert">
              <span>{loadError}</span>
              <button
                className="text-button"
                type="button"
                onClick={() => void loadTasks()}
              >
                重新加载
              </button>
            </div>
          )}

          {activeRun !== null && (
            <RunScreen
              task={activeRun.task}
              runRecordId={activeRun.runRecordId}
              onBack={() => setActiveRun(null)}
              onRelaunch={() => setDialog({ kind: "start", task: activeRun.task })}
              onOpenColumnFetch={() =>
                setDialog({ kind: "edit", task: activeRun.task })
              }
            />
          )}

          {activeRun === null && page === "tasks" && (
            <section className="card" id="tasks" aria-labelledby="tasks-title">
              <header className="card-header">
                <div>
                  <h1 id="tasks-title">任务</h1>
                  <span className="card-subtitle">
                    {taskSummaryLabel(tasks, refreshing)}
                  </span>
                </div>
                <button
                  className="button is-primary"
                  type="button"
                  onClick={openCreateDialog}
                >
                  <Plus size={15} aria-hidden="true" />
                  新建任务
                </button>
              </header>

              {tasks !== null && tasks.length > 0 && (
                <div className="toolbar">
                  <label className="search-field">
                    <span>搜索</span>
                    <input
                      value={query}
                      onChange={(event) => setQuery(event.target.value)}
                      placeholder="任务名 / 源表 / 目标表"
                    />
                  </label>
                  <button
                    className="icon-button"
                    type="button"
                    title="刷新任务"
                    aria-label="刷新任务"
                    onClick={() => void loadTasks()}
                    disabled={refreshing}
                  >
                    <RefreshCw
                      className={refreshing ? "is-spinning" : ""}
                      size={16}
                      aria-hidden="true"
                    />
                  </button>
                </div>
              )}

              <TaskResults
                tasks={tasks}
                filteredTasks={filteredTasks}
                refreshing={refreshing}
                onCreate={openCreateDialog}
                onAction={setDialog}
              />
            </section>
          )}

          {activeRun === null && page === "history" && (
            <HistoryScreen tasks={tasks ?? []} />
          )}
        </div>

        {page === "tasks" && dialog?.kind === "create" && (
          <TaskFormDialog
            title="新建任务"
            initial={emptyTask}
            submitLabel="新建"
            onClose={closeDialog}
            onSubmit={handleCreate}
          />
        )}
        {page === "tasks" && dialog?.kind === "edit" && (
          <TaskFormDialog
            title={`编辑 · ${dialog.task.name}`}
            initial={taskInputFrom(dialog.task)}
            submitLabel="保存"
            hideName
            onClose={closeDialog}
            onSubmit={(input) => handleUpdate(dialog.task, input)}
          />
        )}
        {page === "tasks" && dialog?.kind === "rename" && (
          <RenameDialog
            task={dialog.task}
            onClose={closeDialog}
            onSubmit={(input) => handleUpdate(dialog.task, input)}
          />
        )}
        {page === "tasks" && dialog?.kind === "delete" && (
          <DeleteDialog
            task={dialog.task}
            onClose={closeDialog}
            onDelete={() => handleDelete(dialog.task)}
          />
        )}
        {dialog?.kind === "start" && (
          <StartRunDialog
            task={dialog.task}
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
    case "tasks":
      return "任务";
    case "history":
      return "运行历史";
  }
}

function taskSummaryLabel(tasks: Task[] | null, refreshing: boolean): string {
  if (tasks !== null) {
    return `共 ${tasks.length} 个`;
  }
  if (refreshing) {
    return "正在读取";
  }
  return "暂不可用";
}

function TaskResults({
  tasks,
  filteredTasks,
  refreshing,
  onCreate,
  onAction,
}: {
  tasks: Task[] | null;
  filteredTasks: Task[];
  refreshing: boolean;
  onCreate: () => void;
  onAction: (dialog: DialogState) => void;
}) {
  if (tasks === null) {
    return (
      <div className="loading-state" aria-live="polite">
        {refreshing ? "正在加载任务..." : "任务暂不可用"}
      </div>
    );
  }
  if (tasks.length === 0) {
    return <EmptyState onCreate={onCreate} />;
  }
  if (filteredTasks.length === 0) {
    return <div className="no-results">没有匹配的任务</div>;
  }
  return <TaskTable tasks={filteredTasks} onAction={onAction} />;
}

function EmptyState({ onCreate }: { onCreate: () => void }) {
  return (
    <div className="empty-state">
      <div className="empty-icon">
        <Database size={22} aria-hidden="true" />
      </div>
      <h2>还没有任务</h2>
      <p>新建第一个 Oracle → MySQL 导入任务。</p>
      <button className="button is-primary" type="button" onClick={onCreate}>
        <Plus size={15} aria-hidden="true" />
        新建任务
      </button>
    </div>
  );
}

function TaskTable({
  tasks,
  onAction,
}: {
  tasks: Task[];
  onAction: (dialog: DialogState) => void;
}) {
  return (
    <div className="table-wrap">
      <table className="data-grid">
        <thead>
          <tr>
            <th>任务</th>
            <th>源表</th>
            <th>目标表</th>
            <th>主键</th>
            <th>条件</th>
            <th className="action-column">操作</th>
          </tr>
        </thead>
        <tbody>
          {tasks.map((task) => (
            <tr key={task.task_id}>
              <td>
                <span className="task-name">{task.name}</span>
                <span className="task-id">{task.task_id}</span>
              </td>
              <td className="mono">
                {task.spec.owner}.{task.spec.table}
              </td>
              <td className="mono">{task.spec.target_table}</td>
              <td className="mono">{task.spec.primary_key.join(", ")}</td>
              <td className="sql-cell" title={conditionSummary(task.spec)}>
                {conditionSummary(task.spec)}
              </td>
              <td>
                <div className="row-actions">
                  <ActionButton
                    label="发起运行"
                    icon={<Play size={15} />}
                    onClick={() => onAction({ kind: "start", task })}
                  />
                  <ActionButton
                    label="编辑任务定义"
                    icon={<Pencil size={15} />}
                    onClick={() => onAction({ kind: "edit", task })}
                  />
                  <ActionButton
                    label="改名"
                    icon={<Tag size={15} />}
                    onClick={() => onAction({ kind: "rename", task })}
                  />
                  <ActionButton
                    label="删除"
                    danger
                    icon={<Trash2 size={15} />}
                    onClick={() => onAction({ kind: "delete", task })}
                  />
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ActionButton({
  label,
  icon,
  danger = false,
  onClick,
}: {
  label: string;
  icon: ReactNode;
  danger?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      className={`icon-button ${danger ? "is-danger" : ""}`}
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
    >
      {icon}
    </button>
  );
}

type ColumnFetchState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "oracle-failed"; message: string }
  | { kind: "target-ddl-failed"; columns: FetchedColumn[]; issues: TargetDdlIssue[] }
  | { kind: "ready"; result: ColumnFetchResult };

/**
 * 任务定义编辑器 —— 构建器本身就是任务定义（ADR-0036 §1）。
 *
 * 它不再是「一次性生成四个字段、选择态丢弃」的向导：结构化规格是唯一真相源，
 * 选表、勾列、勾主键、条件、排序**全部原样存进任务定义**，再次打开还在（所有者裁定 6）。
 * SQL 是规格的派生面，只读展示，v1 没有编辑入口。
 */
function TaskFormDialog({
  title,
  initial,
  submitLabel,
  hideName = false,
  onClose,
  onSubmit,
}: {
  title: string;
  initial: TaskInput;
  submitLabel: string;
  hideName?: boolean;
  onClose: () => void;
  onSubmit: (input: TaskInput) => Promise<void>;
}) {
  const [name, setName] = useState(initial.name);
  const [spec, setSpec] = useState<TaskSpec>(() => ({ ...initial.spec }));
  const [tables, setTables] = useState<BuilderTable[]>([]);
  const [columns, setColumns] = useState<BuilderColumn[]>([]);
  const [loading, setLoading] = useState<"tables" | "columns" | null>(null);
  const [builderError, setBuilderError] = useState<string | null>(null);
  const [sql, setSql] = useState<BuilderSql | null>(null);
  const [sqlError, setSqlError] = useState<string | null>(null);
  const [columnFetch, setColumnFetch] = useState<ColumnFetchState>({ kind: "idle" });
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const dblink = spec.dblink ?? "";
  const tableKey = spec.owner === "" ? "" : tableKeyFor({ owner: spec.owner, name: spec.table });
  const dictionary = useMemo(
    () => new Map(columns.map((column) => [column.name, column])),
    [columns],
  );
  // 编辑既有任务时字典还没读，但选中的列名规格里就有——照样列出来，
  // 只是字典那几列显示为未知。不读一次 Oracle 就改不了条件，是没必要的门槛。
  const columnNames = useMemo(() => {
    const names = columns.map((column) => column.name);
    for (const column of spec.columns) {
      if (!names.includes(column)) {
        names.push(column);
      }
    }
    return names;
  }, [columns, spec.columns]);

  // SQL 现算：规格一变就换一份（ADR-0036 §2 不存 SQL）。规格还没成形时不去打扰后端——
  // 它会拿 `validate()` 的报错回话，那不是「SQL 生成失败」，只是还没填完。
  const specComplete = spec.owner !== "" && spec.columns.length > 0;
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
    setColumnFetch({ kind: "idle" });
  }

  async function loadTables() {
    setLoading("tables");
    setBuilderError(null);
    try {
      setTables(await fetchBuilderTables(dblink));
    } catch (loadError) {
      setBuilderError(messageFrom(loadError));
    } finally {
      setLoading(null);
    }
  }

  async function loadColumns() {
    if (spec.owner === "" || spec.table === "") {
      return;
    }
    setLoading("columns");
    setBuilderError(null);
    try {
      setColumns(
        await fetchBuilderColumns({
          dblink,
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
    // 换表就等于换一份规格：列、主键、条件、排序全是上一张表的列名，留着只会生成一段
    // 引用不存在列的 SQL。目标表名是用户自己写的，不跟着清。
    updateSpec({
      owner: table?.owner ?? "",
      table: table?.name ?? "",
      columns: [],
      primary_key: [],
      conditions: [],
      order_by: [],
    });
  }

  function toggleColumn(column: string) {
    const selected = spec.columns.includes(column);
    updateSpec({
      columns: selected
        ? spec.columns.filter((name) => name !== column)
        : [...spec.columns, column],
      primary_key: selected
        ? spec.primary_key.filter((name) => name !== column)
        : spec.primary_key,
    });
  }

  function toggleKey(column: string) {
    updateSpec({
      primary_key: spec.primary_key.includes(column)
        ? spec.primary_key.filter((name) => name !== column)
        : [...spec.primary_key, column],
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
          value_source: "runtime",
          constant: "",
        },
      ],
    });
  }

  function updateCondition(index: number, change: Partial<Condition>) {
    updateSpec({
      conditions: spec.conditions.map((condition, position) =>
        position === index ? { ...condition, ...change } : condition,
      ),
    });
  }

  function removeCondition(index: number) {
    updateSpec({
      conditions: spec.conditions.filter((_, position) => position !== index),
    });
  }

  function addOrderTerm() {
    const column = columnNames[0];
    if (column === undefined) {
      return;
    }
    updateSpec({ order_by: [...spec.order_by, { column, direction: "asc" }] });
  }

  function updateOrderTerm(index: number, change: Partial<OrderTerm>) {
    updateSpec({
      order_by: spec.order_by.map((term, position) =>
        position === index ? { ...term, ...change } : term,
      ),
    });
  }

  function removeOrderTerm(index: number) {
    updateSpec({
      order_by: spec.order_by.filter((_, position) => position !== index),
    });
  }

  async function handleColumnFetch() {
    setColumnFetch({ kind: "loading" });
    try {
      setColumnFetch({ kind: "ready", result: await fetchColumns(spec) });
    } catch (fetchError) {
      const targetDdlFailure = targetDdlFailureFrom(fetchError);
      if (targetDdlFailure !== null) {
        setColumnFetch({ kind: "target-ddl-failed", ...targetDdlFailure });
      } else {
        setColumnFetch({ kind: "oracle-failed", message: messageFrom(fetchError) });
      }
    }
  }

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      await onSubmit({ name, spec });
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

          <section className="builder-guide" aria-labelledby="builder-source-title">
            <header>
              <div>
                <strong id="builder-source-title">源表</strong>
                <span>Oracle 字典元数据仅展示，不判定类型是否可搬</span>
              </div>
            </header>
            <div className="builder-controls">
              <FormField label="数据库链接（可选）">
                <input
                  value={dblink}
                  placeholder="如 FA"
                  onChange={(event) =>
                    updateSpec({
                      dblink:
                        event.target.value === "" ? undefined : event.target.value,
                    })
                  }
                />
              </FormField>
              <button
                className="button is-ghost"
                type="button"
                onClick={() => void loadTables()}
                disabled={loading !== null}
              >
                {loading === "tables" ? (
                  <LoaderCircle className="is-spinning" size={15} />
                ) : (
                  <RefreshCw size={15} />
                )}
                {loading === "tables" ? "读取中" : "读取表"}
              </button>
              <FormField label="Oracle 表">
                {tables.length === 0 ? (
                  <input
                    readOnly
                    value={spec.owner === "" ? "" : `${spec.owner}.${spec.table}`}
                    placeholder="先读取表"
                  />
                ) : (
                  <select
                    value={tableKey}
                    onChange={(event) => selectTable(event.target.value)}
                  >
                    <option value="">请选择</option>
                    {tables.map((table) => {
                      const key = tableKeyFor(table);
                      return (
                        <option key={key} value={key}>
                          {table.owner}.{table.name}
                        </option>
                      );
                    })}
                  </select>
                )}
              </FormField>
              <button
                className="button is-ghost"
                type="button"
                onClick={() => void loadColumns()}
                disabled={spec.owner === "" || loading !== null}
              >
                {loading === "columns" ? (
                  <LoaderCircle className="is-spinning" size={15} />
                ) : (
                  <TableProperties size={15} />
                )}
                {loading === "columns" ? "读取中" : "读取列"}
              </button>
            </div>
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
                      <th>选择</th>
                      <th>主键</th>
                      <th>列名</th>
                      <th>字典类型</th>
                      <th>精度 / 长度</th>
                      <th>可空</th>
                    </tr>
                  </thead>
                  <tbody>
                    {columnNames.map((columnName) => {
                      const column = dictionary.get(columnName);
                      const selected = spec.columns.includes(columnName);
                      return (
                        <tr key={columnName}>
                          <td>
                            <input
                              type="checkbox"
                              checked={selected}
                              onChange={() => toggleColumn(columnName)}
                              aria-label={`选择 ${columnName}`}
                            />
                          </td>
                          <td>
                            <input
                              type="checkbox"
                              checked={spec.primary_key.includes(columnName)}
                              disabled={!selected}
                              onChange={() => toggleKey(columnName)}
                              aria-label={`把 ${columnName} 设为主键列`}
                            />
                          </td>
                          <td className="mono">{columnName}</td>
                          <td className="mono">{column?.data_type ?? "—"}</td>
                          <td className="mono">
                            {column === undefined ? "—" : columnShape(column)}
                          </td>
                          <td>
                            {column === undefined ? "—" : column.nullable ? "是" : "否"}
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
                {spec.columns.length} 列已选 · {spec.primary_key.length} 列作主键
              </span>
              <span className="builder-key-note">
                主键必选：撤掉 DELETE 之后，去重全靠它（ADR-0035 §2）。
              </span>
            </footer>
          </section>

          <ConditionEditor
            conditions={spec.conditions}
            columnNames={columnNames}
            dictionary={dictionary}
            onAdd={addCondition}
            onChange={updateCondition}
            onRemove={removeCondition}
          />

          <OrderEditor
            terms={spec.order_by}
            columnNames={columnNames}
            onAdd={addOrderTerm}
            onChange={updateOrderTerm}
            onRemove={removeOrderTerm}
          />

          <FormField label="target_table">
            <input
              required
              value={spec.target_table}
              onChange={(event) => updateSpec({ target_table: event.target.value })}
            />
          </FormField>
          <p className="target-side-note">
            目标端只给这一个文本框：不给目标表下拉、不给目标列列表，
            <strong>是不画，不是没画完</strong>。目标表由你自己用下面的建表 SQL 建。
          </p>

          <GeneratedSql sql={sql} error={sqlError} ready={specComplete} />

          <section className="column-fetch-section" aria-labelledby="column-fetch-title">
            <header>
              <div>
                <strong id="column-fetch-title">目标表建表 SQL</strong>
                <span>由当前规格生成的 SQL 开游标 describe 正向推导</span>
              </div>
              <button
                className="button is-primary"
                type="button"
                onClick={() => void handleColumnFetch()}
                disabled={columnFetch.kind === "loading"}
              >
                {columnFetch.kind === "loading" ? (
                  <LoaderCircle className="is-spinning" size={15} aria-hidden="true" />
                ) : (
                  <TableProperties size={15} aria-hidden="true" />
                )}
                {columnFetch.kind === "loading" ? "取列中" : "拿建表 SQL"}
              </button>
            </header>
            <ColumnFetchPanel state={columnFetch} />
          </section>

          {error !== null && (
            <div className="form-error" role="alert">
              {error}
            </div>
          )}
        </div>
        <ModalFooter onClose={onClose} busy={submitting} submitLabel={submitLabel} />
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
                <span>参数名</span>
                <input
                  required
                  value={condition.parameter}
                  onChange={(event) => onChange(index, { parameter: event.target.value })}
                />
              </label>
              <label>
                <span>值来源</span>
                <select
                  value={condition.value_source}
                  onChange={(event) => {
                    const valueSource = event.target.value as Condition["value_source"];
                    onChange(index, {
                      value_source: valueSource,
                      // 运行时填的条件不许同时写死常量值——两个值来源只能有一个说了算。
                      constant: valueSource === "runtime" ? "" : condition.constant,
                    });
                  }}
                >
                  <option value="runtime">运行时填</option>
                  <option value="constant">常量</option>
                </select>
              </label>
              <label>
                <span>常量值</span>
                <input
                  value={condition.constant}
                  disabled={condition.value_source === "runtime"}
                  required={condition.value_source === "constant"}
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
        值一律走绑定变量，常量也有自己的参数名——理由不是防注入，是转义正确性（ADR-0011 §2）。
        只有「运行时填」的参数进运行参数集与并发互斥键。
      </small>
    </section>
  );
}

function OrderEditor({
  terms,
  columnNames,
  onAdd,
  onChange,
  onRemove,
}: {
  terms: OrderTerm[];
  columnNames: string[];
  onAdd: () => void;
  onChange: (index: number, change: Partial<OrderTerm>) => void;
  onRemove: (index: number) => void;
}) {
  return (
    <section className="spec-editor" aria-labelledby="order-title">
      <header>
        <div>
          <strong id="order-title">排序</strong>
          <span>字段 + 正序 / 倒序，可留空</span>
        </div>
        <button
          className="button is-ghost"
          type="button"
          onClick={onAdd}
          disabled={columnNames.length === 0}
        >
          <Plus size={15} aria-hidden="true" />
          添加排序
        </button>
      </header>
      {terms.length === 0 ? (
        <p className="spec-empty">没有排序：由 Oracle 决定返回顺序。</p>
      ) : (
        <ul className="condition-list">
          {terms.map((term, index) => (
            <li key={index} className="condition-row is-order">
              <label>
                <span>字段</span>
                <select
                  value={term.column}
                  onChange={(event) => onChange(index, { column: event.target.value })}
                >
                  {columnOptions(columnNames, term.column)}
                </select>
              </label>
              <label>
                <span>方向</span>
                <select
                  value={term.direction}
                  onChange={(event) =>
                    onChange(index, {
                      direction: event.target.value as OrderTerm["direction"],
                    })
                  }
                >
                  <option value="asc">正序</option>
                  <option value="desc">倒序</option>
                </select>
              </label>
              <button
                className="icon-button is-danger"
                type="button"
                title="删除这条排序"
                aria-label={`删除排序 ${term.column}`}
                onClick={() => onRemove(index)}
              >
                <Trash2 size={15} aria-hidden="true" />
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/**
 * 源端 SQL 的只读展示。
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
          <strong id="generated-sql-title">源端 SQL</strong>
          <span>由规格现算，只读</span>
        </div>
      </header>
      {error !== null ? (
        <div className="form-error" role="alert">
          {error}
        </div>
      ) : sql === null ? (
        <p className="spec-empty">
          {ready ? "正在生成..." : "先选表、勾列，SQL 会自己长出来。"}
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

function ColumnFetchPanel({ state }: { state: ColumnFetchState }) {
  if (state.kind === "idle") {
    return (
      <p className="column-fetch-hint">
        随时可取；目标表名为空时，DDL 会保留可见占位符。
      </p>
    );
  }
  if (state.kind === "loading") {
    return (
      <div className="fetch-progress">
        <span />
        正在连接 Oracle 并 describe 当前 SQL
      </div>
    );
  }
  if (state.kind === "oracle-failed") {
    return (
      <div className="fetch-failure" role="alert">
        <strong>Oracle 取列失败</strong>
        <span>{state.message}</span>
      </div>
    );
  }

  const columns = state.kind === "ready" ? state.result.columns : state.columns;
  return (
    <div className="fetch-ready">
      <p className="fetch-scope-note">
        这份取列结果<strong>刷新即丢</strong>：不进任务定义、不进存储，只是这一次的查看。
      </p>
      <div className="table-wrap">
        <table className="data-grid">
          <thead>
            <tr>
              <th>结果集列名</th>
              <th>describe 类型</th>
              <th>精度 / 长度</th>
            </tr>
          </thead>
          <tbody>
            {columns.map((column) => (
              <tr key={column.name}>
                <td className="mono">{column.name}</td>
                <td className="mono">{fetchedColumnType(column)}</td>
                <td className="mono">{columnShape(column)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="ddl-header">
        <strong>CREATE TABLE</strong>
        {state.kind === "ready" && (
          <button
            className="icon-button"
            type="button"
            title="复制建表 SQL"
            aria-label="复制建表 SQL"
            onClick={() =>
              void navigator.clipboard.writeText(state.result.target_ddl)
            }
          >
            <Copy size={15} aria-hidden="true" />
          </button>
        )}
      </div>
      {state.kind === "ready" ? (
        <>
          <pre className="ddl-output">
            <DdlText ddl={state.result.target_ddl} />
          </pre>
          <div className="row-size-warning">
            <strong>执行时若报 ERROR 1118 Row size too large</strong>
            <span>
              列宽合计超出 MySQL
              单行上限，需缩窄字符列或拆表；这是静态提示，产品不预先判定行长。
            </span>
          </div>
        </>
      ) : (
        <div className="row-size-warning is-crit">
          <strong>这些列无法生成目标表定义，整份建表 SQL 不给。</strong>
          {state.issues.map((issue, index) => (
            <span key={`${issue.column}-${index}`}>
              <span className="mono">{issue.column}</span>（
              <span className="mono">{issue.source}</span>）：{issue.message}
            </span>
          ))}
          <span>请按逐列原因修正后重新取列。</span>
        </div>
      )}
    </div>
  );
}

/**
 * DDL 原样照给，只把待填写的占位符切出来单独标记。
 * target_table 留空时 source 会在 DDL 里保留它（`crates/source/src/target_ddl.rs`），
 * 裸 NUMBER 未配精度时也会保留 `DECIMAL(<p>,<s>)`。
 */
function DdlText({ ddl }: { ddl: string }) {
  const segments = ddlTextSegments(ddl);
  return (
    <>
      {segments.map((segment, index) => (
        <Fragment key={`${segment.kind}-${index}`}>
          {segment.kind === "placeholder" ? (
            <span className="ddl-placeholder">{segment.value}</span>
          ) : (
            segment.value
          )}
        </Fragment>
      ))}
    </>
  );
}

/**
 * 任务列表里那一列条件的单行读法：`列 符 参数名` 或 `列 符 常量`。
 *
 * 一条条件都没有时明写「整表」——留空会被读成「没配好」，而整表取数是允许的形态。
 */
function conditionSummary(spec: TaskSpec): string {
  if (spec.conditions.length === 0) {
    return "整表";
  }
  return spec.conditions
    .map(
      (condition) =>
        `${condition.column} ${comparisonSymbol(condition.operator)} ${
          condition.value_source === "constant"
            ? condition.constant
            : `:${condition.parameter}`
        }`,
    )
    .join(" AND ");
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

function Modal({
  title,
  onClose,
  busy,
  narrow = false,
  wide = false,
  children,
}: {
  title: string;
  onClose: () => void;
  busy: boolean;
  narrow?: boolean;
  wide?: boolean;
  children: ReactNode;
}) {
  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !busy) {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [busy, onClose]);

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !busy) {
          onClose();
        }
      }}
    >
      <section
        className={`modal ${narrow ? "is-narrow" : ""} ${wide ? "is-wide" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby="modal-title"
      >
        <header className="modal-header">
          <h2 id="modal-title">{title}</h2>
          <button
            className="icon-button"
            type="button"
            title="关闭"
            aria-label="关闭"
            onClick={onClose}
            disabled={busy}
          >
            <X size={16} aria-hidden="true" />
          </button>
        </header>
        {children}
      </section>
    </div>
  );
}

function ModalFooter({
  onClose,
  busy,
  submitLabel,
}: {
  onClose: () => void;
  busy: boolean;
  submitLabel: string;
}) {
  return (
    <footer className="modal-footer">
      <button
        className="button is-ghost"
        type="button"
        onClick={onClose}
        disabled={busy}
      >
        取消
      </button>
      <button
        className="button is-primary"
        type="submit"
        disabled={busy}
      >
        {busy ? "正在保存" : submitLabel}
      </button>
    </footer>
  );
}

function FormField({
  label,
  badge,
  children,
}: {
  label: string;
  badge?: string;
  children: ReactNode;
}) {
  return (
    <label className="form-field">
      <span className="field-label">
        {label}
        {badge !== undefined && <span className="field-badge">{badge}</span>}
      </span>
      {children}
    </label>
  );
}
