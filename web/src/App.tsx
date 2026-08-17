import {
  Bell,
  CalendarClock,
  Check,
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
  WandSparkles,
  X,
} from "lucide-react";
import { Fragment, useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent, ReactNode } from "react";

import {
  ApiError,
  createTask,
  deleteTask,
  fetchBuilderColumns,
  fetchBuilderTables,
  fetchColumns,
  generateBuilderTask,
  inspectSqlShape,
  listTasks,
  taskInputFrom,
  updateTask,
} from "./api";
import type {
  BuilderColumn,
  BuilderTable,
  ColumnFetchResult,
  FetchedColumn,
  ShapeCheck,
  Task,
  TaskDefinition,
  TaskInput,
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
import { shapeRuleDescription, shapeRuleLabel } from "./shape";
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

const emptyTask: TaskInput = {
  name: "",
  source_sql: "",
  source_date_col: "",
  target_table: "",
  target_date_col: "",
};

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
      [task.name, task.task_id, task.source_sql, task.target_table]
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
                      placeholder="任务名 / 目标表 / SQL"
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
            editFieldsOnly
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
            <th>目标表</th>
            <th>源日期列</th>
            <th>目标日期列</th>
            <th>source_sql</th>
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
              <td className="mono">{task.target_table}</td>
              <td className="mono">{task.source_date_col}</td>
              <td className="mono">{task.target_date_col}</td>
              <td className="sql-cell" title={task.source_sql}>
                {task.source_sql}
              </td>
              <td>
                <div className="row-actions">
                  <ActionButton
                    label="发起运行"
                    icon={<Play size={15} />}
                    onClick={() => onAction({ kind: "start", task })}
                  />
                  <ActionButton
                    label="编辑四个字段"
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

function TaskFormDialog({
  title,
  initial,
  submitLabel,
  editFieldsOnly = false,
  onClose,
  onSubmit,
}: {
  title: string;
  initial: TaskInput;
  submitLabel: string;
  editFieldsOnly?: boolean;
  onClose: () => void;
  onSubmit: (input: TaskInput) => Promise<void>;
}) {
  const [input, setInput] = useState<TaskInput>({ ...initial });
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [guideOpen, setGuideOpen] = useState(false);
  const [overwriteConfirm, setOverwriteConfirm] = useState(false);
  const [generatedSql, setGeneratedSql] = useState<string | null>(null);
  const [shapeChecks, setShapeChecks] = useState<ShapeCheck[]>([]);
  const [shapeError, setShapeError] = useState<string | null>(null);
  const [columnFetch, setColumnFetch] = useState<ColumnFetchState>({ kind: "idle" });

  const definition = useMemo(() => taskDefinitionFrom(input), [input]);

  useEffect(() => {
    if (input.source_sql.trim() === "") {
      setShapeChecks([]);
      setShapeError(null);
      return;
    }
    let active = true;
    const timeout = window.setTimeout(() => {
      void inspectSqlShape(definition)
        .then((checks) => {
          if (active) {
            setShapeChecks(checks);
            setShapeError(null);
          }
        })
        .catch((inspectError) => {
          if (active) {
            setShapeError(messageFrom(inspectError));
          }
        });
    }, 250);
    return () => {
      active = false;
      window.clearTimeout(timeout);
    };
  }, [definition]);

  function updateField<K extends keyof TaskInput>(
    field: K,
    value: TaskInput[K],
  ) {
    setInput((currentInput) => ({ ...currentInput, [field]: value }));
    if (field !== "name") {
      setColumnFetch({ kind: "idle" });
    }
  }

  // 角标说的是「你在这一屏改过它」：非空、不是构建器这次生成的、也不是打开时加载进来的那一段。
  // 覆盖确认用的判断比它宽——加载进来的 SQL 同样值得保护，只是它不算「手改」，写上就是假话。
  const sqlManuallyEdited =
    input.source_sql.trim() !== "" &&
    input.source_sql !== generatedSql &&
    input.source_sql !== initial.source_sql;

  function openGuide() {
    const sqlIsProtected =
      input.source_sql.trim() !== "" && input.source_sql !== generatedSql;
    if (sqlIsProtected) {
      setOverwriteConfirm(true);
    } else {
      setGuideOpen(true);
    }
  }

  function applyGeneratedTask(task: TaskDefinition) {
    setInput((currentInput) => ({ ...currentInput, ...task }));
    setGeneratedSql(task.source_sql);
    setGuideOpen(false);
    setColumnFetch({ kind: "idle" });
  }

  async function handleColumnFetch() {
    setColumnFetch({ kind: "loading" });
    try {
      setColumnFetch({ kind: "ready", result: await fetchColumns(definition) });
    } catch (fetchError) {
      const checks = shapeChecksFromColumnFetchError(fetchError);
      if (checks !== null) {
        setColumnFetch({ kind: "shape-failed", checks });
      } else {
        const targetDdlFailure = targetDdlFailureFrom(fetchError);
        if (targetDdlFailure !== null) {
          setColumnFetch({ kind: "target-ddl-failed", ...targetDdlFailure });
        } else {
          setColumnFetch({ kind: "oracle-failed", message: messageFrom(fetchError) });
        }
      }
    }
  }

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      await onSubmit(input);
      onClose();
    } catch (submitError) {
      setError(messageFrom(submitError));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Modal
      title={title}
      onClose={onClose}
      busy={submitting || overwriteConfirm}
      wide
    >
      <form onSubmit={(event) => void handleSubmit(event)}>
        <div className="modal-body form-stack">
          {!editFieldsOnly && (
            <FormField label="任务名称">
              <input
                autoFocus
                required
                value={input.name}
                onChange={(event) => updateField("name", event.target.value)}
              />
            </FormField>
          )}
          <section className="builder-entry" aria-label="SQL 构建器">
            <div>
              <strong>SQL 构建器</strong>
              <span>一次性生成四字段，选择状态不会保存</span>
            </div>
            <button className="button is-ghost" type="button" onClick={openGuide}>
              <WandSparkles size={15} aria-hidden="true" />
              {input.source_sql.trim() === "" ? "打开构建器" : "重走向导"}
            </button>
          </section>
          {guideOpen && (
            <SqlBuilderGuide
              targetTable={input.target_table}
              onCancel={() => setGuideOpen(false)}
              onGenerated={applyGeneratedTask}
            />
          )}
          <FormField
            label="source_sql"
            badge={sqlManuallyEdited ? "当前 SQL 已被手改" : undefined}
          >
            <textarea
              autoFocus={editFieldsOnly}
              required
              rows={7}
              value={input.source_sql}
              onChange={(event) =>
                updateField("source_sql", event.target.value)
              }
            />
          </FormField>
          <ShapeFeedback checks={shapeChecks} error={shapeError} />
          <div className="field-grid">
            <FormField label="source_date_col">
              <input
                required
                value={input.source_date_col}
                onChange={(event) =>
                  updateField("source_date_col", event.target.value)
                }
              />
            </FormField>
            <FormField label="target_table">
              <input
                required
                value={input.target_table}
                onChange={(event) =>
                  updateField("target_table", event.target.value)
                }
              />
            </FormField>
            <FormField label="target_date_col">
              <input
                required
                value={input.target_date_col}
                onChange={(event) =>
                  updateField("target_date_col", event.target.value)
                }
              />
            </FormField>
          </div>
          <p className="target-side-note">
            目标端只给这两个文本框：不给目标表下拉、不给目标列列表，
            <strong>是不画，不是没画完</strong>。目标表由你自己用下面的建表 SQL 建。
          </p>
          <section className="column-fetch-section" aria-labelledby="column-fetch-title">
            <header>
              <div>
                <strong id="column-fetch-title">目标表建表 SQL</strong>
                <span>由当前 SQL 的游标 describe 正向推导</span>
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
        <ModalFooter
          onClose={onClose}
          busy={submitting}
          submitLabel={submitLabel}
        />
      </form>
      {overwriteConfirm && (
        <div className="confirm-backdrop" role="presentation">
          <section
            className="confirm-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="overwrite-title"
          >
            <header id="overwrite-title">重走向导会覆盖你手改的 SQL</header>
            <p>
              构建器会用新生成的四字段整段替换当前内容，也不会恢复上一次的勾选状态。
            </p>
            <footer>
              <button
                className="button is-ghost"
                type="button"
                onClick={() => setOverwriteConfirm(false)}
              >
                取消
              </button>
              <button
                className="button is-danger"
                type="button"
                onClick={() => {
                  setOverwriteConfirm(false);
                  setGuideOpen(true);
                }}
              >
                覆盖并重走向导
              </button>
            </footer>
          </section>
        </div>
      )}
    </Modal>
  );
}

