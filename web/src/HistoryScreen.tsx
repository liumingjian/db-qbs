import { ChevronDown, ChevronRight, RefreshCw } from "lucide-react";
import { Fragment, useCallback, useEffect, useMemo, useState } from "react";

import { listRunHistory } from "./api";
import type { RunHistory, RunHistoryFilters, Task } from "./api";
import {
  ErrorCodeTag,
  SensitiveValue,
  TerminalBlock,
} from "./components/DesignSystem";
import { messageFrom } from "./errors";
import { historyPresentation, runIdPresentation } from "./history";
import type { HistoryPresentation } from "./history";
import { runParamsSummary } from "./spec";

export function HistoryScreen({ tasks }: { tasks: Task[] }) {
  const [history, setHistory] = useState<RunHistory[] | null>(null);
  const [taskId, setTaskId] = useState("");
  const [refreshing, setRefreshing] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);

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

  const taskOptions = useMemo(() => {
    const names = new Map(tasks.map((task) => [task.task_id, task.name]));
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
    () => new Map(tasks.map((task) => [task.task_id, task.name])),
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
            {history === null ? "正在读取" : `共 ${history.length} 条`}
            {" · 保留 90 天，按时间不按条数"}
          </span>
        </div>
        <button
          className="button is-ghost"
          type="button"
          onClick={() => void loadHistory({ taskId })}
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
            value={taskId}
            onChange={(event) => setTaskId(event.target.value)}
          >
            <option value="">全部任务</option>
            {taskOptions.map(([id, name]) => (
              <option key={id} value={id}>
                {name === id ? id : `${name} · ${id}`}
              </option>
            ))}
          </select>
        </label>
      </div>
      {error !== null && (
        <div className="history-error" role="alert">
          {error}
        </div>
      )}
      <HistoryResults
        history={history}
        refreshing={refreshing}
        expandedId={expandedId}
        taskNames={taskNames}
        onToggle={toggleDetails}
      />
    </section>
  );
}

interface HistoryResultsProps {
  history: RunHistory[] | null;
  refreshing: boolean;
  expandedId: string | null;
  taskNames: ReadonlyMap<string, string>;
  onToggle: (runRecordId: string) => void;
}

function HistoryResults({
  history,
  refreshing,
  expandedId,
  taskNames,
  onToggle,
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
            <th>RUN_RECORD_ID</th>
            <th>RUN_ID</th>
            <th>任务</th>
            <th>运行参数</th>
            <th className="outcome-column">结局</th>
            <th>错误码</th>
            <th className="numeric-column">行数</th>
            <th className="numeric-column">耗时</th>
            <th>发起于</th>
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
              onToggle={onToggle}
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
  onToggle: (runRecordId: string) => void;
}

function HistoryTableRow({
  row,
  expanded,
  taskName,
  onToggle,
}: HistoryTableRowProps) {
  const presentation = historyPresentation(row);
  const detailToggleLabel = expanded ? "收起详情" : "展开详情";

  return (
    <Fragment>
      <tr className={expanded ? "is-expanded" : ""}>
        <td className="mono">
          <button
            className="history-link"
            type="button"
            aria-expanded={expanded}
            onClick={() => onToggle(row.run_record_id)}
          >
            {row.run_record_id}
          </button>
        </td>
        <td className={row.run_id === null ? "missing-run-id" : "mono run-id-cell"}>
          {runIdPresentation(row)}
        </td>
        <td>
          <span className="task-name">{taskName ?? row.task_id}</span>
          <span className="task-id">{row.task_id}</span>
        </td>
        <td className="mono run-params-cell" title={runParamsSummary(row.run_params)}>
          {runParamsSummary(row.run_params)}
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
          <td colSpan={10}>
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
        <DetailValue label="run_record_id" value={row.run_record_id} />
        <DetailValue label="run_id" value={runIdPresentation(row)} />
        <DetailValue label="task" value={taskName ?? row.task_id} />
        <DetailValue label="run_params" value={runParamsSummary(row.run_params)} />
        <DetailValue label="staging_table" value={row.staging_table ?? "—"} />
        <DetailValue
          label="started_at"
          value={formatTimestamp(row.started_at, true)}
        />
        <DetailValue
          label="finished_at"
          value={formatTimestamp(row.finished_at, true)}
        />
      </dl>

      {presentation.kind === "unknown" ? (
        <div
          className={`unknown-conclusion is-${row.unknown_reason?.toLowerCase()}`}
        >
          <strong>结局不明</strong>
          <span>{presentation.conclusion}</span>
          <small>目标表效果未知；没有错误码，也没有目标端终态块。</small>
        </div>
      ) : (
        <>
          <div className="detail-status">
            <span className="outcome-label">
              outcome <strong>{row.outcome ?? "进行中"}</strong>
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
                  target_table_effect <strong>{row.target_table_effect}</strong>
                </span>
              )}
            {row.source_code !== null && (
              <span className="source-code">
                source_code <strong>{row.source_code}</strong>
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
          这是<strong>当时实际执行</strong>的语句快照（ADR-0036 §2），任务定义之后怎么改都不会改到它。
          存的是未绑定的语句文本，参数值不内联——值看上面的 <code>run_params</code>。
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
        <h2>门禁四数</h2>
        <dl className="metric-grid">
          <DetailValue
            label="source_rows"
            value={formatOptionalCount(row.source_rows)}
          />
          <DetailValue
            label="staged_rows"
            value={formatOptionalCount(row.staged_rows)}
          />
          <DetailValue
            label="sink_reported_rows"
            value={formatOptionalCount(row.sink_reported_rows)}
          />
          <DetailValue
            label="purged_rows"
            value={formatOptionalCount(row.purged_rows)}
          />
        </dl>
      </section>
      <section>
        <h2>分段耗时</h2>
        <dl className="metric-grid">
          <DetailValue
            label="fetch_ms"
            value={formatMilliseconds(row.fetch_ms)}
          />
          <DetailValue
            label="push_ms"
            value={formatMilliseconds(row.push_ms)}
          />
          <DetailValue
            label="commit_ms"
            value={formatMilliseconds(row.commit_ms)}
          />
          <DetailValue
            label="count_ms"
            value={formatMilliseconds(row.count_ms)}
          />
          <DetailValue
            label="cursor_ms"
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

function formatTimestamp(value: string | null, includeDate = false): string {
  if (value === null) {
    return "—";
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return new Intl.DateTimeFormat("zh-CN", {
    year: includeDate ? "numeric" : undefined,
    month: includeDate ? "2-digit" : undefined,
    day: includeDate ? "2-digit" : undefined,
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(date);
}
