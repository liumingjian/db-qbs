import { ArrowLeft, Ban, Pencil, Play, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";

import { cancelRun, fetchRun } from "./api";
import type { RunDetail, Task } from "./api";
import {
  ErrorCodeTag,
  PhaseLine,
  SensitiveValue,
  TerminalBlock,
} from "./components/DesignSystem";
import { messageFrom } from "./errors";
import { runIdPresentation } from "./history";
import { mappingSuggestion } from "./m3";
import { runPresentation } from "./run";
import type { RunPresentation } from "./run";
import { runParamsSummary } from "./spec";

const RUN_POLL_INTERVAL_MS = 1000;
const countFormatter = new Intl.NumberFormat("zh-CN");

export function RunScreen({
  task,
  runRecordId,
  onBack,
  onRelaunch,
  onEditTask,
}: {
  task: Task;
  runRecordId: string;
  onBack: () => void;
  onRelaunch: () => void;
  onEditTask: () => void;
}) {
  const [detail, setDetail] = useState<RunDetail | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [cancelMessage, setCancelMessage] = useState<string | null>(null);

  useEffect(() => {
    let effectActive = true;
    let runIsLive = true;
    let requestInFlight = false;
    let pollTimer: number | undefined;

    function canLoadRun() {
      return (
        effectActive &&
        runIsLive &&
        document.visibilityState === "visible" &&
        !requestInFlight
      );
    }

    function scheduleNextLoad() {
      if (!canLoadRun() || pollTimer !== undefined) {
        return;
      }
      pollTimer = window.setTimeout(() => {
        pollTimer = undefined;
        void load();
      }, RUN_POLL_INTERVAL_MS);
    }

    async function load() {
      if (!canLoadRun()) {
        return;
      }
      requestInFlight = true;
      try {
        const nextDetail = await fetchRun(runRecordId);
        if (!effectActive) {
          return;
        }
        setDetail(nextDetail);
        setLoadError(null);
        runIsLive = nextDetail.live;
      } catch (error) {
        if (effectActive) {
          setLoadError(messageFrom(error));
        }
      } finally {
        requestInFlight = false;
        scheduleNextLoad();
      }
    }

    function handleVisibilityChange() {
      if (pollTimer !== undefined) {
        window.clearTimeout(pollTimer);
        pollTimer = undefined;
      }
      if (canLoadRun()) {
        void load();
      }
    }

    document.addEventListener("visibilitychange", handleVisibilityChange);
    void load();
    return () => {
      effectActive = false;
      if (pollTimer !== undefined) {
        window.clearTimeout(pollTimer);
      }
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  }, [runRecordId]);

  const presentation = detail === null ? null : runPresentation(detail);

  async function handleCancel() {
    setCancelMessage(null);
    try {
      const response = await cancelRun(runRecordId);
      setCancelMessage(response.message);
    } catch (error) {
      setCancelMessage(messageFrom(error));
    }
  }

  return (
    <section className="card run-card" aria-labelledby="run-title">
      <header className="card-header run-header">
        <div>
          <button className="back-button" type="button" onClick={onBack}>
            <ArrowLeft size={15} aria-hidden="true" />
            返回任务
          </button>
          <h1 id="run-title">{task.name}</h1>
          <span className="card-subtitle mono">运行记录 · {runRecordId}</span>
        </div>
        {detail?.live === true ? (
          <button
            className="button is-ghost"
            type="button"
            onClick={() => void handleCancel()}
          >
            <Ban size={15} aria-hidden="true" />
            取消运行
          </button>
        ) : (
          <button className="button is-primary" type="button" onClick={onRelaunch}>
            <Play size={15} aria-hidden="true" />
            重新发起
          </button>
        )}
      </header>

      {loadError !== null && (
        <div className="run-notice is-error" role="alert">
          <span>{loadError}</span>
          <span>将在页面可见时继续读取。</span>
        </div>
      )}
      {cancelMessage !== null && (
        <div className="run-notice" role="status">{cancelMessage}</div>
      )}

      {detail === null || presentation === null ? (
        <div className="loading-state">
          <RefreshCw className="is-spinning" size={16} aria-hidden="true" />
          正在读取运行详情...
        </div>
      ) : (
        <div className="run-content">
          <RunIdentity task={task} detail={detail} />
          {detail.live ? (
            <LiveRun detail={detail} presentation={presentation} />
          ) : (
            <FinishedRun
              detail={detail}
              presentation={presentation}
              onEditTask={onEditTask}
            />
          )}
        </div>
      )}
    </section>
  );
}

function RunIdentity({ task, detail }: { task: Task; detail: RunDetail }) {
  return (
    <dl className="run-identity">
      <DetailValue label="运行记录" value={detail.run_record_id} />
      <DetailValue
        label="目标端运行号"
        value={runIdPresentation(detail)}
      />
      <DetailValue label="所属任务" value={task.task_id} />
      <DetailValue label="运行参数" value={runParamsSummary(detail.run_params)} />
      <DetailValue label="暂存表" value={detail.staging_table ?? "—"} />
    </dl>
  );
}

function LiveRun({
  detail,
  presentation,
}: {
  detail: RunDetail & { live: true };
  presentation: RunPresentation;
}) {
  return (
    <>
      <section className={`live-state is-${presentation.kind}`}>
        <PhaseLine current={presentation.phase} />
        <div className="indeterminate-progress" aria-label="运行进行中">
          <span />
        </div>
        <strong>{presentation.conclusion}</strong>
        {presentation.kind === "accepted" && (
          <span>已受理，正在启动。</span>
        )}
      </section>
      <dl className="run-metrics">
        <Metric label="已推行数" value={formatCount(detail.rows_pushed)} />
        <Metric label="当前批次序号" value={formatCount(detail.seq)} />
        <Metric label="已用时" value={formatDuration(detail.ms)} />
        <Metric label="累计字节" value={formatBytes(detail.bytes)} />
      </dl>
    </>
  );
}

function FinishedRun({
  detail,
  presentation,
  onEditTask,
}: {
  detail: RunDetail & { live: false };
  presentation: RunPresentation;
  onEditTask: () => void;
}) {
  const mappingFailed = presentation.kind === "mapping-failed";
  return (
    <>
      <section className={`run-result is-${presentation.kind}`}>
        <div className="run-result-heading">
          <span className="outcome-label">
            outcome <strong>{detail.outcome ?? "UNKNOWN"}</strong>
          </span>
          {presentation.terminalEffect !== null && (
            <TerminalBlock effect={presentation.terminalEffect} />
          )}
        </div>
        <RunConclusion detail={detail} presentation={presentation} />
      </section>

      {mappingFailed && <PrecheckReports detail={detail} />}

      {mappingFailed && (
        <div className="precheck-exit">
          <span>
            目标表结构与本次取数的列对不上。请在目标库中调整目标表，或回到任务编辑修改字段映射。
          </span>
          <button className="button is-ghost" type="button" onClick={onEditTask}>
            <Pencil size={15} aria-hidden="true" />
            编辑任务
          </button>
        </div>
      )}

      <dl className="run-metrics is-finished">
        <Metric label="已推行数" value={formatCount(detail.rows_pushed)} />
        <Metric label="批次数" value={formatCount(detail.seq)} />
        <Metric label="累计批次耗时" value={formatDuration(detail.ms)} />
        <Metric label="累计字节" value={formatBytes(detail.bytes)} />
      </dl>

      {(detail.column !== null || detail.value !== null) &&
        presentation.kind !== "unknown" && (
          <SensitiveValue
            column={detail.column ?? undefined}
            value={detail.value ?? undefined}
          />
        )}
    </>
  );
}

function RunConclusion({
  detail,
  presentation,
}: {
  detail: RunDetail & { live: false };
  presentation: RunPresentation;
}) {
  if (presentation.kind === "unknown") {
    return (
      <div className={`unknown-conclusion is-${detail.unknown_reason?.toLowerCase()}`}>
        <strong>结局不明</strong>
        <span>{presentation.conclusion}</span>
        <small>无法确认目标表是否被修改，请到目标库核对。</small>
      </div>
    );
  }

  if (presentation.error !== null) {
    return (
      <ErrorCodeTag
        code={presentation.error.code}
        httpStatus={presentation.error.httpStatus ?? undefined}
        conclusion={presentation.conclusion}
      />
    );
  }

  const className =
    presentation.kind === "succeeded"
      ? "success-conclusion"
      : "plain-conclusion";
  return <div className={className}>{presentation.conclusion}</div>;
}

/**
 * 映射预检报告（ADR-0009）。
 *
 * 这里过去是并排两段：source 本地的 SQL 形状预检 + sink 的映射预检。形状预检整段随
 * ADR-0036 §5 取消，于是只剩这一段——**不留占位空栏**，没有的东西不摆在屏上说「未执行」。
 *
 * 容器随之改成单段布局（#132）：两栏 grid 与 `is-map-failed`（「映射失败时整宽」）在只剩
 * 一段之后都没有对象了，一并撤掉；态修饰只保留 `.is-failed`，本组件只在映射预检失败时渲染。
 */
function PrecheckReports({
  detail,
}: {
  detail: RunDetail & { live: false };
}) {
  return (
    <div className="precheck-reports">
      <section className="is-failed">
        <header>
          <strong>映射预检</strong>
          <span>目标端</span>
        </header>
        <p>{detail.message ?? "目标端映射预检未通过。"}</p>
        <DiagnosticTable
          columns={["列", "源端", "目标端", "规则", "建议"]}
          rows={detail.mapping_issues.map((issue) => [
            issue.column ?? "—",
            issue.source ?? "—",
            issue.target ?? "—",
            issue.rule ?? issue.message ?? "—",
            mappingSuggestion(issue),
          ])}
        />
        <small>总计 {detail.mapping_issues.length} 项问题</small>
      </section>
    </div>
  );
}

function DiagnosticTable({
  columns,
  rows,
}: {
  columns: string[];
  rows: string[][];
}) {
  return (
    <div className="diagnostic-table-wrap">
      <table className="diagnostic-table">
        <thead>
          <tr>{columns.map((column) => <th key={column}>{column}</th>)}</tr>
        </thead>
        <tbody>
          {rows.map((row, rowIndex) => (
            <tr key={`${row[0]}-${rowIndex}`}>
              {row.map((value, columnIndex) => (
                <td key={`${columns[columnIndex]}-${columnIndex}`}>{value}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
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

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

function formatCount(value: number): string {
  return countFormatter.format(value);
}

function formatDuration(milliseconds: number): string {
  const totalSeconds = Math.floor(milliseconds / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${formatCount(bytes)} B`;
  }
  return `${(bytes / 1024).toFixed(1)} KiB`;
}
