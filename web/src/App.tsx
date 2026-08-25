import {
  Database,
  LoaderCircle,
  Menu,
  PanelLeftClose,
  Radio,
  Server,
  Settings,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent } from "react";

import {
  createTask,
  cancelRun,
  deleteTask,
  listAgents,
  listDatasources,
  listRunHistory,
  listTasks,
  startRun,
  taskInputFrom,
  updateTask,
} from "./api";
import type {
  Agent,
  RunHistory,
  Task,
  Datasource,
  TaskInput,
} from "./api";
import { messageFrom } from "./errors";
import { AgentScreen } from "./AgentScreen";
import { JobCenterScreen } from "./JobCenterScreen";
import { latestRunByTask, runStatus } from "./listing";
import { RunScreen } from "./RunScreen";
import { SettingsScreen } from "./SettingsScreen";
import { DatasourceScreen } from "./DatasourceScreen";
import { evaluateEdit, evaluateEntry, gateFix, gateReason } from "./entry";
import type { EntryFix, EntryGuard } from "./entry";
import { TaskEntryDialog } from "./TaskEntryDialog";
import { TaskWizardScreen } from "./TaskWizardScreen";
import type { TaskWizardScreenHandle } from "./TaskWizardScreen";
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
  const wizardScreenRef = useRef<TaskWizardScreenHandle>(null);
  const pageRef = useRef(page);
  const wizardDraftRef = useRef(wizardDraft);
  const navigationBypass = useRef(false);
  pageRef.current = page;
  wizardDraftRef.current = wizardDraft;
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
      if (navigationBypass.current) {
        navigationBypass.current = false;
        return;
      }
      // 旧的 `#history` 打进来就地换成作业中心的地址：**不留一个还能回去的空屏**。
      if (RETIRED_HISTORY_HASHES.includes(window.location.hash)) {
        window.location.replace("#jobs");
        return;
      }
      const requested = pageFromHash(window.location.hash);
      if (pageRef.current === "wizard" && wizardDraftRef.current !== null && requested !== "wizard") {
        window.history.replaceState(null, "", "#wizard");
        wizardScreenRef.current?.requestLeave(() => commitNavigation(requested));
        return;
      }
      setPage(requested);
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
  const editDatasourceOptions = useMemo(() => {
    if (wizardDraft === null) return { sources: [], targets: [] };
    const guard = evaluateEdit(
      {
        source_datasource_id: wizardDraft.source.datasource_id,
        target_datasource_id: wizardDraft.target.datasource_id,
      },
      datasources,
      agents,
      datasourcesLoading,
    );
    return guard.kind === "open" ? guard : { sources: [], targets: [] };
  }, [agents, datasources, datasourcesLoading, wizardDraft]);

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
    if (pageRef.current === "wizard" && wizardDraftRef.current !== null && nextPage !== "wizard") {
      wizardScreenRef.current?.requestLeave(() => commitNavigation(nextPage));
      return;
    }
    commitNavigation(nextPage);
  }

  function commitNavigation(nextPage: Page) {
    setActiveRun(null);
    if (nextPage !== "wizard") setWizardDraft(null);
    setPage(nextPage);
    if (window.location.hash !== `#${nextPage}`) {
      navigationBypass.current = true;
      window.location.hash = nextPage;
    }
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
      commitNavigation("jobs");
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
    commitNavigation("jobs");
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
              ref={wizardScreenRef}
              initial={wizardDraft}
              onCancel={() => commitNavigation("jobs")}
              onSubmit={handleWizardSubmit}
              sourceOptions={editDatasourceOptions.sources}
              targetOptions={editDatasourceOptions.targets}
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
