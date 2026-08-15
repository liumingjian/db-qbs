import { Play, X } from "lucide-react";
import { useEffect, useState } from "react";
import type { FormEvent } from "react";

import { listRunHistory, startRun } from "./api";
import type { Task } from "./api";
import { messageFrom } from "./errors";

export function StartRunDialog({
  task,
  onClose,
  onStarted,
}: {
  task: Task;
  onClose: () => void;
  onStarted: (runRecordId: string) => void;
}) {
  const [bizDate, setBizDate] = useState("");
  const [possiblyRunning, setPossiblyRunning] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (bizDate === "") {
      setPossiblyRunning(false);
      return;
    }
    let active = true;
    void listRunHistory({ taskId: task.task_id, bizDate })
      .then((rows) => {
        if (active) {
          setPossiblyRunning(rows.some((row) => row.outcome === null));
        }
      })
      .catch(() => {
        if (active) {
          setPossiblyRunning(false);
        }
      });
    return () => {
      active = false;
    };
  }, [bizDate, task.task_id]);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      const accepted = await startRun(task.task_id, bizDate);
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
            <span className="modal-context mono">{task.task_id}</span>
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
            <label className="form-field">
              <span>业务日期</span>
              <input
                type="date"
                required
                value={bizDate}
                onChange={(event) => setBizDate(event.target.value)}
              />
            </label>
            {possiblyRunning && (
              <div className="stale-run-hint">
                <strong>该任务该业务日期可能已有一个 run 进行中。</strong>
                <span>这条提示可能滞后、不是门禁；真正的并发判断由后端在发起时完成。</span>
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
            <button className="button is-primary" type="submit">
              <Play size={15} aria-hidden="true" />
              {submitting ? "正在发起" : "发起"}
            </button>
          </footer>
        </form>
      </section>
    </div>
  );
}
