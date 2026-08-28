import {
  Ban,
  CalendarClock,
  Clock3,
  Database,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { ICON } from "./components/DesignSystem";

import { deleteTask, fetchScheduleState, startRun, taskInputFrom, updateTask } from "./api";
import type { Datasource, QueuedOccurrence, RunHistory, Task } from "./api";
import { messageFrom } from "./errors";
import { qualifiedTargetTable } from "./datasource";
import { formatTimestamp, historyPresentation } from "./history";
import {
  datasourceFilterOptions,
  DEFAULT_PAGE_SIZE,
  EMPTY_TASK_FILTERS,
  LATEST_RUN_LABELS,
  LATEST_RUN_ORDER,
  latestRunStatus,
  paginate,
  pageContainingTask,
  taskMatchesFilters,
} from "./listing";
import type { LatestRunStatus, TaskFilters } from "./listing";
import { progressOf } from "./progress";
import { runHash } from "./routes";
import { RunDrawer } from "./RunDrawer";
import { ScheduleCard } from "./ScheduleCard";
import { sourceSummary } from "./spec";
import { rowRunAction } from "./troubleshooting";
import type { Step } from "./wizard";
import { ActionButton, Modal, ModalFooter, Pagination } from "./ui";

/**
 * 作业中心——**任务列表与运行历史合成的那一屏**（ADR-0043 §2）。
 *
 * 一行 = **一个任务 + 它最近一次运行**。这条合并是所有者 2026-08-21 裁定 2 的直接后果：
 * 「运行历史」独立屏取消，同一个任务的多次历史本版不做（原话「另说」）。
 *
 * 列序（2026-08 UX 评审 P1-1 改，原裁定 3 的顺序作废）：**☐ · 任务名 · 运行状态 ·
 * 迁移进度 · 目标表 · 源表 · 启动时间 · 运行时长 · 操作**。主键 / 条件 / 错误码 /
 * 目标表效果**一个都不在这张表上**——它们是任务属性或三轴的东西，收进详情抽屉（`RunDrawer`）。
 *
 * 改序的理由只有一条：**这一屏存在的理由是「跑了没有、跑成什么样」**。原来它排在第 5、6 列，
 * 1440 下要横滚 507px 才看得见，而占着最前面两格的是源表与目标表——任务属性，一天看一次
 * 就够，一次运行也不会变。现在前四格答完「哪个任务 / 跑成什么样 / 到哪儿了 / 写进哪张表」，
 * 后面才是它的定义。
 *
 * 「运行状态」列是**一维索引，不是轴二**：五个词都是同一种实心方角标签，齐是对的。
 * 轴二 / 轴三整体在抽屉里，形状一个没变（ADR-0043 §4，走查 X17）。
 */
export interface JobCenterProps {
  tasks: Task[] | null;
  datasources: Datasource[];
  /** 每个任务的**最近一次**运行；判定在 `listing.ts`，本屏不自己认哪条是最近的。 */
  latestRuns: ReadonlyMap<string, RunHistory>;
  refreshing: boolean;
  onRefresh: () => void;
  onCreate: () => void;
  onEdit: (task: Task) => void;
  onDelete: (task: Task) => void;
  /** 正在发起的那个任务的 id——**只有它那一行**的发起键在这段时间里按不动。 */
  startingTaskId: string | null;
  onStart: (task: Task) => void;
  onStop: (runRecordId: string) => void;
  /** 重跑就是按这个任务当前的定义再跑一次；上一次那条记录不再带进来（没有可预填的东西）。 */
  onRerun: (task: Task) => void;
  onEditFailure: (task: Task, step: Step) => void;
  /** 批量删除跑完之后要重读清单——本屏不改 `App` 的 state。 */
  onChanged: () => void;
  focusTaskId: string | null;
  onFocusConsumed: () => void;
}

export function JobCenterScreen({
  tasks,
  datasources,
  latestRuns,
  refreshing,
  onRefresh,
  onCreate,
  onEdit,
  onDelete,
  startingTaskId,
  onStart,
  onStop,
  onRerun,
  onEditFailure,
  onChanged,
  focusTaskId,
  onFocusConsumed,
}: JobCenterProps) {
  // 筛选条上**正在填**的那一组与**已生效**的那一组分开存：查询是显式的，
  // 改一下下拉不重筛（ADR-0042 §1 的既有裁定，走查 X10）。
  const [draft, setDraft] = useState<TaskFilters>(EMPTY_TASK_FILTERS);
  const [filters, setFilters] = useState<TaskFilters>(EMPTY_TASK_FILTERS);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(DEFAULT_PAGE_SIZE);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [openTaskId, setOpenTaskId] = useState<string | null>(null);
  /**
   * 哪一个批量动作正等着二次确认。**发起和删除共用这一格**：两者互斥
   * （同一批勾选，同一颗按钮群），分成两个布尔会多出「两个框同时开着」这种不可能态。
   */
  const [bulkConfirm, setBulkConfirm] = useState<"start" | "delete" | null>(null);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [bulkSummary, setBulkSummary] = useState<BulkSummary | null>(null);
  /** 正在改调度的那个任务，`null` 是没开对话框。 */
  const [schedulingTaskId, setSchedulingTaskId] = useState<string | null>(null);
  const focusedRow = useRef<HTMLTableRowElement | null>(null);
  const queued = useQueuedOccurrences();

  useEffect(() => {
    if (focusTaskId === null || !(tasks ?? []).some((task) => task.task_id === focusTaskId)) {
      return;
    }
    setDraft(EMPTY_TASK_FILTERS);
    setFilters(EMPTY_TASK_FILTERS);
    setPage(pageContainingTask(tasks ?? [], focusTaskId, pageSize));
  }, [focusTaskId, pageSize, tasks]);

  const filtered = useMemo(
    () =>
      (tasks ?? []).filter((task) =>
        taskMatchesFilters(
          task,
          filters,
          latestRunStatus(latestRuns.get(task.task_id)),
        ),
      ),
    [filters, latestRuns, tasks],
  );
  const slice = paginate(filtered, page, pageSize);
  const sourceOptions = useMemo(
    () => datasourceFilterOptions(datasources, tasks ?? [], "source"),
    [datasources, tasks],
  );
  const targetOptions = useMemo(
    () => datasourceFilterOptions(datasources, tasks ?? [], "target"),
    [datasources, tasks],
  );
  const selectedTasks = (tasks ?? []).filter((task) =>
    selected.has(task.task_id),
  );
  const openTask =
    openTaskId === null
      ? undefined
      : (tasks ?? []).find((task) => task.task_id === openTaskId);
  const openRun = openTaskId === null ? undefined : latestRuns.get(openTaskId);
  const schedulingTask =
    schedulingTaskId === null
      ? undefined
      : (tasks ?? []).find((task) => task.task_id === schedulingTaskId);

  /** 「查询」/「重置」共用一条路：生效的那一组换掉，页码回第 1 页。 */
  function applyFilters(next: TaskFilters) {
    setDraft(next);
    setFilters(next);
    setPage(1);
  }

  function toggleOne(taskId: string, checked: boolean) {
    setSelected((current) => {
      const next = new Set(current);
      if (checked) {
        next.add(taskId);
      } else {
        next.delete(taskId);
      }
      return next;
    });
  }

  /**
   * 表头全选**只全选当前页**（ADR-0043 §6，走查 X15）。
   *
   * 跨页全选会让人在**看不见的行**上执行动作：勾一下表头就把第 2、3 页那些他压根没看过的
   * 任务一起删了。取消也只取消当前页，语义才对得上。
   */
  function togglePage(checked: boolean) {
    setSelected((current) => {
      const next = new Set(current);
      for (const task of slice.rows) {
        if (checked) {
          next.add(task.task_id);
        } else {
          next.delete(task.task_id);
        }
      }
      return next;
    });
  }

  /**
   * 批量发起：**串行**，一条失败不中断后面的，跑完汇总一行（ADR-0043 §6）。
   *
   * 串行而非并发，是因为发起会真的打两端的库；并发五条等于同时开五个 Oracle 游标。
   * 不加后端批量端点：那要定部分失败的语义与原子性，是它自己一票的体量，
   * 而现场规模（几十个任务）下逐条调用的代价可以忽略。
   *
   * **没有「这条任务要先填参数」这一支了**：发起的全部输入就是任务身份，
   * 所以批量发起与单条发起打的是同一个端点、走的是同一条路。
   *
   * 进来之前先过 `BulkStartDialog` 的二次确认（2026-08 UX 评审 P0-2）。
   */
  async function runBulkStart() {
    setBulkBusy(true);
    const failures: BulkFailure[] = [];
    let ok = 0;
    for (const task of selectedTasks) {
      try {
        await startRun(task.task_id);
        ok += 1;
      } catch (error) {
        failures.push({ name: task.name, reason: messageFrom(error) });
      }
    }
    setBulkBusy(false);
    setBulkConfirm(null);
    setBulkSummary({ verb: "发起", total: selectedTasks.length, ok, failures });
    onChanged();
  }

  /** 批量删除：同样串行、同样汇总；确认框在 `BulkDeleteDialog` 里把名字逐条列全。 */
  async function runBulkDelete() {
    setBulkBusy(true);
    const failures: BulkFailure[] = [];
    let ok = 0;
    for (const task of selectedTasks) {
      try {
        await deleteTask(task.task_id);
        ok += 1;
      } catch (error) {
        failures.push({ name: task.name, reason: messageFrom(error) });
      }
    }
    setBulkBusy(false);
    setBulkConfirm(null);
    setSelected(new Set());
    setBulkSummary({ verb: "删除", total: selectedTasks.length, ok, failures });
    onChanged();
  }

  const hasTasks = tasks !== null && tasks.length > 0;
  const pageIds = slice.rows.map((task) => task.task_id);
  const allOnPageSelected =
    pageIds.length > 0 && pageIds.every((id) => selected.has(id));
  const focusedOnPage =
    focusTaskId !== null && slice.rows.some((task) => task.task_id === focusTaskId);

  useEffect(() => {
    if (!focusedOnPage) {
      return;
    }
    const frame = window.requestAnimationFrame(() => {
      focusedRow.current?.scrollIntoView({ block: "center", behavior: "smooth" });
    });
    const timeout = window.setTimeout(onFocusConsumed, 1800);
    return () => {
      window.cancelAnimationFrame(frame);
      window.clearTimeout(timeout);
    };
  }, [focusTaskId, focusedOnPage, onFocusConsumed]);

  return (
    <>
      {hasTasks && (
        <div className="filter-card">
          <label className="filter-field is-wide">
            <span>任务名</span>
            <input
              value={draft.keyword}
              placeholder="任务名 / 源表 / 目标表"
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  keyword: event.target.value,
                }))
              }
            />
          </label>
          <label className="filter-field">
            <span>源端</span>
            <select
              value={draft.sourceDatasourceId}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  sourceDatasourceId: event.target.value,
                }))
              }
            >
              <option value="">全部源端</option>
              {sourceOptions.map(([id, name]) => (
                <option key={id} value={id}>
                  {name}
                </option>
              ))}
            </select>
          </label>
          <label className="filter-field">
            <span>目标端</span>
            <select
              value={draft.targetDatasourceId}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  targetDatasourceId: event.target.value,
                }))
              }
            >
              <option value="">全部目标端</option>
              {targetOptions.map(([id, name]) => (
                <option key={id} value={id}>
                  {name}
                </option>
              ))}
            </select>
          </label>
          <label className="filter-field">
            <span>运行状态</span>
            <select
              value={draft.latestStatus}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  latestStatus: event.target.value as LatestRunStatus | "",
                }))
              }
            >
              <option value="">全部</option>
              {LATEST_RUN_ORDER.map((status) => (
                <option key={status} value={status}>
                  {LATEST_RUN_LABELS[status]}
                </option>
              ))}
            </select>
          </label>
          <span className="filter-actions">
            <button
              className="button is-ghost"
              type="button"
              onClick={() => applyFilters(EMPTY_TASK_FILTERS)}
            >
              重置
            </button>
            <button
              className="button is-primary"
              type="button"
              onClick={() => applyFilters(draft)}
            >
              查询
            </button>
          </span>
        </div>
      )}

      {bulkSummary !== null && (
        <div
          className={`bulk-summary ${bulkSummary.failures.length > 0 ? "is-failed" : ""}`}
          role="status"
        >
          <span>{bulkSummaryText(bulkSummary)}</span>
          <button
            className="text-button"
            type="button"
            onClick={() => setBulkSummary(null)}
          >
            知道了
          </button>
        </div>
      )}

      <section className="card table-card" id="jobs" aria-labelledby="jobs-title">
        {/* 与数据源屏、Agent 屏**同一个卡头**（UX 评审 P2）：这里原来是另一套
            （`.table-title-row` + `.table-count`），两套只在结构上不同，长得却几乎一样——
            于是三屏的卡头高度、内边距、标题与副标题的间距各差一点，谁也说不清哪个是对的。
            那块灰底标题是照参照物量的，留着，但改成三屏共有。 */}
        <header className="card-header">
          <div>
            <h1 id="jobs-title">
              作业中心
            </h1>
            <span className="card-subtitle">
              {countLabel(tasks, filtered, refreshing)}
            </span>
          </div>
          <div className="table-toolbar">
            <span className="toolbar-icons">
              <button
                className="icon-button is-tool"
                type="button"
                title="刷新"
                aria-label="刷新"
                disabled={refreshing}
                onClick={onRefresh}
              >
                <RefreshCw
                  className={refreshing ? "is-spinning" : ""}
                  size={ICON.md}
                  aria-hidden="true"
                />
              </button>
            </span>
            <button className="button is-primary" type="button" onClick={onCreate}>
              <Plus size={ICON.sm} aria-hidden="true" />
              新建任务
            </button>
            {/* 未选中时**禁用**，不是能点了才报错（ADR-0043 §6，走查 X15）。 */}
            <button
              className="button"
              type="button"
              disabled={selectedTasks.length === 0 || bulkBusy}
              onClick={() => setBulkConfirm("start")}
            >
              {bulkBusy ? "正在发起" : "批量发起"}
            </button>
            {/* 只是打开确认框，此刻一行都还没删——所以是最轻的那一档
                （2026-08 UX 评审 P0-2.4）。落锤那颗在 `BulkDeleteDialog` 里。 */}
            <button
              className="button is-danger is-ghost"
              type="button"
              disabled={selectedTasks.length === 0 || bulkBusy}
              onClick={() => setBulkConfirm("delete")}
            >
              批量删除
            </button>
          </div>
        </header>

        <JobResults
          queued={queued}
          tasks={tasks}
          filtered={filtered}
          rows={slice.rows}
          datasources={datasources}
          latestRuns={latestRuns}
          refreshing={refreshing}
          selected={selected}
          allOnPageSelected={allOnPageSelected}
          onToggleOne={toggleOne}
          onTogglePage={togglePage}
          onCreate={onCreate}
          onEdit={onEdit}
          onSchedule={(task) => setSchedulingTaskId(task.task_id)}
          onDelete={onDelete}
          startingTaskId={startingTaskId}
          onStart={onStart}
          onStop={onStop}
          onOpen={setOpenTaskId}
          onClearFilters={() => applyFilters(EMPTY_TASK_FILTERS)}
          focusTaskId={focusTaskId}
          focusedRow={focusedRow}
        />

        {hasTasks && (
          <Pagination
            page={slice.page}
            pageCount={slice.pageCount}
            total={slice.total}
            pageSize={slice.pageSize}
            unit="个"
            onPage={setPage}
            onPageSize={(size) => {
              setPageSize(size);
              // 换每页条数回第 1 页：留在第 7 页而每页从 20 变 100，
              // 落点是一屏跟刚才毫无关系的行（走查 X11）。
              setPage(1);
            }}
          />
        )}
      </section>

      {openTask !== undefined && openRun !== undefined && (
        <RunDrawer
          task={openTask}
          run={openRun}
          tasks={tasks}
          onClose={() => setOpenTaskId(null)}
          onRerun={(task) => {
            setOpenTaskId(null);
            onRerun(task);
          }}
          onEditTask={(task, step) => {
            setOpenTaskId(null);
            onEditFailure(task, step);
          }}
        />
      )}

      {schedulingTask !== undefined && (
        <ScheduleDialog
          task={schedulingTask}
          onClose={() => setSchedulingTaskId(null)}
          onSaved={() => {
            setSchedulingTaskId(null);
            onChanged();
          }}
        />
      )}

      {bulkConfirm === "start" && (
        <BulkStartDialog
          tasks={selectedTasks}
          datasources={datasources}
          busy={bulkBusy}
          onClose={() => setBulkConfirm(null)}
          onConfirm={() => void runBulkStart()}
        />
      )}

      {bulkConfirm === "delete" && (
        <BulkDeleteDialog
          tasks={selectedTasks}
          busy={bulkBusy}
          onClose={() => setBulkConfirm(null)}
          onConfirm={() => void runBulkDelete()}
        />
      )}
    </>
  );
}

