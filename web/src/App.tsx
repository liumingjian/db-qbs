import {
  Bell,
  CalendarClock,
  Clock3,
  Database,
  Pencil,
  Plus,
  RefreshCw,
  Tag,
  Trash2,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent, ReactNode } from "react";

import {
  createTask,
  deleteTask,
  listTasks,
  taskInputFrom,
  updateTask,
} from "./api";
import type { Task, TaskInput } from "./api";

type DialogState =
  | { kind: "create" }
  | { kind: "edit"; task: Task }
  | { kind: "rename"; task: Task }
  | { kind: "delete"; task: Task }
  | null;

const emptyTask: TaskInput = {
  name: "",
  source_sql: "",
  source_date_col: "",
  target_table: "",
  target_date_col: "",
};

export function App() {
  const [tasks, setTasks] = useState<Task[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(true);
  const [query, setQuery] = useState("");
  const [dialog, setDialog] = useState<DialogState>(null);

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

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="product-brand">
          <span className="brand-mark">Q</span>
          db-qbs
        </div>
        <nav aria-label="主导航">
          <a className="nav-item is-active" href="#tasks" aria-current="page">
            <Database size={15} aria-hidden="true" />
            任务
          </a>
          <span className="nav-item is-disabled">
            <Clock3 size={15} aria-hidden="true" />
            运行历史
            <span className="nav-badge">后续</span>
          </span>
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
          <span className="breadcrumb">
            数据导入 <span aria-hidden="true">/</span> <strong>任务</strong>
          </span>
          <span className="environment">source · 当前实例</span>
        </header>

        <div className="content">
          {loadError !== null && (
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
        </div>

        {dialog?.kind === "create" && (
          <TaskFormDialog
            title="新建任务"
            initial={emptyTask}
            submitLabel="新建"
            onClose={closeDialog}
            onSubmit={handleCreate}
          />
        )}
        {dialog?.kind === "edit" && (
          <TaskFormDialog
            title={`编辑 · ${dialog.task.name}`}
            initial={taskInputFrom(dialog.task)}
            submitLabel="保存"
            editFieldsOnly
            onClose={closeDialog}
            onSubmit={(input) => handleUpdate(dialog.task, input)}
          />
        )}
        {dialog?.kind === "rename" && (
          <RenameDialog
            task={dialog.task}
            onClose={closeDialog}
            onSubmit={(input) => handleUpdate(dialog.task, input)}
          />
        )}
        {dialog?.kind === "delete" && (
          <DeleteDialog
            task={dialog.task}
            onClose={closeDialog}
            onDelete={() => handleDelete(dialog.task)}
          />
        )}
      </main>
    </div>
  );
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

  function updateField<K extends keyof TaskInput>(
    field: K,
    value: TaskInput[K],
  ) {
    setInput((currentInput) => ({ ...currentInput, [field]: value }));
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
    <Modal title={title} onClose={onClose} busy={submitting}>
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
          <FormField label="source_sql">
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
    </Modal>
  );
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
  children,
}: {
  title: string;
  onClose: () => void;
  busy: boolean;
  narrow?: boolean;
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
        className={`modal ${narrow ? "is-narrow" : ""}`}
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

function FormField({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="form-field">
      <span>{label}</span>
      {children}
    </label>
  );
}

function messageFrom(error: unknown): string {
  return error instanceof Error ? error.message : "请求失败，请稍后重试";
}
