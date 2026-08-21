import { ChevronDown, ChevronRight, RefreshCw, RotateCcw } from "lucide-react";
import { Fragment, useCallback, useEffect, useMemo, useState } from "react";

import { listRunHistory } from "./api";
import type { RunHistory, RunHistoryFilters, Task } from "./api";
import {
  ErrorCodeTag,
  SensitiveValue,
  TerminalBlock,
} from "./components/DesignSystem";
import { messageFrom } from "./errors";
import { formatTimestamp, historyPresentation, runIdPresentation } from "./history";
import type { HistoryPresentation } from "./history";
import {
  DEFAULT_PAGE_SIZE,
  EMPTY_HISTORY_FILTERS,
  historyMatchesFilters,
  paginate,
  RUN_STATUS_LABELS,
  RUN_STATUS_ORDER,
} from "./listing";
import type { HistoryFilters, RunStatus } from "./listing";
import { rerunAction } from "./rerun";
import { runParamsSummary } from "./spec";
import { ActionButton, Pagination } from "./ui";

/** 点了「重跑」之后由 `App` 打开发起对话框——本屏不发请求（规格 #149 A3）。 */
export type RerunRequest = (task: Task, row: RunHistory) => void;

/**
 * `tasks` 是**可空**的：`null` 表示任务清单还没读到，不是「一个任务都没有」。
 * 两者混为一谈会让刚进屏的那一瞬间每一行都报「任务已删除」。
 */
