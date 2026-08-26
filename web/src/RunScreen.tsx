import { ArrowLeft, Ban, Play, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";

import { cancelRun, fetchRun } from "./api";
import type { RunDetail, Task } from "./api";
import {
  ErrorCodeTag,
  ICON,
  PhaseLine,
  SensitiveValue,
  TerminalBlock,
  UnknownConclusion,
  UpsertNote,
  UPSERT_NOTE_DONE,
} from "./components/DesignSystem";
import { messageFrom } from "./errors";
import { FailureEvidence } from "./FailureEvidence";
import { runIdPresentation } from "./history";
import { PrecheckReports } from "./PrecheckReports";
import { progressOfLiveRun } from "./progress";
import { runPresentation } from "./run";
import type { RunPresentation } from "./run";
import { abortRefusal } from "./runStage";
import type { Step } from "./wizard";

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
  onEditTask: (step: Step) => void;
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

  // 「已用时」要每秒自己走，不能等下一次轮询把它捎回来：轮询失败或页面切走时，
  // 屏幕上那个数会停在最后一次成功读取的时刻，而运行还在跑。
  const now = useNow(detail?.live === true);
  const hidden = typeof document === "undefined" ? false : document.hidden;
  const presentation = detail === null ? null : runPresentation(detail);
  // 停不停得了和服务端读同一条规则，所以按钮在点下去之前就知道答案——
  // 过去它一直亮着，人只有吃一个 409 才发现封口点已经过了。
  const cancelRefusal = detail === null ? null : abortRefusal(detail.stage);

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
            <ArrowLeft size={ICON.sm} aria-hidden="true" />
            返回任务
          </button>
          <h1 id="run-title">{task.name}</h1>
          <span className="card-subtitle mono">运行记录 · {runRecordId}</span>
        </div>
        {detail?.live === true ? (
          // 灰掉而不是隐藏：理由挂得住，消失挂不住。
          <span className="run-cancel">
            <button
              className="button is-ghost"
              type="button"
              disabled={cancelRefusal !== null}
              onClick={() => void handleCancel()}
            >
              <Ban size={ICON.sm} aria-hidden="true" />
              取消运行
            </button>
            {cancelRefusal !== null && (
              <span className="run-cancel-reason">{cancelRefusal}</span>
            )}
          </span>
        ) : (
          <button className="button is-primary" type="button" onClick={onRelaunch}>
            <Play size={ICON.sm} aria-hidden="true" />
            重新发起
          </button>
        )}
      </header>

      {loadError !== null && (
        <div className="run-notice is-error" role="alert">
          <span>{loadError}</span>
          {/* 这句话只在**真的暂停了**的时候才成立（UX 评审 P1-8）。轮询只在页面不可见时
              停；页面开着的时候它每秒都在重试，而这句读起来像「已经不试了」。 */}
          <span>{hidden ? "页面在后台，已暂停读取；切回来会继续。" : "正在重试。"}</span>
        </div>
      )}
      {cancelMessage !== null && (
        <div className="run-notice" role="status">{cancelMessage}</div>
      )}

      {detail === null || presentation === null ? (
        <div className="loading-state">
          <RefreshCw className="is-spinning" size={ICON.md} aria-hidden="true" />
          正在读取运行详情...
        </div>
      ) : (
        <div className="run-content">
          <RunIdentity task={task} detail={detail} />
          {detail.live ? (
            <LiveRun detail={detail} presentation={presentation} now={now} />
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

/**
 * 每秒一跳，只在需要的时候跳。
 *
 * 运行结束就不跳了——一个终局的运行时长是个定值，让它继续每秒重渲染一次整屏，
 * 只是白烧电。
 */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
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
      <DetailValue label="暂存表" value={detail.staging_table ?? "—"} />
    </dl>
  );
}

function LiveRun({
  detail,
  presentation,
  now,
}: {
  detail: RunDetail & { live: true };
  presentation: RunPresentation;
  /** 当前时刻，每秒一跳。墙钟时长与「最后动静」都从它算。 */
  now: number;
}) {
  const progress = progressOfLiveRun(detail);
  return (
    <>
      <section className={`live-state is-${presentation.kind}`}>
        <PhaseLine current={presentation.phase} />
        <div className="indeterminate-progress" aria-label="运行进行中">
          <span />
        </div>
        <div className="live-progress-row" title={progress.title}>
          <span>迁移进度</span>
          {progress.kind === "value" ? (
            <span className="progress is-live-detail">
              <span className="progress-track">
                <span
                  className={`progress-fill is-${progress.tone}`}
                  style={{ width: `${progress.percent}%` }}
                />
              </span>
              <span className="progress-pct">{progress.label}</span>
            </span>
          ) : (
            <span className="empty-value">{progress.label}</span>
          )}
        </div>
        <strong>{presentation.conclusion}</strong>
        {presentation.kind === "accepted" && (
          <span>已受理，正在启动。</span>
        )}
      </section>
      <dl className="run-metrics">
        <Metric label="已推行数" value={formatCount(detail.rows_pushed)} />
        <Metric
          label="总行数"
          value={detail.total_rows === null ? "—" : formatCount(detail.total_rows)}
        />
        <Metric label="当前批次序号" value={formatCount(detail.seq)} />
        {/* 「已用时」是**墙钟**，不是批次耗时的累加（UX 评审 P1-8）。开跑前计数那几十秒里
            一个批次都还没有，`ms` 是 0——原来这一格因此在一次真的在跑的运行上
            先写将近一分钟的「已用时 00:00」。`ms` 还在，只是叫回它本来的名字，
            与终局屏那一格同名。 */}
        <Metric label="已用时" value={formatElapsed(detail.started_at, now)} />
        <Metric label="最后动静" value={formatSince(detail.last_ts, now)} />
        <Metric label="累计批次耗时" value={formatDuration(detail.ms)} />
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
  onEditTask: (step: Step) => void;
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
        {presentation.terminalEffect === "SWAPPED" && (
          <UpsertNote text={UPSERT_NOTE_DONE} />
        )}
      </section>

      {(presentation.kind === "failed" ||
        presentation.kind === "mapping-failed" ||
        presentation.kind === "unknown") && (
        <FailureEvidence
          run={detail}
          variant={presentation.kind === "unknown" ? "unknown" : "failure"}
          onEditTask={onEditTask}
        />
      )}

      {mappingFailed && <PrecheckReports detail={detail} />}

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
      <UnknownConclusion
        reason={detail.unknown_reason}
        conclusion={presentation.conclusion}
      />
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

/** 从发起到此刻的墙钟时长。时间戳读不动时给横杠，不给一个 00:00。 */
function formatElapsed(startedAt: string, now: number): string {
  const started = Date.parse(startedAt);
  if (Number.isNaN(started)) {
    return "—";
  }
  return formatDuration(Math.max(0, now - started));
}

/**
 * 距离最后一个批次多久了——**卡住与慢的分界线**。
 *
 * 一次搬五十万行的运行看上去和一次已经僵在那里的运行没有区别：进度都不动，
 * 行数都不涨。差别只在这一个数上，而 `last_ts` 一直取回来了，只是没人显示它。
 */
function formatSince(lastTs: string | null, now: number): string {
  if (lastTs === null) {
    return "—";
  }
  const last = Date.parse(lastTs);
  if (Number.isNaN(last)) {
    return "—";
  }
  const seconds = Math.max(0, Math.floor((now - last) / 1000));
  return seconds < 60
    ? `${seconds} 秒前`
    : `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒前`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${formatCount(bytes)} B`;
  }
  return `${(bytes / 1024).toFixed(1)} KiB`;
}
