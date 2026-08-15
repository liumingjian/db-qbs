import { ArrowLeft, Ban, Play, RefreshCw } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { cancelRun, fetchRun } from "./api";
import type { RunDetail, Task } from "./api";
import {
  ErrorCodeTag,
  PhaseLine,
  SensitiveValue,
  TerminalBlock,
} from "./components/DesignSystem";
import { messageFrom } from "./errors";
import { runPresentation } from "./run";

const countFormatter = new Intl.NumberFormat("zh-CN");

export function RunScreen({
  task,
  runRecordId,
  onBack,
  onRelaunch,
}: {
  task: Task;
  runRecordId: string;
  onBack: () => void;
  onRelaunch: () => void;
}) {
  const [detail, setDetail] = useState<RunDetail | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [cancelMessage, setCancelMessage] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let live = true;
    let timer: number | undefined;

    function schedule() {
      if (active && live && document.visibilityState === "visible") {
        timer = window.setTimeout(() => void load(), 1000);
      }
    }

    async function load() {
      if (!active) {
        return;
      }
      try {
        const nextDetail = await fetchRun(runRecordId);
        if (!active) {
          return;
        }
        setDetail(nextDetail);
        setLoadError(null);
        live = nextDetail.live;
        schedule();
      } catch (error) {
        if (active) {
          setLoadError(messageFrom(error));
          schedule();
        }
      }
    }

    function handleVisibilityChange() {
      if (timer !== undefined) {
        window.clearTimeout(timer);
        timer = undefined;
      }
      if (document.visibilityState === "visible" && live) {
        void load();
      }
    }

    document.addEventListener("visibilitychange", handleVisibilityChange);
    void load();
    return () => {
      active = false;
      if (timer !== undefined) {
        window.clearTimeout(timer);
      }
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  }, [runRecordId]);

  const presentation = useMemo(
    () => (detail === null ? null : runPresentation(detail)),
    [detail],
  );

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
          <span className="card-subtitle mono">run_record_id · {runRecordId}</span>
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
            <FinishedRun detail={detail} presentation={presentation} />
          )}
        </div>
      )}
    </section>
  );
}

function RunIdentity({ task, detail }: { task: Task; detail: RunDetail }) {
  return (
    <dl className="run-identity">
      <DetailValue label="run_record_id" value={detail.run_record_id} />
      <DetailValue
        label="run_id"
        value={detail.run_id ?? "未发起，目标端不知道这次运行"}
      />
      <DetailValue label="task_id" value={task.task_id} />
      <DetailValue label="biz_date" value={detail.biz_date ?? "等待子进程回报"} />
      <DetailValue label="staging_table" value={detail.staging_table ?? "—"} />
    </dl>
  );
}

function LiveRun({
  detail,
  presentation,
}: {
  detail: RunDetail & { live: true };
  presentation: ReturnType<typeof runPresentation>;
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
          <span>父进程已经受理，子进程尚未进入 PREPARING。</span>
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
}: {
  detail: RunDetail & { live: false };
  presentation: ReturnType<typeof runPresentation>;
}) {
  const showPrechecks =
    presentation.kind === "shape-failed" || presentation.kind === "mapping-failed";
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
        {presentation.kind === "unknown" ? (
          <div className={`unknown-conclusion is-${detail.unknown_reason?.toLowerCase()}`}>
            <strong>结局不明</strong>
            <span>{presentation.conclusion}</span>
            <small>没有错误码，也没有目标端终态块。</small>
          </div>
        ) : presentation.error !== null ? (
          <ErrorCodeTag
            code={presentation.error.code}
            httpStatus={presentation.error.httpStatus ?? undefined}
            conclusion={presentation.conclusion}
          />
        ) : (
          <div className={presentation.kind === "succeeded" ? "success-conclusion" : "plain-conclusion"}>
            {presentation.conclusion}
          </div>
        )}
      </section>

      {showPrechecks && <PrecheckReports detail={detail} kind={presentation.kind} />}

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

function PrecheckReports({
  detail,
  kind,
}: {
  detail: RunDetail & { live: false };
  kind: ReturnType<typeof runPresentation>["kind"];
}) {
  const shapeFailed = kind === "shape-failed";
  return (
    <div className="precheck-reports">
      <section className={shapeFailed ? "is-failed" : "is-passed"}>
        <header>
          <strong>SQL 形状预检</strong>
          <span>source 本地</span>
        </header>
        <p>{shapeFailed ? detail.message : "六条形状规则已通过。"}</p>
        <DiagnosticTable
          columns={["规则", "结果", "说明"]}
          rows={detail.shape_checks.map((check) => [
            check.rule,
            check.passed ? "通过" : "未通过",
            check.message,
          ])}
        />
        {shapeFailed && <small>六条规则一次报告；本次未向 sink 发出请求。</small>}
      </section>
      <section className={shapeFailed ? "is-skipped" : "is-failed"}>
        <header>
          <strong>映射预检</strong>
          <span>sink</span>
        </header>
        {shapeFailed ? (
          <p>未执行</p>
        ) : (
          <>
            <p>{detail.message ?? "目标端映射预检未通过。"}</p>
            <DiagnosticTable
              columns={["列", "源端", "目标端", "规则"]}
              rows={detail.mapping_issues.map((issue) => [
                issue.column ?? "—",
                issue.source ?? "—",
                issue.target ?? "—",
                issue.rule ?? issue.message ?? "—",
              ])}
            />
            <small>总计 {detail.mapping_issues.length} 项问题</small>
          </>
        )}
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