export function HistoryScreen({
  tasks,
  onRerun,
}: {
  tasks: Task[] | null;
  onRerun: RerunRequest;
}) {
  const [history, setHistory] = useState<RunHistory[] | null>(null);
  // 筛选条上**正在填**的那一组，与**已生效**的那一组分开存：查询是显式的
  // （ADR-0041 增补 1 的既有裁定），改一下下拉就重新筛一遍不是本屏的行为。
  const [draft, setDraft] = useState<HistoryFilters>(EMPTY_HISTORY_FILTERS);
  const [applied, setApplied] = useState<HistoryFilters>(EMPTY_HISTORY_FILTERS);
  const [page, setPage] = useState(1);
  const [refreshing, setRefreshing] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const taskId = draft.taskId;

  const loadHistory = useCallback(async (filters: RunHistoryFilters) => {
    setRefreshing(true);
    try {
      setHistory(await listRunHistory(filters));
      setError(null);
    } catch (loadError) {
      setError(messageFrom(loadError));
    } finally {
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void loadHistory({});
  }, [loadHistory]);

  /** 「查询」：任务那一维交给服务端，状态那一维在本地筛，两者同时生效、同时回到第 1 页。 */
  function applyFilters(next: HistoryFilters) {
    setDraft(next);
    setApplied(next);
    setPage(1);
    void loadHistory({ taskId: next.taskId });
  }

  const filtered = useMemo(
    () => (history ?? []).filter((row) => historyMatchesFilters(row, applied)),
    [applied, history],
  );
  const slice = paginate(filtered, page, DEFAULT_PAGE_SIZE);

  const taskOptions = useMemo(() => {
    const names = new Map((tasks ?? []).map((task) => [task.task_id, task.name]));
    for (const row of history ?? []) {
      if (!names.has(row.task_id)) {
        names.set(row.task_id, row.task_id);
      }
    }
    if (taskId !== "" && !names.has(taskId)) {
      names.set(taskId, taskId);
    }
    return [...names.entries()].sort((left, right) =>
      left[1].localeCompare(right[1], "zh-CN"),
    );
  }, [history, taskId, tasks]);
  const taskNames = useMemo(
    () => new Map((tasks ?? []).map((task) => [task.task_id, task.name])),
    [tasks],
  );

  function toggleDetails(runRecordId: string) {
    setExpandedId((current) =>
      current === runRecordId ? null : runRecordId,
    );
  }

  return (
    <section
      className="card history-card"
      id="history"
      aria-labelledby="history-title"
    >
      <header className="card-header">
        <div>
          <h1 id="history-title">运行历史</h1>
          <span className="card-subtitle">
            {history === null ? "正在读取" : historyCountLabel(history, filtered)}
            {" · 保留最近 90 天"}
          </span>
        </div>
        <button
          className="button is-ghost"
          type="button"
          onClick={() => void loadHistory({ taskId: applied.taskId })}
          disabled={refreshing}
        >
          <RefreshCw
            className={refreshing ? "is-spinning" : ""}
            size={15}
            aria-hidden="true"
          />
          {refreshing ? "刷新中" : "刷新"}
        </button>
      </header>
      <div className="history-filters">
        <label className="filter-field">
          <span>任务</span>
          <select
            value={draft.taskId}
            onChange={(event) =>
              setDraft((current) => ({ ...current, taskId: event.target.value }))
            }
          >
            <option value="">全部任务</option>
            {taskOptions.map(([id, name]) => (
              <option key={id} value={id}>
                {name === id ? id : `${name} · ${id}`}
              </option>
            ))}
          </select>
        </label>
        <label className="filter-field is-compact">
          <span>状态</span>
          <select
            value={draft.status}
            onChange={(event) =>
              setDraft((current) => ({
                ...current,
                status: event.target.value as RunStatus | "",
              }))
            }
          >
            <option value="">全部</option>
            {RUN_STATUS_ORDER.map((status) => (
              <option key={status} value={status}>
                {RUN_STATUS_LABELS[status]}
              </option>
            ))}
          </select>
        </label>
        <button
          className="button is-primary"
          type="button"
          onClick={() => applyFilters(draft)}
          disabled={refreshing}
        >
          查询
        </button>
        <button
          className="button is-ghost"
          type="button"
          onClick={() => applyFilters(EMPTY_HISTORY_FILTERS)}
          disabled={refreshing && draft.status === "" && taskId === ""}
        >
          重置
        </button>
      </div>
      {error !== null && (
        <div className="history-error" role="alert">
          {error}
        </div>
      )}
      <HistoryResults
        history={history === null ? null : slice.rows}
        refreshing={refreshing}
        expandedId={expandedId}
        taskNames={taskNames}
        tasks={tasks}
        onToggle={toggleDetails}
        onRerun={onRerun}
      />
      {history !== null && (
        <Pagination
          page={slice.page}
          pageCount={slice.pageCount}
          total={slice.total}
          pageSize={slice.pageSize}
          onPage={setPage}
        />
      )}
    </section>
  );
}

/**
 * 卡片头那句计数。筛出来的与总共有的**不同时才写两个数**——
 * 没筛的时候写「筛出 12 / 共 12 条」是在制造一个不存在的区别。
 */
function historyCountLabel(
  history: readonly RunHistory[],
  filtered: readonly RunHistory[],
): string {
  return filtered.length === history.length
    ? `共 ${history.length} 条`
    : `筛出 ${filtered.length} / 共 ${history.length} 条`;
}

interface HistoryResultsProps {
  history: RunHistory[] | null;
  refreshing: boolean;
  expandedId: string | null;
  taskNames: ReadonlyMap<string, string>;
  tasks: Task[] | null;
  onToggle: (runRecordId: string) => void;
  onRerun: RerunRequest;
}

function HistoryResults({
  history,
  refreshing,
  expandedId,
  taskNames,
  tasks,
  onToggle,
  onRerun,
}: HistoryResultsProps) {
  if (history === null) {
    return (
      <div className="loading-state" aria-live="polite">
        {refreshing ? "正在加载运行历史..." : "运行历史暂不可用"}
      </div>
    );
  }
  if (history.length === 0) {
    return <div className="no-results">当前筛选下没有运行历史</div>;
  }

  return (
    <div className="table-wrap history-table-wrap">
      <table className="data-grid history-grid">
        <thead>
          <tr>
            <th>任务</th>
            <th className="outcome-column">结局</th>
            <th>错误码</th>
            <th className="numeric-column">行数</th>
            <th className="numeric-column">耗时</th>
            <th>发起于</th>
            <th>操作</th>
            <th className="expand-column">
              <span className="visually-hidden">详情</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {history.map((row) => (
            <HistoryTableRow
              key={row.run_record_id}
              row={row}
              expanded={expandedId === row.run_record_id}
              taskName={taskNames.get(row.task_id)}
              tasks={tasks}
              onToggle={onToggle}
              onRerun={onRerun}
            />
          ))}
        </tbody>
      </table>
    </div>
  );
}

interface HistoryTableRowProps {
  row: RunHistory;
  expanded: boolean;
  taskName: string | undefined;
  tasks: Task[] | null;
  onToggle: (runRecordId: string) => void;
  onRerun: RerunRequest;
}

function HistoryTableRow({
  row,
  expanded,
  taskName,
  tasks,
  onToggle,
  onRerun,
}: HistoryTableRowProps) {
  const presentation = historyPresentation(row);
  const detailToggleLabel = expanded ? "收起详情" : "展开详情";

  return (
    <Fragment>
      <tr className={expanded ? "is-expanded" : ""}>
        <td>
          <span className="task-name">{taskName ?? row.task_id}</span>
          <span className="task-id">{row.task_id}</span>
          <button
            className="history-link"
            type="button"
            aria-expanded={expanded}
            onClick={() => onToggle(row.run_record_id)}
          >
            运行记录 · {row.run_record_id}
          </button>
        </td>
        <td className="outcome-column">
          <HistoryOutcome row={row} presentation={presentation} />
        </td>
        <td>
          <HistoryErrorCell presentation={presentation} />
        </td>
        <td className="numeric-column mono">
          {formatCount(row.source_rows ?? row.rows_pushed)}
        </td>
        <td className="numeric-column mono">{formatElapsed(row)}</td>
        <td className="mono history-time">{formatTimestamp(row.started_at)}</td>
        <td>
          <RerunCell row={row} tasks={tasks} onRerun={onRerun} />
        </td>
        <td>
          <button
            className="detail-toggle"
            type="button"
            title={detailToggleLabel}
            aria-label={detailToggleLabel}
            aria-expanded={expanded}
            onClick={() => onToggle(row.run_record_id)}
          >
            {expanded ? (
              <ChevronDown size={15} />
            ) : (
              <ChevronRight size={15} />
            )}
          </button>
        </td>
      </tr>
      {expanded && (
        <tr className="history-detail-row">
          <td colSpan={8}>
            <RunHistoryDetail
              row={row}
              taskName={taskName}
              presentation={presentation}
            />
          </td>
        </tr>
      )}
    </Fragment>
  );
}

/**
 * 「重跑」动作位。三态由 `rerunAction` 判，本组件只负责摆出来：
 * 没资格的给一个占位破折号（**不是空白**，空白会让人以为这列漏渲染了），
 * 任务没了的给禁用按钮 + 原因（规格 #149 A6）——入口不许凭空消失。
 */
function RerunCell({
  row,
  tasks,
  onRerun,
}: {
  row: RunHistory;
  tasks: Task[] | null;
  onRerun: RerunRequest;
}) {
  const action = rerunAction(row, tasks);
  if (action.kind === "hidden") {
    return <span className="empty-value">—</span>;
  }
  if (action.kind === "disabled") {
    // 原因挂在**外层 span** 上：浏览器不给 `disabled` 控件派发指针事件，
    // 挂在按钮自己的 `title` 上等于没写（悬停永远不出提示）。
    // `aria-label` 也把原因带上，否则屏幕阅读器只听见一个按不动的「重跑」。
    const label = `重跑（不可用）：${action.reason}`;
    return (
      <span className="row-actions" title={label}>
        <ActionButton
          label={label}
          icon={<RotateCcw size={15} />}
          disabled
          onClick={() => {}}
        />
      </span>
    );
  }
  return (
    <span className="row-actions">
      <ActionButton
        label="重跑"
        title="重跑：按这次的运行参数预填发起对话框"
        icon={<RotateCcw size={15} />}
        onClick={() => onRerun(action.task, row)}
      />
    </span>
  );
}

function HistoryOutcome({
  row,
  presentation,
}: {
  row: RunHistory;
  presentation: HistoryPresentation;
}) {
  if (presentation.kind === "unknown") {
    return (
      <span
        className={`unknown-summary is-${row.unknown_reason?.toLowerCase()}`}
      >
        <span>结局不明</span>
        <small>{presentation.conclusion}</small>
      </span>
    );
  }
  if (presentation.kind === "live") {
    return <span className="live-summary">{presentation.conclusion}</span>;
  }
  if (presentation.terminalEffect !== null) {
    return <TerminalBlock effect={presentation.terminalEffect} />;
  }
  if (row.run_id === null) {
    return <span className="neutral-outcome">未发起</span>;
  }
  if (row.sink_code === "PRECHECK_FAILED") {
    return <span className="neutral-outcome">未建暂存表</span>;
  }
  if (row.target_table_effect === "UNKNOWN") {
    return <span className="neutral-outcome mono">UNKNOWN</span>;
  }
  return <span className="neutral-outcome">{row.outcome ?? "进行中"}</span>;
}

function HistoryErrorCell({
  presentation,
}: {
  presentation: HistoryPresentation;
}) {
  if (presentation.error === null) {
    return <span className="empty-value">—</span>;
  }
  const category =
    presentation.error.httpStatus === null ||
    presentation.error.httpStatus >= 500
      ? "is-internal"
      : "is-rejected";
  return (
    <span className={`error-code ${category}`}>
      {presentation.error.code}
      {presentation.error.httpStatus !== null && (
        <span className="http-code">{presentation.error.httpStatus}</span>
      )}
    </span>
  );
}

interface RunHistoryDetailProps {
  row: RunHistory;
  taskName: string | undefined;
  presentation: HistoryPresentation;
}

function RunHistoryDetail({
  row,
  taskName,
  presentation,
}: RunHistoryDetailProps) {
  return (
    <section
      className="history-detail"
      aria-label={`${row.run_record_id} 运行详情`}
    >
      <dl className="identity-grid">
        <DetailValue label="运行记录" value={row.run_record_id} />
        <DetailValue label="目标端运行号" value={runIdPresentation(row)} />
        <DetailValue label="任务" value={taskName ?? row.task_id} />
        <DetailValue label="运行参数" value={runParamsSummary(row.run_params)} />
        <DetailValue label="暂存表" value={row.staging_table ?? "—"} />
        <DetailValue
          label="发起时间"
          value={formatTimestamp(row.started_at, true)}
        />
        <DetailValue
          label="结束时间"
          value={formatTimestamp(row.finished_at, true)}
        />
      </dl>

      {presentation.kind === "unknown" ? (
        <div
          className={`unknown-conclusion is-${row.unknown_reason?.toLowerCase()}`}
        >
          <strong>结局不明</strong>
          <span>{presentation.conclusion}</span>
          <small>无法确认目标表是否被修改，请到目标库核对。</small>
        </div>
      ) : (
        <>
          <div className="detail-status">
            <span className="outcome-label">
              运行结果 <strong>{row.outcome ?? "进行中"}</strong>
            </span>
            {presentation.terminalEffect !== null && (
              <TerminalBlock effect={presentation.terminalEffect} />
            )}
            {row.target_table_effect === "UNKNOWN" && (
              <span className="unknown-effect">UNKNOWN　目标表效果未知</span>
            )}
            {presentation.terminalEffect === null &&
              row.target_table_effect !== null &&
              row.target_table_effect !== "UNKNOWN" && (
                <span className="effect-text">
                  目标表 <strong>{row.target_table_effect}</strong>
                </span>
              )}
            {row.source_code !== null && (
              <span className="source-code">
                源端 <strong>{row.source_code}</strong>
              </span>
            )}
          </div>
          {presentation.kind === "failed" && presentation.error !== null && (
            <ErrorCodeTag
              code={presentation.error.code}
              httpStatus={presentation.error.httpStatus ?? undefined}
              conclusion={presentation.conclusion}
            />
          )}
          {presentation.kind === "failed" && presentation.error === null && (
            <div className="plain-conclusion">{presentation.conclusion}</div>
          )}
        </>
      )}

      <section className="history-source-sql">
        <h2>当次执行的源端 SQL</h2>
        <p>
          本次运行实际执行的语句；参数值见上方运行参数。
        </p>
        <pre className="mono">{row.source_sql}</pre>
      </section>

      <HistoryMetrics row={row} />
      {(row.column !== null || row.value !== null) &&
        presentation.kind !== "unknown" && (
          <SensitiveValue
            column={row.column ?? undefined}
            value={row.value ?? undefined}
          />
        )}
    </section>
  );
}

function HistoryMetrics({ row }: { row: RunHistory }) {
  return (
    <div className="history-metric-groups">
      <section>
        <h2>行数核对</h2>
        <dl className="metric-grid">
          <DetailValue
            label="源端读取"
            value={formatOptionalCount(row.source_rows)}
          />
          <DetailValue
            label="暂存写入"
            value={formatOptionalCount(row.staged_rows)}
          />
          <DetailValue
            label="目标端回报"
            value={formatOptionalCount(row.sink_reported_rows)}
          />
          <DetailValue
            label="清理行数"
            value={formatOptionalCount(row.purged_rows)}
          />
        </dl>
      </section>
      <section>
        <h2>分段耗时</h2>
        <dl className="metric-grid">
          <DetailValue
            label="取数"
            value={formatMilliseconds(row.fetch_ms)}
          />
          <DetailValue
            label="推送"
            value={formatMilliseconds(row.push_ms)}
          />
          <DetailValue
            label="提交"
            value={formatMilliseconds(row.commit_ms)}
          />
          <DetailValue
            label="计数"
            value={formatMilliseconds(row.count_ms)}
          />
          <DetailValue
            label="开游标"
            value={formatMilliseconds(row.cursor_ms)}
          />
        </dl>
      </section>
    </div>
  );
}

function DetailValue({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

const countFormatter = new Intl.NumberFormat("zh-CN");

function formatCount(value: number): string {
  return countFormatter.format(value);
}

function formatOptionalCount(value: number | null): string {
  return value === null ? "—" : formatCount(value);
}

function formatMilliseconds(value: number | null): string {
  return value === null ? "—" : `${formatCount(value)} ms`;
}

function formatElapsed(row: RunHistory): string {
  const startedAt = Date.parse(row.started_at);
  const finishedAt =
    row.finished_at === null ? Number.NaN : Date.parse(row.finished_at);
  const milliseconds = Number.isNaN(finishedAt)
    ? row.ms
    : Math.max(0, finishedAt - startedAt);
  const totalSeconds = Math.floor(milliseconds / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  return [hours, minutes, seconds]
    .map((part) => String(part).padStart(2, "0"))
    .join(":");
}
