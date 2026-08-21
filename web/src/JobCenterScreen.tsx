import {
  Clock3,
  Database,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Tag,
  Trash2,
} from "lucide-react";
import { useMemo, useState } from "react";

import { deleteTask, startRun } from "./api";
import type { Datasource, RunHistory, Task } from "./api";
import { messageFrom } from "./errors";
import { formatTimestamp, historyPresentation } from "./history";
import {
  datasourceFilterOptions,
  DEFAULT_PAGE_SIZE,
  EMPTY_TASK_FILTERS,
  LATEST_RUN_LABELS,
  LATEST_RUN_ORDER,
  latestRunStatus,
  paginate,
  taskMatchesFilters,
} from "./listing";
import type { LatestRunStatus, TaskFilters } from "./listing";
import { progressOf } from "./progress";
import { RunDrawer } from "./RunDrawer";
import { runtimeConditions } from "./spec";
import { ActionButton, Modal, Pagination } from "./ui";

/**
 * 作业中心——**任务列表与运行历史合成的那一屏**（ADR-0043 §2）。
 *
 * 一行 = **一个任务 + 它最近一次运行**。这条合并是所有者 2026-08-21 裁定 2 的直接后果：
 * 「运行历史」独立屏取消，同一个任务的多次历史本版不做（原话「另说」）。
 *
 * 列序逐字（裁定 3）：**☐ · 任务名 · 源表 · 目标表 · 迁移进度 · 运行状态 · 启动时间 ·
 * 运行时长 · 操作**。主键 / 条件 / 错误码 / 目标表效果**一个都不在这张表上**——
 * 它们是任务属性或三轴的东西，收进详情抽屉（`RunDrawer`）。
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
  onRename: (task: Task) => void;
  onDelete: (task: Task) => void;
  onStart: (task: Task) => void;
  onRerun: (task: Task, run: RunHistory) => void;
  /** 批量删除跑完之后要重读清单——本屏不改 `App` 的 state。 */
  onChanged: () => void;
}

