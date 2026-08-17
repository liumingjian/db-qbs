import { ArrowLeft, Ban, Play, RefreshCw, TableProperties } from "lucide-react";
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
import {
  failedShapeRuleCount,
  shapeRuleDescription,
  shapeRuleLabel,
} from "./shape";
import { runPresentation } from "./run";
import type { RunPresentation, RunPresentationKind } from "./run";

const RUN_POLL_INTERVAL_MS = 1000;
const countFormatter = new Intl.NumberFormat("zh-CN");
type PrecheckKind = Extract<
  RunPresentationKind,
  "shape-failed" | "mapping-failed"
>;

export function RunScreen({
  task,
  runRecordId,
  onBack,
  onRelaunch,
  onOpenColumnFetch,
}: {
  task: Task;
  runRecordId: string;
  onBack: () => void;
  onRelaunch: () => void;
  onOpenColumnFetch: () => void;
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
            <FinishedRun
              detail={detail}
              presentation={presentation}
              onOpenColumnFetch={onOpenColumnFetch}
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
  onOpenColumnFetch,
}: {
  detail: RunDetail & { live: false };
  presentation: RunPresentation;
  onOpenColumnFetch: () => void;
}) {
  const precheckKind =
    presentation.kind === "shape-failed" || presentation.kind === "mapping-failed"
      ? presentation.kind
      : null;
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

      {precheckKind !== null && (
        <PrecheckReports detail={detail} kind={precheckKind} />
      )}

      {precheckKind === "mapping-failed" && (
        <div className="precheck-exit">
          <span>
            目标表和这段 SQL 对不上。建表 SQL 在取列那一步现取，这屏不重给——
            免得你拿着旧的去撞 <code>ERROR 1050</code>。
          </span>
          <button className="button is-ghost" type="button" onClick={onOpenColumnFetch}>
            <TableProperties size={15} aria-hidden="true" />
            回到取列拿建表 SQL
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
        <small>没有错误码，也没有目标端终态块。</small>
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

function PrecheckReports({
  detail,
  kind,
}: {
  detail: RunDetail & { live: false };
  kind: PrecheckKind;
}) {
  const shapeFailed = kind === "shape-failed";
  // 副标题不复述结论条：卡头已经说了这是哪一段预检，结论条已经说了「未向 sink 发出请求」，
  // 这里只报「六条里几条没过」，与通过态那句同句式。source 回的 `detail.message`
  // 是英文原文（`source-local SQL shape precheck found N problem(s)`），属于 API 语义不动它。
  const shapeSubtitle = shapeFailed
    ? `六条形状规则中 ${failedShapeRuleCount(detail.shape_checks)} 条未通过。`
    : "六条形状规则已通过。";
  return (
    <div className="precheck-reports">
      <section className={shapeFailed ? "is-failed" : "is-passed"}>
        <header>
          <strong>SQL 形状预检</strong>
          <span>source 本地</span>
        </header>
        <p>{shapeSubtitle}</p>
        <DiagnosticTable
          columns={["规则", "结果", "说明"]}
          rows={detail.shape_checks.map((check) => [
            shapeRuleLabel(check.rule),
            check.passed ? "通过" : "未通过",
            shapeRuleDescription(check.rule, check.message),
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
          <p>
            未执行——没跑到这一段，<code>sink</code> 不知道存在这个运行。
          </p>
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
