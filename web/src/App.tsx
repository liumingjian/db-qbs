import {
  ChevronDown,
  ChevronRight,
  Database,
  LoaderCircle,
  Menu,
  PanelLeftClose,
  Radio,
  RefreshCw,
  Search,
  Server,
  Settings,
  TableProperties,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";

import {
  createTask,
  cancelRun,
  deleteTask,
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
  startRun,
  taskInputFrom,
  updateTask,
} from "./api";
import type {
  Agent,
  BuilderColumn,
  ColumnMapping,
  BuilderSql,
  BuilderTable,
  RunHistory,
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
import { latestRunByTask, runStatus } from "./listing";
import { RunScreen } from "./RunScreen";
import { SettingsScreen } from "./SettingsScreen";
import { HighlightedSqlInput, SqlEditor } from "./SqlEditor";
import type { TargetColumnName } from "./spec";
import {
  matchSameNameTargets,
  renameTargetField,
  targetFieldOf,
} from "./spec";
import { DatasourceScreen } from "./DatasourceScreen";
import { evaluateEdit, evaluateEntry, gateFix, gateReason } from "./entry";
import type { EntryFix, EntryGuard } from "./entry";
import { TaskEntryDialog } from "./TaskEntryDialog";
import { TaskWizardScreen } from "./TaskWizardScreen";
import { FormField, Modal, ModalFooter } from "./ui";
import { openExisting, openNew, taskName, toSpec } from "./wizard";
import type { Draft, Step } from "./wizard";

type DialogState =
  | { kind: "entry" }
  | { kind: "edit"; task: Task; requestedStep: Step }
  | { kind: "rename"; task: Task }
  | { kind: "delete"; task: Task }
  | null;

/**
 * 导航四项：**作业中心 · 数据源 · 目标端 Agent · 系统设置**（ADR-0046 §2 改写 ADR-0044 §6 的次序）。
 * 「运行历史」独立屏随作业中心的合并整屏取消。
 *
 * 次序按**回访频次**排，不按依赖链：agent 屏是装机时去一次、出事时回去一次的运维屏，
 * 「先有 agent 才建得出 MySQL 数据源」只在第一次装机那一天成立，不该让它占住天天要点的第一格。
 * 落地页本来也一直是作业中心（见 `pageFromHash` 的兜底），导航第一项是 agent 时，
 * 高亮的那一项和展开的那一屏对不上。
 */
type NavigationPage = "jobs" | "datasources" | "agents" | "settings";
type Page = NavigationPage | "wizard";

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
  if (hash === "#wizard") {
    return "wizard";
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

const NAV_ITEMS: readonly { page: NavigationPage; label: string }[] = [
  { page: "jobs", label: "作业中心" },
  { page: "datasources", label: "数据源" },
  { page: "agents", label: "目标端 Agent" },
  { page: "settings", label: "系统设置" },
];

function navIcon(page: NavigationPage, size: number) {
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
  const [wizardDraft, setWizardDraft] = useState<Draft | null>(null);
  const [focusTaskId, setFocusTaskId] = useState<string | null>(null);
  const [activeRun, setActiveRun] = useState<{
    task: Task;
    runRecordId: string;
  } | null>(null);
  /**
   * 发起失败的那句话。发起不再有对话框可以把错误挂在里面，所以它挂在屏顶——
   * 「同一任务已有一次运行进行中」这类 409 就是从这里读到的。
   */
  const [startError, setStartError] = useState<string | null>(null);
  /**
   * 正在发起的那个任务。挡的是**同一行连点两下**：第二下会撞回一个 409，
   * 而那不是用户做错了什么。别的行照常按得动——发起是逐条串行打后端的，
   * 一条在飞不构成拦住其余每一行的理由。
   */
  const [startingTaskId, setStartingTaskId] = useState<string | null>(null);

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
    () => runHistory.some((run) => runStatus(run) === "live"),
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

  const consumeTaskFocus = useCallback(() => setFocusTaskId(null), []);

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

  useEffect(() => {
    if (page === "wizard" && wizardDraft === null) {
      window.location.replace("#jobs");
    }
  }, [page, wizardDraft]);

  useEffect(() => {
    if (dialog?.kind !== "edit") return;
    const guard = evaluateEdit(dialog.task, datasources, agents, datasourcesLoading);
    if (guard.kind !== "open") return;
    const source = guard.sources.find(
      (option) => option.datasource_id === dialog.task.source_datasource_id,
    );
    const target = guard.targets.find(
      (option) => option.datasource_id === dialog.task.target_datasource_id,
    );
    if (source === undefined || target === undefined) return;
    setActiveRun(null);
    setWizardDraft(openExisting(
      dialog.task,
      { datasource_id: source.datasource_id, name: source.name },
      { datasource_id: target.datasource_id, name: target.name },
      target.agentStatus === "online",
      dialog.requestedStep,
    ));
    setDialog(null);
    setPage("wizard");
    window.location.hash = "wizard";
  }, [agents, datasources, datasourcesLoading, dialog]);

  const latestRuns = useMemo(() => latestRunByTask(runHistory), [runHistory]);

  function toggleSider() {
    setCollapsed((current) => {
      writeCollapsed(!current);
      return !current;
    });
  }

  function openCreateDialog() {
    setDialog({ kind: "entry" });
  }

  function closeDialog() {
    setDialog(null);
  }

  function navigate(nextPage: Page) {
    setActiveRun(null);
    if (nextPage !== "wizard") setWizardDraft(null);
    setPage(nextPage);
    window.location.hash = nextPage;
  }

  async function handleWizardSubmit(draft: Draft, action: "start" | "save-only") {
    const input = {
      name: taskName(draft),
      source_datasource_id: draft.source.datasource_id,
      target_datasource_id: draft.target.datasource_id,
      spec: toSpec(draft),
    };
    if (draft.mode === "edit" && draft.taskId !== null) {
      const updated = await updateTask(draft.taskId, input);
      setTasks((currentTasks) =>
        currentTasks?.map((task) => task.task_id === updated.task_id ? updated : task) ?? [updated],
      );
      setFocusTaskId(updated.task_id);
      navigate("jobs");
      void loadList();
      return;
    }
    const created = await createTask(input);
    setTasks((currentTasks) => [...(currentTasks ?? []), created]);
    if (action === "start") {
      try {
        await startRun(created.task_id);
      } catch (error) {
        setStartError(`${created.name} 已保存，但发起失败：${messageFrom(error)}`);
      }
    }
    setFocusTaskId(created.task_id);
    navigate("jobs");
    void loadList();
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

  /**
   * 发起一次运行：**点了就跑**。
   *
   * 没有对话框、没有参数表单——任务定义里已经写全了要跑什么。跑起来之后直接落到
   * 运行详情，那是唯一还有事可看的地方；失败时把话留在屏顶，人还站在原来的列表上。
   */
  async function handleStart(task: Task) {
    if (startingTaskId === task.task_id) {
      return;
    }
    setStartingTaskId(task.task_id);
    setStartError(null);
    try {
      const accepted = await startRun(task.task_id);
      setActiveRun({ task, runRecordId: accepted.run_record_id });
    } catch (error) {
      setStartError(`${task.name}：${messageFrom(error)}`);
    } finally {
      setStartingTaskId(null);
    }
    void loadList();
  }

  async function handleStop(runRecordId: string) {
    setStartError(null);
    try {
      await cancelRun(runRecordId);
    } catch (error) {
      setStartError(messageFrom(error));
    } finally {
      void loadList();
    }
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
  const navPage = activeRun !== null || page === "wizard" ? null : page;
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
            <strong>
              {activeRun !== null
                ? "运行详情"
                : page === "wizard" && wizardDraft?.mode === "edit"
                  ? "编辑任务"
                  : pageLabel(page)}
            </strong>
          </span>
          <span className="topbar-right">
            <span className="environment">当前工作台</span>
          </span>
        </header>

        <div className={`content ${page === "wizard" ? "is-wizard" : ""}`}>
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

          {startError !== null && (
            <div className="notice is-error" role="alert">
              <span>{startError}</span>
              <button
                className="text-button"
                type="button"
                onClick={() => setStartError(null)}
              >
                知道了
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
              onRelaunch={() => void handleStart(activeRun.task)}
              onEditTask={(requestedStep) =>
                setDialog({ kind: "edit", task: activeRun.task, requestedStep })
              }
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
              onEdit={(task) => setDialog({ kind: "edit", task, requestedStep: 1 })}
              onRename={(task) => setDialog({ kind: "rename", task })}
              onDelete={(task) => setDialog({ kind: "delete", task })}
              startingTaskId={startingTaskId}
              onStart={(task) => void handleStart(task)}
              onStop={(runRecordId) => void handleStop(runRecordId)}
              /* 重跑与发起现在是**同一件事**：按任务当前的定义再跑一次。
                 上一次没有留下任何需要预填的取值，所以也没有第二条代码路径。 */
              onRerun={handleStart}
              onEditFailure={(task, requestedStep) =>
                setDialog({ kind: "edit", task, requestedStep })
              }
              onChanged={() => void loadList()}
              focusTaskId={focusTaskId}
              onFocusConsumed={consumeTaskFocus}
            />
          )}

          {activeRun === null && page === "wizard" && wizardDraft !== null && (
            <TaskWizardScreen
              initial={wizardDraft}
              onCancel={() => navigate("jobs")}
              onSubmit={handleWizardSubmit}
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

        {page === "jobs" && dialog?.kind === "entry" && (
          <TaskEntryDialog
            guard={evaluateEntry(datasources, agents, datasourcesLoading)}
            onClose={closeDialog}
            onFix={(fix) => {
              closeDialog();
              navigate(fix);
            }}
            onContinue={(sourceDatasourceId, targetDatasourceId) => {
              const guard = evaluateEntry(datasources, agents, datasourcesLoading);
              if (guard.kind !== "open") return;
              const source = guard.sources.find((option) => option.datasource_id === sourceDatasourceId);
              const target = guard.targets.find((option) => option.datasource_id === targetDatasourceId);
              if (source === undefined || target === undefined) return;
              setDialog(null);
              setWizardDraft(openNew(
                { datasource_id: source.datasource_id, name: source.name },
                { datasource_id: target.datasource_id, name: target.name },
                target.agentStatus === "online",
              ));
              setPage("wizard");
              window.location.hash = "wizard";
            }}
          />
        )}
        {page === "jobs" && dialog?.kind === "edit" && (
          <TaskEditGuardDialog
            guard={evaluateEdit(dialog.task, datasources, agents, datasourcesLoading)}
            onClose={closeDialog}
            onFix={(fix) => {
              closeDialog();
              navigate(fix);
            }}
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
    case "wizard":
      return "新建导入";
  }
}

function TaskEditGuardDialog({
  guard,
  onClose,
  onFix,
}: {
  guard: EntryGuard;
  onClose: () => void;
  onFix: (fix: EntryFix) => void;
}) {
  if (guard.kind === "open") return null;
  if (guard.kind === "loading") {
    return (
      <Modal title="检查编辑条件" onClose={onClose} busy={false} narrow>
        <div className="modal-body entry-loading">
          <LoaderCircle className="is-spinning" size={18} aria-hidden="true" />
          正在检查任务绑定的数据源
        </div>
      </Modal>
    );
  }
  const fix = gateFix(guard.gate);
  return (
    <Modal title="暂时不能编辑任务" onClose={onClose} busy={false} narrow>
      <div className="modal-body entry-blocked">
        <strong>{gateReason(guard.gate)}</strong>
        <span>补齐这项条件后，再从作业中心编辑任务。</span>
      </div>
      <footer className="modal-footer">
        <button className="button is-ghost" type="button" onClick={onClose}>
          取消
        </button>
        <button className="button is-primary" type="button" onClick={() => onFix(fix)}>
          {fix === "agents" ? "前往目标端 Agent" : "前往数据源"}
        </button>
      </footer>
    </Modal>
  );
}

type TargetMetaState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "failed"; message: string }
  | { kind: "ready"; table: string; data: TargetTableMetadata };

type SourceQueryMode = "table" | "sql";

/**
 * 自定义 SQL 模式下过滤条件必须是空的——那条路上的过滤只能写在 SQL 自己里。
 * 编辑一条老任务时顺手抹掉，免得一个看不见的字段把保存卡在服务端的校验上。
 */
function normalizeTaskSpecForEditor(spec: TaskSpec): TaskSpec {
  const isSql = Boolean(spec.source_sql?.trim());
  return { ...spec, where_clause: isSql ? "" : (spec.where_clause ?? "") };
}

/**
 * 任务定义编辑器 —— 构建器本身就是任务定义。
 *
 * 它不再是「一次性生成四个字段、选择态丢弃」的向导：结构化规格是唯一真相源，
 * 选表、勾列、勾主键、过滤条件**全部原样存进任务定义**，再次打开还在。
 * SQL 是规格的派生面，只读展示，没有编辑入口。
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
  const sourceDatasourceId = initial.source_datasource_id;
  const targetDatasourceId = initial.target_datasource_id;
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
  const selectedTargetDatasource = datasources.find(
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
  // 跟源列表同一个顺序。按 `spec.columns` 排会变成「勾选先后」——取消再勾一列，
  // 它就跳到映射表末尾，两张表对不上行。
  const selectedSourceColumns = useMemo(
    () =>
      columnNames.filter((name) =>
        spec.columns.some((mapping) => mapping.source === name),
      ),
    [columnNames, spec.columns],
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
      .then((data) => {
        setTargetMeta({ kind: "ready", table, data });
        autoFillSameNameTargets(data.columns);
      })
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

  // 当前表单里有没有会被切模式抹掉的东西。空表单上弹确认框是纯噪声，
  // 所以这个判断存在的全部意义就是「只在真有东西可丢时才拦一下」。
  const sourceWorkInProgress =
    spec.owner !== "" ||
    (spec.source_sql?.trim() ?? "") !== "" ||
    spec.columns.length > 0 ||
    (spec.where_clause ?? "").trim() !== "";

  function switchSourceQueryMode(nextMode: SourceQueryMode) {
    if (nextMode === sourceQueryMode) {
      return;
    }
    // 切模式会把源表 / 结果列、字段映射、主键、过滤条件整套清掉——它们全是上一条
    // 取数路径的产物，留着只会生成一段引用不存在列的 SQL。原来这一步是**静默**的：
    // 填了半小时的映射一点就没，且没有撤销。
    if (
      sourceWorkInProgress &&
      !window.confirm(
        "切换取数方式会清空当前的源表 / 结果列、字段映射、主键和过滤条件。确定切换？",
      )
    ) {
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
      where_clause: "",
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
        // 首次读取默认全勾（多数场景就是整段搬）；已经勾过就只做交集——
        // 「读取列」是刷新结果列，不是重置这张表单。改 SQL 文本本身已经在
        // textarea 的 onChange 里清空过勾选，所以这里留下来的一定是同一条 SQL 的选择。
        const previous = new Map(
          spec.columns.map((mapping) => [mapping.source, mapping]),
        );
        const nextMappings: ColumnMapping[] =
          spec.columns.length === 0
            ? nextColumns.map((column) => ({ source: column.name, target: "" }))
            : nextColumns
                .map((column) => previous.get(column.name))
                .filter((mapping): mapping is ColumnMapping => mapping !== undefined);
        // 两个方向都要接线。读目标列那一路在 `loadTargetColumns` 里接；
        // 反过来——目标列已经读过、这会儿才读源列——新种下的映射全是空的，
        // 那一路的 `.then` 早跑完了，不在这里补就永远没人补。
        const seeded =
          targetMeta.kind === "ready"
            ? matchSameNameTargets(
                { columns: nextMappings, primary_key: spec.primary_key },
                targetMeta.data.columns,
                { onlyUnmapped: true },
              )
            : { columns: nextMappings, primary_key: spec.primary_key };
        const survivingAfterMatch = new Set(
          seeded.columns.map((mapping) => mapping.target),
        );
        updateSpec({
          columns: seeded.columns,
          primary_key: seeded.primary_key.filter((name) =>
            survivingAfterMatch.has(name),
          ),
          where_clause: "",
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
      where_clause: "",
    });
    if (table !== undefined) {
      setSourceExpandedOwners((current) => new Set(current).add(table.owner));
    }
  }

  // 勾源列只表达“本次要搬这列”。目标字段要等目标表列读取后，在字段映射区明确选择。
  // 自定义 SQL 模式同样可勾：没勾的列不进投影，也就不会过线（见 TaskSpec::source_sql）。
  function toggleColumn(column: string) {
    const mapping = spec.columns.find((candidate) => candidate.source === column);
    if (mapping === undefined) {
      updateSpec({
        columns: [...spec.columns, { source: column, target: "" }],
      });
      return;
    }
    // 主键存的是目标字段，取消源列时按当前目标名摘掉。
    updateSpec({
      columns: spec.columns.filter((candidate) => candidate.source !== column),
      primary_key: spec.primary_key.filter((name) => name !== mapping.target),
    });
  }

  function toggleAllColumns() {
    if (allSourceColumnsSelected) {
      // 取消全选只是不搬这些列了，**表还是那张表**——过滤条件照旧成立，不清。
      // （换表 / 换取数方式 / 换数据源才清：那时列名整套都不是同一批了。）
      updateSpec({ columns: [], primary_key: [] });
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

  /**
   * 目标列一读回来就把**同名的**源列自动接上——只填还空着的，已经选好的不动。
   *
   * 原来这一步要用户自己去点「同名填充」：`ID` / `C_NAME` / `LOAD_DATE` 名字一模一样，
   * 四个下拉却都停在「请选择目标字段」，等于让人手工确认一件机器已经知道的事。
   * 只填空位是关键——重新读取目标列不该把用户手改过的映射冲掉。
   */
  function autoFillSameNameTargets(targetColumns: readonly TargetColumnName[]) {
    setSpec((current) => ({
      ...current,
      ...matchSameNameTargets(current, targetColumns, { onlyUnmapped: true }),
    }));
  }

  /** 全部退回未映射。主键存的是目标字段，一并清掉，否则会剩下一批指向空名字的键。 */
  function clearMapping() {
    updateSpec({
      columns: spec.columns.map((mapping) => ({ ...mapping, target: "" })),
      primary_key: [],
    });
  }

  /** 「同名填充」那颗键：用户显式要求，所以覆盖已有映射。 */
  function fillSameNameTargets() {
    if (targetMeta.kind !== "ready") {
      return;
    }
    updateSpec(
      matchSameNameTargets(spec, targetMeta.data.columns, {
        onlyUnmapped: false,
      }),
    );
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

          <section className="builder-guide" aria-labelledby="builder-source-title">
            <header>
              <div>
                {/* 一个概念一个名字：tab、卡片标题、字段标签原来是「自定义 SQL」/
                    「源 SQL」/「自定义 SQL」三种说法。统一到 tab 上那个。 */}
                <strong id="builder-source-title">
                  {sourceQueryMode === "table" ? "源表" : "自定义 SQL"}
                </strong>
                <span>
                  {sourceQueryMode === "table"
                    ? "按库展开表，输入关键字可筛选"
                    : "输入一条只读 SELECT，读取结果列后继续做目标映射"}
                </span>
              </div>
              {/* 取数方式这个岔路口决定下面出现的是一棵库表树还是一个 textarea，
                  原来却摆在上一张卡的页脚、和辅助说明同一个字重。搬到它真正控制的
                  这张卡的头上。 */}
              <div className="builder-mode-switch">
                <div
                  className="builder-mode-buttons"
                  role="tablist"
                  aria-label="取数方式"
                >
                  <button
                    className={
                      sourceQueryMode === "table" ? "button is-primary" : "button"
                    }
                    type="button"
                    role="tab"
                    aria-selected={sourceQueryMode === "table"}
                    onClick={() => switchSourceQueryMode("table")}
                  >
                    按表选择
                  </button>
                  <button
                    className={
                      sourceQueryMode === "sql" ? "button is-primary" : "button"
                    }
                    type="button"
                    role="tab"
                    aria-selected={sourceQueryMode === "sql"}
                    onClick={() => switchSourceQueryMode("sql")}
                  >
                    自定义 SQL
                  </button>
                </div>
                <button
                  className="button is-ghost"
                  type="button"
                  onClick={() =>
                    void (sourceQueryMode === "table" ? loadTables() : loadColumns())
                  }
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
              </div>
            </header>
            {sourceQueryMode === "table" && (
              <div className="builder-controls">
                <FormField
                  label="源端 DBLINK（可选）"
                  badge={
                    dblinksLoading
                      ? "读取中"
                      : dblinks.length > 0
                        ? `发现 ${dblinks.length} 个`
                        : undefined
                  }
                  neutralBadge
                  inlineBadge
                >
                  <span className="combo-input">
                    <input
                      list="source-dblinks"
                      value={spec.dblink ?? ""}
                      placeholder={dblinksLoading ? "正在读取" : "不走 dblink 就留空"}
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
                          where_clause: "",
                        });
                      }}
                    />
                    <ChevronDown size={15} aria-hidden="true" />
                  </span>
                  <datalist id="source-dblinks">
                    {dblinks.map((dblink) => (
                      <option key={dblink} value={dblink} />
                    ))}
                  </datalist>
                </FormField>
              </div>
            )}
            {sourceQueryMode === "sql" ? (
              <SqlEditor
                value={spec.source_sql ?? ""}
                placeholder={"SELECT *\nFROM APP.T_CUSTOMER@POC_LINK_A\nWHERE STATUS = 1"}
                onChange={(next) => {
                  setColumns([]);
                  updateSpec({
                    source_sql: next,
                    columns: [],
                    primary_key: [],
                    where_clause: "",
                  });
                }}
                /* 格式化只动空白（`formatSql` 的不变式），结果列还是同一批——
                   把已读的列和已勾的主键清掉，等于罚人排一次版。 */
                onFormat={(next) => updateSpec({ source_sql: next })}
              />
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
              onClearMapping={clearMapping}
            />
          </section>

          {sourceQueryMode === "table" ? (
            <WhereClauseEditor
              value={spec.where_clause ?? ""}
              columnNames={columnNames}
              onChange={(where_clause) => updateSpec({ where_clause })}
            />
          ) : (
            /* 这张卡原来在 SQL 模式下整个消失，不留一句话——上一屏还有的东西
               下一屏没了，读起来像丢了功能。留住卡，明说它去哪了。 */
            <section className="spec-editor" aria-labelledby="conditions-title">
              <header>
                <div>
                  <strong id="conditions-title">过滤条件</strong>
                  <span>由你写的 SQL 决定</span>
                </div>
              </header>
              <p className="spec-empty">
                自定义 SQL 模式：过滤请直接写进上面的 SQL。
              </p>
            </section>
          )}

          {/* 两种模式都要看到构建 SQL。SQL 模式下**尤其**要看：你写的那段不是原样
              执行的，外面套了一层只取勾选列的投影，这里是唯一能核对最终语句的地方。 */}
          <GeneratedSql
            sql={sql}
            error={sqlError}
            ready={specComplete}
            mode={sourceQueryMode}
          />

          {error !== null && (
            <div className="form-error" role="alert">
              {error}
            </div>
          )}
        </div>
        {/* 保存只看规格自己完不完整。`sqlError` 是**预览**这一路的偶发失败
            （一次 500、一次断网），拿它挡保存等于让人对着一个没有重试入口的
            禁用按钮干瞪眼；规格真有毛病由服务端 `validate()` 在保存时硬拒。 */}
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

/** 占位符里那条示例是**能直接改的真句子**，不是「请输入条件」这种废话。 */
const WHERE_PLACEHOLDER = "D_BIZ >= DATE '2026-08-01'\n  AND STATUS IN ('OK', 'WARN')";

/**
 * 过滤条件：**一个自由 WHERE 文本框**，写什么就原样拼进 `WHERE` 后面。
 *
 * 取代的是「字段 + 三个比较符（`>` `<` `=`，连 `>=` 都没有）+ 值」那张四格表单。
 * 用这门平台的人本来就写 SQL；那张表单表达不了他们真正要的条件（`>=`、`IN`、
 * `BETWEEN`、`LIKE`、`OR`、子查询），只会把人逼去绕道自定义 SQL——而自定义 SQL
 * 连表都得自己写。文本框反而是**更小**的东西：只需要写条件，表还是选出来的。
 *
 * 高亮与自定义 SQL 那格共用同一份词法（`tokenize`），不引第三方编辑器。
 * 列清单摆在下面作参考——**不做补全**：补全要接管键盘，而这一格短到不值当。
 */
function WhereClauseEditor({
  value,
  columnNames,
  onChange,
}: {
  value: string;
  columnNames: string[];
  onChange: (next: string) => void;
}) {
  return (
    <section className="spec-editor" aria-labelledby="conditions-title">
      <header>
        <div>
          <strong id="conditions-title">过滤条件</strong>
          <span>留空就是整表取数；写什么就原样拼进 WHERE 后面</span>
        </div>
      </header>
      <div className="where-clause-editor">
        <HighlightedSqlInput
          value={value}
          placeholder={WHERE_PLACEHOLDER}
          label="过滤条件（WHERE 之后的部分）"
          rows={4}
          onChange={onChange}
        />
        <small className="spec-note">
          只写 <span className="mono">WHERE</span> 之后的部分，不用写{" "}
          <span className="mono">WHERE</span> 这个词，也不要写分号。
          {columnNames.length > 0 && (
            <>
              {" "}本表可用的列：
              <span className="mono">{columnNames.join(", ")}</span>
            </>
          )}
        </small>
      </div>
    </section>
  );
}

/**
 * 构建 SQL 的只读展示。
 *
 * **没有编辑入口**：SQL 是规格的派生面，现算现看，不存进任务定义，
 * 也没有把它传回后端的路。这里连 `textarea` 都不给——给了就是在暗示可以改。
 */
function GeneratedSql({
  sql,
  error,
  ready,
  mode,
}: {
  sql: BuilderSql | null;
  error: string | null;
  ready: boolean;
  mode: SourceQueryMode;
}) {
  return (
    <section className="generated-sql" aria-labelledby="generated-sql-title">
      <header>
        <div>
          <strong id="generated-sql-title">构建 SQL</strong>
          <span>
            {mode === "sql"
              ? "只读预览——实际执行的是这一段：你写的 SQL 外面套了一层只取勾选列的投影"
              : "只读预览"}
          </span>
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
            : mode === "sql"
              ? "先读取结果列，再选目标表完成映射与主键。"
              : "先选源表和源列，再读取目标列完成映射与主键。"}
        </p>
      ) : (
        <pre className="ddl-output">{sql.source_sql}</pre>
      )}
    </section>
  );
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
  onClearMapping,
}: {
  state: TargetMetaState;
  spec: TaskSpec;
  selectedSources: string[];
  onReload: () => void;
  onTargetChange: (source: string, target: string) => void;
  onToggleKey: (source: string) => void;
  onFillSameName: () => void;
  onClearMapping: () => void;
}) {
  // 结构表默认收起。展开态是**用户的会话选择**，不进任务定义——它和目标表结构本身
  // 一样是瞬态的（ADR-0038 §8）。
  const [structureOpen, setStructureOpen] = useState(false);
  if (state.kind === "idle") {
    return null;
  }
  const retry = (
    <button
      className="button is-ghost"
      type="button"
      onClick={onReload}
      disabled={state.kind === "loading"}
    >
      {state.kind === "loading" ? "读取中" : "重新读取"}
    </button>
  );
  if (state.kind === "loading") {
    return (
      <div className="loading-state" aria-live="polite">
        正在读取目标表列...
      </div>
    );
  }
  if (state.kind === "failed") {
    return (
      <div className="target-meta-retry">
        <div className="form-error" role="alert">
          {state.message}
        </div>
        {retry}
      </div>
    );
  }
  if (state.data.columns.length === 0) {
    return (
      <div className="target-meta-retry">
        <p className="column-fetch-hint">
          目标库中没有 <code>{state.table}</code> 这张表。请在目标库中建好这张表，
          然后点「重新读取」。
        </p>
        {retry}
      </div>
    );
  }
  const targetMeta = state.data;
  return (
    <>
      <FieldMappingEditor
        spec={spec}
        selectedSources={selectedSources}
        targetMeta={targetMeta}
        onTargetChange={onTargetChange}
        onToggleKey={onToggleKey}
        onFillSameName={onFillSameName}
        onClearMapping={onClearMapping}
      />
      {/* 原来这张只读表和「字段映射」是父子关系（目标表 > 目标表列参考 > 字段映射），
          三层卡片描述的是同一批列，占掉近半个弹窗的高度。改成与映射表**平级**、
          默认收起：它是查证用的参考，不是每次建任务都要读完的东西。 */}
      <section
        className={`target-structure ${structureOpen ? "is-open" : ""}`}
        aria-labelledby="target-structure-title"
      >
        <header>
          <button
            className="structure-toggle"
            type="button"
            aria-expanded={structureOpen}
            onClick={() => setStructureOpen((open) => !open)}
          >
            {structureOpen ? (
              <ChevronDown size={14} aria-hidden="true" />
            ) : (
              <ChevronRight size={14} aria-hidden="true" />
            )}
            <strong id="target-structure-title">目标表结构</strong>
            <span>
              {targetMeta.columns.length} 列 · 只读参考，不写入任务定义
            </span>
          </button>
          {retry}
        </header>
        {structureOpen && (
          <>
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
                  {targetMeta.columns.map((column) => {
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
                          {constraintsOf(targetMeta, column) || "—"}
                        </td>
                        <td className="mono">{source ?? "（未映射）"}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
            <footer>
              <span className="builder-key-note">
                长度栏同为字符；映射预检按字节判。
              </span>
            </footer>
          </>
        )}
      </section>
    </>
  );
}

function FieldMappingEditor({
  spec,
  selectedSources,
  targetMeta,
  onTargetChange,
  onToggleKey,
  onFillSameName,
  onClearMapping,
}: {
  spec: TaskSpec;
  selectedSources: string[];
  targetMeta: TargetTableMetadata;
  onTargetChange: (source: string, target: string) => void;
  onToggleKey: (source: string) => void;
  onFillSameName: () => void;
  onClearMapping: () => void;
}) {
  const targetColumns = targetMeta.columns;
  const targetByUpper = new Map(
    targetColumns.map((column) => [column.name.toUpperCase(), column]),
  );
  const mappedCount = spec.columns.filter(
    (mapping) => mapping.target.trim() !== "",
  ).length;
  const pendingCount = spec.columns.length - mappedCount;

  return (
    <section className="field-mapping-section" aria-labelledby="field-mapping-title">
      <header>
        <div>
          <strong id="field-mapping-title">字段映射</strong>
          {/* 同名列在读取目标列时就已经自动接上了。这一行说的是「机器做到了多少、
              还剩几个要你决定」，而不是笼统的操作说明。 */}
          <span>
            {selectedSources.length === 0
              ? "读取目标列后再把源列绑定到目标列"
              : pendingCount === 0
                ? `${mappedCount} 列已映射，无需确认`
                : `已自动匹配 ${mappedCount}/${spec.columns.length}，${pendingCount} 个待确认`}
          </span>
        </div>
        <div className="field-mapping-actions">
          <button
            className="button is-ghost"
            type="button"
            onClick={onFillSameName}
            disabled={selectedSources.length === 0 || targetColumns.length === 0}
          >
            同名填充
          </button>
          <button
            className="button is-ghost"
            type="button"
            onClick={onClearMapping}
            disabled={mappedCount === 0}
          >
            清空映射
          </button>
        </div>
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
