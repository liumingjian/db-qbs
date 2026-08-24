import { Play, X } from "lucide-react";
import { useEffect } from "react";

import type { RunHistory, Task } from "./api";
import {
  ErrorCodeTag,
  SensitiveValue,
  TerminalBlock,
} from "./components/DesignSystem";
import { formatTimestamp, historyPresentation, runIdPresentation } from "./history";
import { rerunAction } from "./rerun";
import { conditionSummary, runParamsSummary, sourceSummary } from "./spec";

/**
 * 运行详情抽屉——**顶替整个「运行历史」屏**（ADR-0043 §2 §4）。
 *
 * 它装的是这个任务**最近一次**运行的全部信息：原历史屏展开行里有什么，这里一样不少，
 * 外加原来散在列表上的主键 / 条件 / 错误码 / 目标表效果。
 *
 * 三条边界：
 *
 * 1. **三轴的形状一个没改**，只是换了住处（ADR-0025 §3）：轴二是 `TerminalBlock`、
 *    轴三是 `ErrorCodeTag`，与运行详情页逐字同一套组件。列表那一列是一维索引，不是轴二。
 * 2. **底部的「重跑」不许消失**（ADR-0043 文末自决 1）。历史屏是重跑原来的唯一入口，
 *    屏没了而入口没接住，等于顺手废掉 ADR-0041 增补 2 与规格 #149 A 段。
 *    给不给由 `rerunAction` 判，本组件只负责摆出来——失败与结局不明给，进行中与成功不给。
 * 3. **明说这是最近一次**。一个任务的多次历史本版不做（裁定 2 原话「另说」），
 *    不说清楚的话，看到的人会以为这个任务只跑过一次。
 */
