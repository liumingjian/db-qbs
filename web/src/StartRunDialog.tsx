import { Play, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";

import { listRunHistory, startRun } from "./api";
import type { Condition, RunHistory, RunParams, Task } from "./api";
import { messageFrom } from "./errors";
import { historyPresentation } from "./history";
import { rerunPrefill } from "./rerun";
import { comparisonSymbol, runtimeConditions, sameRunParams } from "./spec";

/** 已跑多久。提示条本身就自称可能滞后，所以这里给个粗粒度就够，不做秒级跳动。 */
function elapsedSince(startedAt: string): string {
  const startedMs = Date.parse(startedAt);
  if (Number.isNaN(startedMs)) {
    return "时长未知";
  }
  const minutes = Math.max(0, Math.floor((Date.now() - startedMs) / 60000));
  if (minutes < 1) {
    return "不到 1 分钟";
  }
  if (minutes < 60) {
    return `${minutes} 分钟`;
  }
  return `${Math.floor(minutes / 60)} 小时 ${minutes % 60} 分钟`;
}

const INPUT_TYPES: Readonly<Record<Condition["value_type"], string>> = {
  text: "text",
  number: "number",
  date: "date",
};

/**
 * 发起运行。重跑走的是**同一个**对话框（ADR-0041 增补 2）：区别只有初值——
 * `rerun` 非空时按那次的运行参数预填，确认键仍是「发起」，并发提示与后端闸门原样都在。
 */
export function StartRunDialog({
  task,
  rerun,
  onClose,
  onStarted,
}: {
  task: Task;
  rerun?: { runRecordId: string; runParams: RunParams };
  onClose: () => void;
  onStarted: (runRecordId: string) => void;
}) {
  const parameters = useMemo(() => runtimeConditions(task.spec), [task.spec]);
  const [values, setValues] = useState<RunParams>(() =>
    rerunPrefill(task.spec, rerun?.runParams ?? {}),
  );
  // 存的是那条进行中的记录本身，不是一个布尔——提示条要报出它的 run_record_id 和已跑时长，
  // 否则人只知道「可能有一个 run」，却没有任何办法去看它。
  const [runningRow, setRunningRow] = useState<RunHistory | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const allFilled = parameters.every(
    (condition) => (values[condition.parameter] ?? "") !== "",
  );

  useEffect(() => {
    if (!allFilled) {
      setRunningRow(null);
      return;
    }
    let active = true;
    // 逐个参数敲的时候不要每键一次就问一次后端——这条提示本来就自称可能滞后。
    const timer = window.setTimeout(() => {
      void listRunHistory({ taskId: task.task_id })
        .then((rows) => {
          if (active) {
            setRunningRow(
              rows.find(
                (row) =>
                  // 「进行中」问 `historyPresentation`，不自己看 `outcome === null`——
                  // 结局不明的行 `outcome` 也是 null，而它恰恰是**可以重跑**的那一类：
                  // 自己看会让重跑一条结局不明的记录时，提示条指着你刚点的那一条说
                  // 「可能已有一个 run 进行中」，而那个 run 早就没了（#150 审查所见）。
                  historyPresentation(row).kind === "live" &&
                  sameRunParams(row.run_params, values),
              ) ?? null,
            );
          }
        })
        .catch(() => {
          if (active) {
            setRunningRow(null);
          }
        });
    }, 250);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [allFilled, task.task_id, values]);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      const accepted = await startRun(task.task_id, values);
      onStarted(accepted.run_record_id);
    } catch (startError) {
      setError(messageFrom(startError));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section className="modal is-narrow" role="dialog" aria-modal="true">
        <header className="modal-header">
          <div>
            <h2>发起运行 · {task.name}</h2>
            <span className="modal-context mono">
              {task.task_id}
              {rerun !== undefined && ` · 重跑自 ${rerun.runRecordId}`}
            </span>
          </div>
          <button
            className="icon-button"
            type="button"
            aria-label="关闭"
            title="关闭"
            onClick={onClose}
          >
            <X size={16} aria-hidden="true" />
          </button>
        </header>
        <form onSubmit={(event) => void handleSubmit(event)}>
          <div className="modal-body form-stack">
            {parameters.length === 0 ? (
              <p className="run-params-empty">
                这个任务没有「运行时填」的条件，发起时无需取值。
                <span>同一任务不允许同时运行两次。</span>
              </p>
            ) : (
              <div className="run-params" aria-label="运行参数">
                {parameters.map((condition) => (
                  <label className="form-field" key={condition.parameter}>
                    <span className="field-label">
                      {condition.parameter}
                      <span className="field-badge mono">
                        {condition.column} {comparisonSymbol(condition.operator)}
                      </span>
                    </span>
                    <input
                      type={INPUT_TYPES[condition.value_type]}
                      required
                      value={values[condition.parameter] ?? ""}
                      onChange={(event) =>
                        setValues((current) => ({
                          ...current,
                          [condition.parameter]: event.target.value,
                        }))
                      }
                    />
                  </label>
                ))}
              </div>
            )}
            {runningRow !== null && (
              <div className="stale-run-hint">
                <strong>该任务以同一组运行参数可能已有一次运行正在进行。</strong>
                <span className="mono">
                  {runningRow.run_record_id} · 已跑 {elapsedSince(runningRow.started_at)}
                </span>
                <span>状态可能有延迟，以发起结果为准。</span>
              </div>
            )}
            {error !== null && (
              <div className="form-error" role="alert">
                {error}
              </div>
            )}
          </div>
          <footer className="modal-footer">
            <button className="button is-ghost" type="button" onClick={onClose}>
              取消
            </button>
            <button className="button is-primary" type="submit" disabled={submitting}>
              <Play size={15} aria-hidden="true" />
              {submitting ? "正在发起" : "发起"}
            </button>
          </footer>
        </form>
      </section>
    </div>
  );
}