/**
 * 那颗定时任务按钮上写什么。
 *
 * **三档，不是两档**：没配表达式、配了但停用、配了且在跑。中间那一档是这个产品
 * 刻意留的一档（`schedule_enabled` 与 `schedule_cron` 是两个字段：暂停不该逼人删掉
 * 自己写好的那一行），按钮上把它和「压根没配」混成同一句话，等于把那个设计抹掉。
 */
function scheduleLabel(task: Task): string {
  const cron = (task.spec.schedule_cron ?? "").trim();
  if (cron === "") {
    return "定时任务：未配置";
  }
  return task.spec.schedule_enabled ? `定时任务：${cron}` : `定时任务：已停用（${cron}）`;
}

/**
 * 在清单上直接改这条任务的调度（#265）。
 *
 * 为什么不让人走编辑向导：向导是**整份任务定义**的编辑器，四步走完才谈得上保存，
 * 而目标表一漂第 3 步就过不去——「今晚先把这个定时停掉」于是变成一件做不到的事。
 * 调度是这一屏上人会反复动的开关，它该有自己的那一颗按钮。
 *
 * 保存送的是任务当前的定义 + 改动过的这两个字段，别的一个字不动。
 */
function ScheduleDialog({
  task,
  onClose,
  onSaved,
}: {
  task: Task;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [cron, setCron] = useState(task.spec.schedule_cron ?? "");
  const [enabled, setEnabled] = useState(task.spec.schedule_enabled);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save() {
    setBusy(true);
    setError(null);
    try {
      // 原文照送（前端不重写一遍解析器），非法表达式由服务端当面拒，
      // 那句话与向导里看到的是同一句。
      await updateTask(
        task.task_id,
        taskInputFrom(task, {
          spec: { ...task.spec, schedule_cron: cron.trim(), schedule_enabled: enabled },
        }),
      );
      onSaved();
    } catch (failure) {
      setError(messageFrom(failure));
      setBusy(false);
    }
  }

  return (
    <Modal title={`定时任务 · ${task.name}`} onClose={onClose} busy={busy} narrow>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <div className="modal-body">
          <ScheduleCard cron={cron} enabled={enabled} onCron={setCron} onEnabled={setEnabled} />
          {error !== null && (
            <p className="form-error" role="alert">
              {error}
            </p>
          )}
        </div>
        <ModalFooter onClose={onClose} busy={busy} submitLabel="保存" />
      </form>
    </Modal>
  );
}

/**
 * 卡内那句计数。**筛出来的与总共有的不同时才写两个数**——
 * 没筛的时候写「筛出 12 / 共 12 个」是在制造一个不存在的区别。
 */
function countLabel(
  tasks: Task[] | null,
  filtered: Task[],
  refreshing: boolean,
): string {
  if (tasks !== null) {
    return filtered.length === tasks.length
      ? `共 ${tasks.length} 个`
      : `筛出 ${filtered.length} / 共 ${tasks.length} 个`;
  }
  return refreshing ? "正在读取" : "暂不可用";
}

interface BulkFailure {
  name: string;
  reason: string;
}

interface BulkSummary {
  verb: string;
  total: number;
  ok: number;
  failures: BulkFailure[];
}

/**
 * 汇总一行：`发起 5 个：成功 4，失败 1（交易流水：目标端不可达）`。
 *
 * **不是只报最后一条**——串行跑完之后，哪几个没成、各自为什么，是这次操作唯一的交代。
 * 失败超过三条时只点名前三个再写「等 N 个」：一行字装不下十条理由，而前三条足够看出是不是同一类毛病。
 */
function bulkSummaryText(summary: BulkSummary): string {
  const head = `${summary.verb} ${summary.total} 个：成功 ${summary.ok}`;
  if (summary.failures.length === 0) {
    return `${head}。`;
  }
  const named = summary.failures
    .slice(0, 3)
    .map((failure) => `${failure.name}：${failure.reason}`)
    .join("；");
  const rest =
    summary.failures.length > 3 ? `，等 ${summary.failures.length} 个` : "";
  return `${head}，失败 ${summary.failures.length}（${named}${rest}）。`;
}

function JobResults({
  tasks,
  filtered,
  rows,
  datasources,
  latestRuns,
  queued,
  refreshing,
  selected,
  allOnPageSelected,
  onToggleOne,
  onTogglePage,
  onCreate,
  onEdit,
  onSchedule,
  onDelete,
  startingTaskId,
  onStart,
  onStop,
  onOpen,
  onClearFilters,
  focusTaskId,
  focusedRow,
}: {
  tasks: Task[] | null;
  filtered: Task[];
  rows: Task[];
  datasources: Datasource[];
  latestRuns: ReadonlyMap<string, RunHistory>;
  queued: ReadonlyMap<string, QueuedOccurrence>;
  refreshing: boolean;
  selected: ReadonlySet<string>;
  allOnPageSelected: boolean;
  onToggleOne: (taskId: string, checked: boolean) => void;
  onTogglePage: (checked: boolean) => void;
  onCreate: () => void;
  onEdit: (task: Task) => void;
  onSchedule: (task: Task) => void;
  onDelete: (task: Task) => void;
  startingTaskId: string | null;
  onStart: (task: Task) => void;
  onStop: (runRecordId: string) => void;
  onOpen: (taskId: string) => void;
  onClearFilters: () => void;
  focusTaskId: string | null;
  focusedRow: React.RefObject<HTMLTableRowElement | null>;
}) {
  if (tasks === null) {
    return (
      <div className="loading-state" aria-live="polite">
        {refreshing ? "正在加载作业中心..." : "作业中心暂不可用"}
      </div>
    );
  }
  if (tasks.length === 0) {
    return (
      <div className="empty-state">
        <div className="empty-icon">
          <Database size={ICON.empty} aria-hidden="true" />
        </div>
        <h2>还没有任务</h2>
        <p>新建第一个 Oracle → MySQL 导入任务。</p>
        <button className="button is-primary" type="button" onClick={onCreate}>
          <Plus size={ICON.sm} aria-hidden="true" />
          新建任务
        </button>
      </div>
    );
  }
  if (filtered.length === 0) {
    // 空表格加一个孤零零的分页条不算回答（走查 X10）。
    // **但也不能只说「没有」**（UX 评审 P2）：筛出零条的时候，人下一步一定是想把筛选
    // 去掉，而唯一的路是回到上面那条筛选栏逐个还原。这里直接给一颗。
    return (
      <div className="no-results">
        <span>没有匹配的任务</span>
        <button className="text-button" type="button" onClick={onClearFilters}>
          清除筛选
        </button>
      </div>
    );
  }

  // 源 / 目标那两行下挂的是**数据源名字**，`datasource_id` 只在数据源屏出现（ADR-0039 §8）。
  const names = new Map(
    datasources.map((datasource) => [datasource.datasource_id, datasource.name]),
  );
  const nameOf = (datasourceId: string) =>
    names.get(datasourceId) ?? (datasourceId === "" ? "—" : datasourceId);

  return (
    <div className="table-wrap">
      <table className="data-grid job-grid">
        <thead>
          <tr>
            <th className="check-column">
              <input
                type="checkbox"
                title="全选当前页"
                aria-label="全选当前页"
                checked={allOnPageSelected}
                onChange={(event) => onTogglePage(event.target.checked)}
              />
            </th>
            <th>任务名</th>
            <th>
              运行状态
              <span className="visually-hidden">
                有进行中任务时会自动刷新。
              </span>
            </th>
            <th>迁移进度</th>
            <th>目标表</th>
            <th>源表</th>
            <th>启动时间</th>
            <th>运行时长</th>
            <th className="action-column">操作</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((task) => {
            const run = latestRuns.get(task.task_id);
            const status = latestRunStatus(run);
            const progress = progressOf(run);
            const source = sourceSummary(task.spec);
            const runAction = rowRunAction(
              run,
              startingTaskId === task.task_id,
            );
            return (
              <tr
                className={task.task_id === focusTaskId ? "is-new-task" : undefined}
                key={task.task_id}
                ref={task.task_id === focusTaskId ? focusedRow : undefined}
              >
                <td className="check-column">
                  <input
                    type="checkbox"
                    title={`选中 ${task.name}`}
                    aria-label={`选中 ${task.name}`}
                    checked={selected.has(task.task_id)}
                    onChange={(event) =>
                      onToggleOne(task.task_id, event.target.checked)
                    }
                  />
                </td>
                <td>
                  {/* `task_id` 不再占第二行（2026-08 UX 评审 P1-1）：每一行都摆一串
                      32 位十六进制，是把**九列里最没人读的那一样**放进了每一行的视线
                      正中。它没消失，挂在任务名的 title 上，要用的时候悬停一下就有。 */}
                  <span className="task-name" title={`任务 ID ${task.task_id}`}>
                    {task.name}
                  </span>
                  {/* 写入方式那两枚标记（「纯追加写」/「先清空再导入」）撤了：它们是
                      任务**属性**，一天看一次就够，却在每一行的任务名后面各占一块，
                      清单最该一眼扫过的那一列因此再没法一眼扫过。两句话都还在——
                      编辑向导第 1 步那格写入方式、运行详情里那句「这一次做了什么」，
                      都是在人正要做决定或正在追责的那一刻说的。 */}
                  {/* 到点了但还没派出去的那些（#266）。它是一个**状态**不是一项属性，
                      所以标记里带上那一刻，`title` 上写清楚它在等什么——队列活在
                      服务端一条后台线程里，不显示出来，屏幕上就只剩「什么都没发生」。 */}
                  {queued.has(task.task_id) && (
                    <span
                      className="write-mark is-queued"
                      title={queuedTitle(queued.get(task.task_id)!)}
                    >
                      排队中
                    </span>
                  )}
                </td>
                <td>
                  {/* 状态是**一条链接**（UX 评审 P1-6）：运行详情整屏现在有地址了，
                      而这一格正是人看完状态之后想点进去的那一格。旁边那颗时钟图标
                      照旧开抽屉——快速看一眼与摊开细看是两件事。 */}
                  {run === undefined ? (
                    <span className={`state is-${status}`}>
                      {LATEST_RUN_LABELS[status]}
                    </span>
                  ) : (
                    <a
                      className={`state is-${status}`}
                      href={runHash(run.run_record_id)}
                      title={conclusionOf(run)}
                    >
                      {LATEST_RUN_LABELS[status]}
                    </a>
                  )}
                </td>
                <td>
                  {progress.kind === "value" ? (
                    <span className="progress" title={progress.title}>
                      <span className="progress-track">
                        <span
                          className={`progress-fill is-${progress.tone}`}
                          style={{ width: `${progress.percent}%` }}
                        />
                      </span>
                      <span className="progress-pct">{progress.label}</span>
                    </span>
                  ) : (
                    <span className="empty-value" title={progress.title}>
                      {progress.label}
                    </span>
                  )}
                </td>
                <td>
                  <span className="table-cell" title={task.spec.target_table}>
                    {task.spec.target_table}
                  </span>
                  <span className="table-side">
                    {nameOf(task.target_datasource_id)}
                  </span>
                </td>
                <td>
                  {/* 自定义 SQL 的任务这里**只给一枚徽标**，不给语句片段：截断到一行的
                      SQL 认不出是哪一条（几十个任务的开头都是 `SELECT a.ID AS ID,`），
                      却要吃掉这一列大半的宽度。全文照旧在 title 上，完整语句在详情里。 */}
                  <span className="table-cell" title={source.full}>
                    {source.kind === "sql" ? (
                      <span className="source-kind">自定义 SQL</span>
                    ) : (
                      source.label
                    )}
                  </span>
                  <span className="table-side">
                    {nameOf(task.source_datasource_id)}
                  </span>
                </td>
                <td className="time-cell">
                  {run === undefined ? (
                    <span className="empty-value">—</span>
                  ) : (
                    formatTimestamp(run.started_at, true)
                  )}
                </td>
                <td className="time-cell">
                  {elapsedLabel(run) ?? <span className="empty-value">—</span>}
                </td>
                <td className="action-column">
                  <span className="row-actions">
                    {/* 发起与停止共用这一格：进行中只给停止，终局或从未运行只给发起。
                        发起请求在途时只锁这一行，别的任务仍然可以操作。 */}
                    {runAction.kind === "start" ? (
                      <ActionButton
                        label={runAction.disabled ? "正在发起" : "发起运行"}
                        icon={<Play size={ICON.md} />}
                        disabled={runAction.disabled}
                        onClick={() => onStart(task)}
                      />
                    ) : (
                      // 停不停得了与运行详情屏读同一条规则（UX 评审 P1-11）：
                      // 这一颗过去无条件亮着，人只有吃一个 409 才知道封口点已经过了。
                      <ActionButton
                        label={
                          runAction.refusal === null
                            ? `停止运行 ${runAction.runRecordId}`
                            : `停止运行（不可用）：${runAction.refusal}`
                        }
                        icon={<Ban size={ICON.md} />}
                        disabled={runAction.refusal !== null}
                        onClick={() => onStop(runAction.runRecordId)}
                      />
                    )}
                    {/* 「复制 cURL」那颗撤了，位子给定时任务：调度是这一屏上人会反复
                        动的开关（今天停一下、明天换个点），而 cURL 一个任务一辈子复制
                        一次。按钮上写着这条任务此刻的调度状态，不必点开才知道。 */}
                    <ActionButton
                      label={scheduleLabel(task)}
                      icon={<CalendarClock size={ICON.md} />}
                      onClick={() => onSchedule(task)}
                    />
                    <span className="divider" />
                    <ActionButton
                      label="编辑任务定义"
                      icon={<Pencil size={ICON.md} />}
                      onClick={() => onEdit(task)}
                    />
                    {/* 运行日志不再单独占一颗：它本来就是运行详情的一段（`RunScreen`
                        里那个 `RunLogPanel`），给它第二个入口只是把同一个地方说成
                        两件事。位子占住不撤（UX 评审 P2-15）——从没跑过的任务这一颗
                        按不动并说明为什么，比凭空少一颗、让整列动作横向错位好。 */}
                    <ActionButton
                      label="查看详情"
                      icon={<Clock3 size={ICON.md} />}
                      disabled={run === undefined}
                      title={run === undefined ? "这个任务还没有跑过" : "运行详情与日志"}
                      onClick={() => onOpen(task.task_id)}
                    />
                    <span className="divider" />
                    <ActionButton
                      label="删除"
                      danger
                      icon={<Trash2 size={ICON.md} />}
                      onClick={() => onDelete(task)}
                    />
                  </span>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** 完整结论挂在状态标签的 `title` 上——列表要窄，但那句人话不该只能开抽屉才看得到。 */
function conclusionOf(run: RunHistory): string {
  return historyPresentation(run).conclusion;
}

/**
 * 运行时长。
 *
 * - 跑完了 → 结束时间减发起时间的**墙钟**时长。
 * - 还在跑 → 到此刻为止已跑了多久；它每次渲染都会变，那正是「还在跑」的实话。
 * - 结局不明 → `null`（界面出 `—`）：不知道它什么时候停的，编一个数出来是撒谎。
 * - 没跑过 → `null`。
 */
function elapsedLabel(run: RunHistory | undefined): string | null {
  if (run === undefined || run.unknown_reason !== null) {
    return null;
  }
  const startedMs = Date.parse(run.started_at);
  if (Number.isNaN(startedMs)) {
    return null;
  }
  const endedMs =
    run.finished_at === null ? Date.now() : Date.parse(run.finished_at);
  if (Number.isNaN(endedMs)) {
    return null;
  }
  return formatDuration(Math.max(0, endedMs - startedMs));
}

function formatDuration(milliseconds: number): string {
  const totalSeconds = Math.floor(milliseconds / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) {
    return `${hours} 小时 ${minutes} 分`;
  }
  if (minutes > 0) {
    return `${minutes} 分 ${seconds} 秒`;
  }
  return `${seconds} 秒`;
}

/**
 * 批量发起的二次确认（2026-08 UX 评审 P0-2）。
 *
 * 单条「发起运行」照旧一按就跑——那是这屏最高频的动作，它当着人的面只动一张表，
 * 而那张表就写在同一行上。**批量不是它的复数**：一颗按钮同时改写 N 张生产表，
 * 而勾是分几页、隔几分钟点下的，按之前根本没有一处地方把「这批到底要写哪几张表」摆齐过。
 * 这层不对称是故意的——摩擦要加在**看不清后果**的那一侧，不是加在每一次动作上。
 *
 * 列的是**任务名与它要写的那张表**，不是任务名加 id：这一步要核对的是「会被改写的是
 * 哪几张表」。目标表带库名（`qualifiedTargetTable`）——同名表在几个库里都可能有一张。
 *
 * 两者之间写「写入」而不是一个箭头：自动生成的任务名**本身**就长成「源表 → 目标表」，
 * 再夹一个箭头，一行里会出现两个方向不同的箭头，读起来像一条三段的链路。
 */
function BulkStartDialog({
  tasks,
  datasources,
  busy,
  onClose,
  onConfirm,
}: {
  tasks: Task[];
  datasources: Datasource[];
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const byId = new Map(datasources.map((datasource) => [datasource.datasource_id, datasource]));
  return (
    <Modal title={`发起 ${tasks.length} 个任务`} onClose={onClose} busy={busy}>
      <div className="modal-body delete-copy">
        <p>
          将<strong>逐个发起</strong>下面这些任务，按主键写进各自的目标表。
          一条失败不中断后面的，跑完给一行汇总。
        </p>
        <ul className="bulk-targets">
          {tasks.map((task) => (
            <li key={task.task_id}>
              <span className="bulk-task-name">{task.name}</span>
              <span className="bulk-task-target">
                写入{" "}
                <span className="mono">
                  {qualifiedTargetTable(byId.get(task.target_datasource_id), task.spec.target_table)}
                </span>
              </span>
            </li>
          ))}
        </ul>
      </div>
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
          type="button"
          onClick={onConfirm}
          disabled={busy}
        >
          {busy ? "正在发起" : `发起 ${tasks.length} 个任务`}
        </button>
      </footer>
    </Modal>
  );
}

/**
 * 批量删除的二次确认。**把要删的任务名逐条列全**（ADR-0043 §6，走查 X15）——
 * 「确定删除 3 个任务？」这句话让人无从核对自己勾中的到底是哪三个。
 */
function BulkDeleteDialog({
  tasks,
  busy,
  onClose,
  onConfirm,
}: {
  tasks: Task[];
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  return (
    <Modal title={`删除 ${tasks.length} 个任务`} onClose={onClose} busy={busy} narrow>
      <div className="modal-body delete-copy">
        <p>
          将逐个删除下面这些任务，<strong>不可撤销</strong>。
          一条失败不中断后面的，跑完给一行汇总。
        </p>
        <ul>
          {tasks.map((task) => (
            <li key={task.task_id}>
              {task.name} · {task.task_id}
            </li>
          ))}
        </ul>
      </div>
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
          className="button is-danger is-solid"
          type="button"
          onClick={onConfirm}
          disabled={busy}
        >
          {busy ? "正在删除" : `删除这 ${tasks.length} 个任务`}
        </button>
      </footer>
    </Modal>
  );
}

/**
 * 调度队列（#266）：到点了、但还没派出去的那些任务。
 *
 * 单独轮询而不是搭在整屏刷新上，理由是**它自己坏掉不该带走任务清单**：这一格是给
 * 现有列表加一枚徽标的，读不到就当成没有人在排队，任务列表照旧。5 秒一次，与服务端
 * 调度器自己的评估间隔同一个数——再快也不会有新答案。
 */
function useQueuedOccurrences(): ReadonlyMap<string, QueuedOccurrence> {
  const [queued, setQueued] = useState<ReadonlyMap<string, QueuedOccurrence>>(
    new Map(),
  );
  useEffect(() => {
    let alive = true;
    async function poll() {
      try {
        const state = await fetchScheduleState();
        if (alive) {
          setQueued(new Map(state.queued.map((row) => [row.task_id, row])));
        }
      } catch {
        // 读不到就维持上一份：闪成空再闪回来比多等 5 秒更难读。
      }
    }
    void poll();
    const timer = window.setInterval(() => void poll(), 5000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);
  return queued;
}

/** 排队徽标的悬停说明：本该什么时候跑、此刻在等什么。 */
export function queuedTitle(occurrence: QueuedOccurrence): string {
  const reason =
    occurrence.waiting_reason.trim() === ""
      ? "已到触发时刻，等待派发"
      : occurrence.waiting_reason;
  return `本该于 ${occurrence.due_at} 触发；${reason}`;
}