export function RunDrawer({
  task,
  run,
  tasks,
  onClose,
  onRerun,
}: {
  task: Task;
  run: RunHistory;
  /** 传给 `rerunAction` 判「任务还在不在」；`null` = 任务清单没读到。 */
  tasks: Task[] | null;
  onClose: () => void;
  onRerun: (task: Task, row: RunHistory) => void;
}) {
  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const presentation = historyPresentation(run);
  const rerun = rerunAction(run, tasks);

  return (
    <>
      <div className="drawer-scrim" role="presentation" onMouseDown={onClose} />
      <aside
        className="drawer"
        role="dialog"
        aria-modal="true"
        aria-labelledby="drawer-title"
      >
        <header className="drawer-header">
          <button
            className="icon-button"
            type="button"
            title="关闭"
            aria-label="关闭"
            onClick={onClose}
          >
            <X size={18} aria-hidden="true" />
          </button>
          <h2 id="drawer-title">运行详情 · {task.name}</h2>
          <span className="sub">{run.run_record_id}</span>
        </header>

        <div className="drawer-body">
          <section className="panel">
            <h3>结论</h3>
            {presentation.kind === "unknown" ? (
              <div
                className={`unknown-conclusion is-${run.unknown_reason?.toLowerCase()}`}
              >
                <strong>结局不明</strong>
                <span>{presentation.conclusion}</span>
                <small>无法确认目标表是否被修改，请到目标库核对。</small>
              </div>
            ) : (
              <div className="panel-body">
                <div className="detail-status">
                  <span className="outcome-label">
                    运行结果 <strong>{run.outcome ?? "进行中"}</strong>
                  </span>
                  {presentation.terminalEffect !== null && (
                    <TerminalBlock effect={presentation.terminalEffect} />
                  )}
                  {run.target_table_effect === "UNKNOWN" && (
                    <span className="unknown-effect">UNKNOWN　目标表效果未知</span>
                  )}
                  {presentation.terminalEffect === null &&
                    run.target_table_effect !== null &&
                    run.target_table_effect !== "UNKNOWN" && (
                      <span className="effect-text">
                        目标表 <strong>{run.target_table_effect}</strong>
                      </span>
                    )}
                  {run.source_code !== null && (
                    <span className="source-code">
                      源端 <strong>{run.source_code}</strong>
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
                {presentation.kind === "succeeded" && (
                  <div className="success-conclusion">{presentation.conclusion}</div>
                )}
                {presentation.kind === "live" && (
                  <div className="drawer-note">{presentation.conclusion}</div>
                )}
              </div>
            )}
          </section>

          <section className="panel">
            <h3>行数核对</h3>
            <div className="panel-body kv">
              <Value label="源端读取" value={optionalCount(run.source_rows)} />
              <Value label="暂存写入" value={optionalCount(run.staged_rows)} />
              <Value
                label="目标端回报"
                value={optionalCount(run.sink_reported_rows)}
                bad={mismatched(run)}
              />
              <Value label="清理行数" value={optionalCount(run.purged_rows)} />
            </div>
          </section>

          <section className="panel">
            <h3>分段耗时</h3>
            <div className="panel-body kv">
              {/* 「开跑前计数」**单独一栏，不混进取数**（ADR-0043 §7）：把它揉进取数里，
                  下一个人看到的「取数慢」会是两件事的和。 */}
              <Value label="开跑前计数" value={milliseconds(run.precount_ms)} />
              <Value label="取数" value={milliseconds(run.fetch_ms)} />
              <Value label="推送" value={milliseconds(run.push_ms)} />
              <Value label="提交" value={milliseconds(run.commit_ms)} />
              <Value label="门禁计数" value={milliseconds(run.count_ms)} />
              <Value label="开游标" value={milliseconds(run.cursor_ms)} />
            </div>
          </section>

          <section className="panel">
            <h3>任务定义</h3>
            {/* 主键与条件从列表挪进来（ADR-0043 §4）：它们是**任务**的属性，
                每次运行都一样，占着列表一整列换不来任何区分度。 */}
            <div className="panel-body kv is-pairs">
              {/* 自定义 SQL 的任务 owner / table 都是空串，直接拼会打出一个裸点。
                  与作业中心同一口径（sourceSummary）：这里给截断的一行，
                  全文进 title——完整语句在下面「当次执行的源端 SQL」那一面板里。 */}
              <Value
                label="源表"
                value={sourceSummary(task.spec).label}
                title={sourceSummary(task.spec).full}
              />
              <Value label="目标表" value={task.spec.target_table} />
              <Value
                label="主键"
                value={task.spec.primary_key.join(", ") || "—"}
              />
              <Value label="条件" value={conditionSummary(task.spec)} />
            </div>
          </section>

          <section className="panel">
            <h3>运行参数与标识</h3>
            <div className="panel-body kv is-pairs">
              <Value label="运行记录" value={run.run_record_id} />
              {/* V15：没有 `run_id` 时写一句话，不是空白也不是横杠；两个 id 谁也不替代谁。 */}
              <Value label="目标端运行号" value={runIdPresentation(run)} />
              <Value label="发起于" value={formatTimestamp(run.started_at, true)} />
              <Value label="结束于" value={formatTimestamp(run.finished_at, true)} />
              <Value label="运行参数" value={runParamsSummary(run.run_params)} />
              <Value label="暂存表" value={run.staging_table ?? "—"} />
            </div>
          </section>

          <section className="panel">
            <h3>当次执行的源端 SQL</h3>
            <pre className="drawer-sql">{run.source_sql}</pre>
          </section>

          {(run.column !== null || run.value !== null) &&
            presentation.kind !== "unknown" && (
              <SensitiveValue
                column={run.column ?? undefined}
                value={run.value ?? undefined}
              />
            )}
        </div>

        <footer className="drawer-footer">
          <span className="drawer-note">
            这一条已是最近一次运行；同一个任务的多次历史本版不展示。
          </span>
          <span className="spacer" />
          <button className="button is-ghost" type="button" onClick={onClose}>
            关闭
          </button>
          {rerun.kind === "enabled" && (
            <button
              className="button is-primary"
              type="button"
              title="重跑：按这次的运行参数预填发起对话框"
              onClick={() => onRerun(rerun.task, run)}
            >
              <Play size={14} aria-hidden="true" />
              重跑
            </button>
          )}
          {rerun.kind === "disabled" && (
            // 该有入口但按不动时**不让它消失**——凭空消失会被读成「功能坏了」。
            // 原因挂在外层 `span` 的 `title` 上：浏览器不给 `disabled` 控件派发指针事件。
            <span title={`重跑（不可用）：${rerun.reason}`}>
              <button
                className="button is-primary"
                type="button"
                aria-label={`重跑（不可用）：${rerun.reason}`}
                disabled
              >
                <Play size={14} aria-hidden="true" />
                重跑
              </button>
            </span>
          )}
        </footer>
      </aside>
    </>
  );
}

function Value({
  label,
  value,
  bad = false,
  title,
}: {
  label: string;
  value: string;
  bad?: boolean;
  /** 值被截断时挂全文；不传就不加属性，避免给短值也挂一份重复的 tooltip。 */
  title?: string;
}) {
  return (
    <div>
      <span className="k">{label}</span>
      <span
        className={`v ${bad ? "is-bad" : ""}`}
        title={title === value ? undefined : title}
      >
        {value}
      </span>
    </div>
  );
}

/** 目标端回报与暂存写入对不上时把那个数染红——门禁判死的正是这一处差额。 */
function mismatched(run: RunHistory): boolean {
  const staged = run.staged_rows;
  const reported = run.sink_reported_rows;
  return staged !== null && reported !== null && staged !== reported;
}

const countFormatter = new Intl.NumberFormat("zh-CN");

function optionalCount(value: number | null): string {
  return value === null ? "—" : countFormatter.format(value);
}

function milliseconds(value: number | null): string {
  return value === null ? "—" : `${countFormatter.format(value)} ms`;
}