type ColumnFetchState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "shape-failed"; checks: ShapeCheck[] }
  | { kind: "oracle-failed"; message: string }
  | { kind: "target-ddl-failed"; columns: FetchedColumn[]; issues: TargetDdlIssue[] }
  | { kind: "ready"; result: ColumnFetchResult };

function SqlBuilderGuide({
  targetTable,
  onCancel,
  onGenerated,
}: {
  targetTable: string;
  onCancel: () => void;
  onGenerated: (task: TaskDefinition) => void;
}) {
  const [dblink, setDblink] = useState("");
  const [tables, setTables] = useState<BuilderTable[]>([]);
  const [tableKey, setTableKey] = useState("");
  const [columns, setColumns] = useState<BuilderColumn[]>([]);
  const [selectedColumns, setSelectedColumns] = useState<string[]>([]);
  const [dateColumn, setDateColumn] = useState("");
  const [loading, setLoading] = useState<
    "tables" | "columns" | "generate" | null
  >(null);
  const [error, setError] = useState<string | null>(null);

  const selectedTable = tables.find(
    (table) => tableKeyFor(table) === tableKey,
  );
  const loadingTables = loading === "tables";
  const loadingColumns = loading === "columns";
  const generatingTask = loading === "generate";

  async function loadTables() {
    setLoading("tables");
    setError(null);
    try {
      setTables(await fetchBuilderTables(dblink));
      setTableKey("");
      setColumns([]);
      setSelectedColumns([]);
      setDateColumn("");
    } catch (loadError) {
      setError(messageFrom(loadError));
    } finally {
      setLoading(null);
    }
  }

  async function loadColumns() {
    if (selectedTable === undefined) {
      return;
    }
    setLoading("columns");
    setError(null);
    try {
      const loaded = await fetchBuilderColumns({
        dblink,
        owner: selectedTable.owner,
        table: selectedTable.name,
      });
      setColumns(loaded);
      setSelectedColumns(loaded.map((column) => column.name));
      setDateColumn("");
    } catch (loadError) {
      setError(messageFrom(loadError));
    } finally {
      setLoading(null);
    }
  }

  function toggleColumn(column: string) {
    setSelectedColumns((selected) => {
      if (selected.includes(column)) {
        return selected.filter((name) => name !== column);
      }
      return [...selected, column];
    });
    if (dateColumn === column) {
      setDateColumn("");
    }
  }

  async function generateTask() {
    if (selectedTable === undefined || dateColumn === "") {
      return;
    }
    setLoading("generate");
    setError(null);
    try {
      onGenerated(
        await generateBuilderTask({
          dblink,
          owner: selectedTable.owner,
          table: selectedTable.name,
          columns: selectedColumns,
          source_date_col: dateColumn,
          target_table: targetTable,
          target_date_col: dateColumn,
        }),
      );
    } catch (generateError) {
      setError(messageFrom(generateError));
      setLoading(null);
    }
  }

  return (
    <section className="builder-guide" aria-labelledby="builder-guide-title">
      <header>
        <div>
          <strong id="builder-guide-title">选表、勾列、指定日期列</strong>
          <span>Oracle 字典元数据仅展示，不判定类型是否可搬</span>
        </div>
        <button
          className="icon-button"
          type="button"
          title="收起构建器"
          aria-label="收起构建器"
          onClick={onCancel}
        >
          <X size={15} aria-hidden="true" />
        </button>
      </header>
      <div className="builder-controls">
        <FormField label="数据库链接（可选）">
          <input
            value={dblink}
            placeholder="如 FA"
            onChange={(event) => setDblink(event.target.value)}
          />
        </FormField>
        <button
          className="button is-ghost"
          type="button"
          onClick={() => void loadTables()}
          disabled={loading !== null}
        >
          {loadingTables ? (
            <LoaderCircle className="is-spinning" size={15} />
          ) : (
            <RefreshCw size={15} />
          )}
          {loadingTables ? "读取中" : "读取表"}
        </button>
        <FormField label="Oracle 表">
          <select
            value={tableKey}
            onChange={(event) => setTableKey(event.target.value)}
            disabled={tables.length === 0}
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
        </FormField>
        <button
          className="button is-ghost"
          type="button"
          onClick={() => void loadColumns()}
          disabled={selectedTable === undefined || loading !== null}
        >
          {loadingColumns ? (
            <LoaderCircle className="is-spinning" size={15} />
          ) : (
            <TableProperties size={15} />
          )}
          {loadingColumns ? "读取中" : "读取列"}
        </button>
      </div>
      {columns.length > 0 && (
        <div className="builder-columns table-wrap">
          <table className="data-grid">
            <thead>
              <tr>
                <th>选择</th>
                <th>日期列</th>
                <th>列名</th>
                <th>字典类型</th>
                <th>精度 / 长度</th>
                <th>可空</th>
              </tr>
            </thead>
            <tbody>
              {columns.map((column) => {
                const selected = selectedColumns.includes(column.name);
                return (
                  <tr key={column.name}>
                    <td>
                      <input
                        type="checkbox"
                        checked={selected}
                        onChange={() => toggleColumn(column.name)}
                        aria-label={`选择 ${column.name}`}
                      />
                    </td>
                    <td>
                      <input
                        type="radio"
                        name="source-date-column"
                        checked={dateColumn === column.name}
                        disabled={!selected}
                        onChange={() => setDateColumn(column.name)}
                        aria-label={`指定 ${column.name} 为日期列`}
                      />
                    </td>
                    <td className="mono">{column.name}</td>
                    <td className="mono">{column.data_type}</td>
                    <td className="mono">{columnShape(column)}</td>
                    <td>{column.nullable ? "是" : "否"}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      {error !== null && (
        <div className="form-error" role="alert">
          {error}
        </div>
      )}
      <footer>
        <span>{selectedColumns.length} 列已选</span>
        <button
          className="button is-primary"
          type="button"
          onClick={() => void generateTask()}
          disabled={
            selectedTable === undefined ||
            selectedColumns.length === 0 ||
            dateColumn === "" ||
            loading !== null
          }
        >
          {generatingTask ? (
            <LoaderCircle className="is-spinning" size={15} />
          ) : (
            <WandSparkles size={15} />
          )}
          {generatingTask ? "生成中" : "生成四字段"}
        </button>
      </footer>
    </section>
  );
}

function ShapeFeedback({
  checks,
  error,
}: {
  checks: ShapeCheck[];
  error: string | null;
}) {
  if (error !== null) {
    return (
      <div className="shape-feedback is-unavailable">
        形状反馈暂不可用：{error}
      </div>
    );
  }

  const failures = checks.filter((check) => !check.passed);
  if (failures.length === 0) {
    return null;
  }

  return (
    <div className="shape-feedback" role="alert">
      <strong>SQL 形状预检未通过</strong>
      {failures.map((check) => (
        <span key={check.rule}>
          {shapeRuleLabel(check.rule)}：
          {shapeRuleDescription(check.rule, check.message)}
        </span>
      ))}
    </div>
  );
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
  if (state.kind === "shape-failed") {
    return (
      <div className="fetch-failure" role="alert">
        <strong>这段 SQL 的形状不能跑，取列未执行。</strong>
        <ShapeCheckTable checks={state.checks} />
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

function ShapeCheckTable({ checks }: { checks: ShapeCheck[] }) {
  return (
    <div className="table-wrap">
      <table className="data-grid shape-grid">
        <thead>
          <tr>
            <th>约束</th>
            <th>结果</th>
          </tr>
        </thead>
        <tbody>
          {checks.map((check) => (
            <tr key={check.rule}>
              <td>{shapeRuleLabel(check.rule)}</td>
              <td className={check.passed ? "shape-pass" : "shape-fail"}>
                {check.passed ? (
                  <>
                    <Check size={14} />通过
                  </>
                ) : (
                  shapeRuleDescription(check.rule, check.message)
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function taskDefinitionFrom(input: TaskInput): TaskDefinition {
  return {
    source_sql: input.source_sql,
    source_date_col: input.source_date_col,
    target_table: input.target_table,
    target_date_col: input.target_date_col,
    ...(input.column_precision === undefined
      ? {}
      : { column_precision: input.column_precision }),
  };
}

function shapeChecksFromColumnFetchError(error: unknown): ShapeCheck[] | null {
  if (!(error instanceof ApiError) || !isRecord(error.body)) {
    return null;
  }
  if (error.body.kind !== "sql_shape" || !Array.isArray(error.body.checks)) {
    return null;
  }
  if (!error.body.checks.every(isShapeCheck)) {
    return null;
  }
  return error.body.checks;
}

function isShapeCheck(value: unknown): value is ShapeCheck {
  return (
    isRecord(value) &&
    typeof value.rule === "string" &&
    typeof value.passed === "boolean" &&
    typeof value.message === "string"
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
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