export function JobCenterScreen({
  tasks,
  datasources,
  latestRuns,
  refreshing,
  onRefresh,
  onCreate,
  onEdit,
  onRename,
  onDelete,
  onStart,
  onRerun,
  onChanged,
}: JobCenterProps) {
  // 筛选条上**正在填**的那一组与**已生效**的那一组分开存：查询是显式的，
  // 改一下下拉不重筛（ADR-0042 §1 的既有裁定，走查 X10）。
  const [draft, setDraft] = useState<TaskFilters>(EMPTY_TASK_FILTERS);
  const [filters, setFilters] = useState<TaskFilters>(EMPTY_TASK_FILTERS);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(DEFAULT_PAGE_SIZE);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [openTaskId, setOpenTaskId] = useState<string | null>(null);
  const [bulkConfirm, setBulkConfirm] = useState(false);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [bulkSummary, setBulkSummary] = useState<BulkSummary | null>(null);

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
   * **带运行时参数的任务在这里直接判失败**，不去打后端：批量发起没有地方填参数值，
   * 送一个空参数集过去只会换回一句后端的「缺参数」。与其如此，不如当场说清楚该单独发起。
   */
  async function runBulkStart() {
    setBulkBusy(true);
    const failures: BulkFailure[] = [];
    let ok = 0;
    for (const task of selectedTasks) {
      if (runtimeConditions(task.spec).length > 0) {
        failures.push({
          name: task.name,
          reason: "需要填运行参数，请单独发起",
        });
        continue;
      }
      try {
        await startRun(task.task_id, {});
        ok += 1;
      } catch (error) {
        failures.push({ name: task.name, reason: messageFrom(error) });
      }
    }
    setBulkBusy(false);
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
    setBulkConfirm(false);
    setSelected(new Set());
    setBulkSummary({ verb: "删除", total: selectedTasks.length, ok, failures });
    onChanged();
  }

  const hasTasks = tasks !== null && tasks.length > 0;
  const pageIds = slice.rows.map((task) => task.task_id);
  const allOnPageSelected =
    pageIds.length > 0 && pageIds.every((id) => selected.has(id));

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
        <div className="table-title-row">
          <h1 className="table-title" id="jobs-title">
            作业中心
          </h1>
          <span className="table-count">
            {countLabel(tasks, filtered, refreshing)}
          </span>
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
                  size={17}
                  aria-hidden="true"
                />
              </button>
            </span>
            <button className="button is-primary" type="button" onClick={onCreate}>
              <Plus size={15} aria-hidden="true" />
              新建任务
            </button>
            {/* 未选中时**禁用**，不是能点了才报错（ADR-0043 §6，走查 X15）。 */}
            <button
              className="button"
              type="button"
              disabled={selectedTasks.length === 0 || bulkBusy}
              onClick={() => void runBulkStart()}
            >
              {bulkBusy ? "正在发起" : "批量发起"}
            </button>
            <button
              className="button is-danger"
              type="button"
              disabled={selectedTasks.length === 0 || bulkBusy}
              onClick={() => setBulkConfirm(true)}
            >
              批量删除
            </button>
          </div>
        </div>

        <JobResults
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
          onRename={onRename}
          onDelete={onDelete}
          onStart={onStart}
          onOpen={setOpenTaskId}
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
          onRerun={(task, run) => {
            setOpenTaskId(null);
            onRerun(task, run);
          }}
        />
      )}

      {bulkConfirm && (
        <BulkDeleteDialog
          tasks={selectedTasks}
          busy={bulkBusy}
          onClose={() => setBulkConfirm(false)}
          onConfirm={() => void runBulkDelete()}
        />
      )}
    </>
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
  refreshing,
  selected,
  allOnPageSelected,
  onToggleOne,
  onTogglePage,
  onCreate,
  onEdit,
  onRename,
  onDelete,
  onStart,
  onOpen,
}: {
  tasks: Task[] | null;
  filtered: Task[];
  rows: Task[];
  datasources: Datasource[];
  latestRuns: ReadonlyMap<string, RunHistory>;
  refreshing: boolean;
  selected: ReadonlySet<string>;
  allOnPageSelected: boolean;
  onToggleOne: (taskId: string, checked: boolean) => void;
  onTogglePage: (checked: boolean) => void;
  onCreate: () => void;
  onEdit: (task: Task) => void;
  onRename: (task: Task) => void;
  onDelete: (task: Task) => void;
  onStart: (task: Task) => void;
  onOpen: (taskId: string) => void;
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
  if (filtered.length === 0) {
    // 空表格加一个孤零零的分页条不算回答（走查 X10）。
    return <div className="no-results">没有匹配的任务</div>;
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
            <th>源表</th>
            <th>目标表</th>
            <th>迁移进度</th>
            <th>
              运行状态
              <span className="visually-hidden">
                有进行中任务时会自动刷新。
              </span>
            </th>
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
            return (
              <tr key={task.task_id}>
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
                  <span className="task-name">{task.name}</span>
                  <span className="task-id">{task.task_id}</span>
                </td>
                <td>
                  <span className="table-cell">
                    {task.spec.owner}.{task.spec.table}
                  </span>
                  <span className="table-side">
                    {nameOf(task.source_datasource_id)}
                  </span>
                </td>
                <td>
                  <span className="table-cell">{task.spec.target_table}</span>
                  <span className="table-side">
                    {nameOf(task.target_datasource_id)}
                  </span>
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
                  <span
                    className={`state is-${status}`}
                    title={run === undefined ? undefined : conclusionOf(run)}
                  >
                    {LATEST_RUN_LABELS[status]}
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
                    <ActionButton
                      label="发起运行"
                      icon={<Play size={16} />}
                      onClick={() => onStart(task)}
                    />
                    <span className="divider" />
                    {run === undefined ? (
                      // 尚未运行的任务开不了抽屉——**禁用而不是消失**，原因挂在外层
                      // `span` 的 `title` 上（浏览器不给 `disabled` 控件派发指针事件，
                      // 挂在按钮自己的 `title` 上等于没写）与按钮的 `aria-label` 上。
                      <span
                        className="row-actions"
                        title="运行详情（不可用）：这个任务尚未运行过，没有可看的运行记录。"
                      >
                        <ActionButton
                          label="运行详情（不可用）：这个任务尚未运行过，没有可看的运行记录。"
                          icon={<Clock3 size={16} />}
                          disabled
                          onClick={() => {}}
                        />
                      </span>
                    ) : (
                      <ActionButton
                        label="运行详情"
                        icon={<Clock3 size={16} />}
                        onClick={() => onOpen(task.task_id)}
                      />
                    )}
                    <ActionButton
                      label="编辑任务定义"
                      icon={<Pencil size={16} />}
                      onClick={() => onEdit(task)}
                    />
                    <ActionButton
                      label="改名"
                      icon={<Tag size={16} />}
                      onClick={() => onRename(task)}
                    />
                    <span className="divider" />
                    <ActionButton
                      label="删除"
                      danger
                      icon={<Trash2 size={16} />}
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
          className="button is-danger"
          type="button"
          onClick={onConfirm}
          disabled={busy}
        >
          {busy ? "正在删除" : "确认删除"}
        </button>
      </footer>
    </Modal>
  );
}
