import { Database, Pencil } from "lucide-react";
import { ICON } from "./components/DesignSystem";

import type { RunHistory } from "./api";
import { HighlightedSql } from "./SqlEditor";
import { remediationFor } from "./troubleshooting";
import type { Step } from "./wizard";

/**
 * 当次运行的连接与参数快照。
 *
 * **两种用法，同一份证据**（2026-08 UX 评审 P0-4）：
 *
 * - `failure` — 失败了，看是哪一侧的哪一项配错了。
 * - `unknown` — 结局不明。这一档原来**根本不出**：两个渲染点都写着
 *   `kind === "failed"`，于是唯一一种「界面自己承认不知道发生了什么」的结局，
 *   反而是唯一一种拿不到核对线索的结局。而它恰恰是最需要线索的：要去哪台机器、
 *   哪个库、哪张表、哪张暂存表上，亲手核对一遍。
 *
 * 结局不明那一档多两栏（暂存表、最后已知行数）——那是**只有这一档才用得上**的两样东西：
 * 暂存表还在不在，直接说明那次运行走到了哪一步。
 */
export function FailureEvidence({
  run,
  variant = "failure",
  onEditTask,
}: {
  run: RunHistory;
  variant?: "failure" | "unknown";
  onEditTask: (step: Step) => void;
}) {
  const remediation = remediationFor(run);
  const { source, target, agent, parameters } = run.evidence ?? {};
  const missing = source == null || target == null || agent == null || parameters == null;
  const unknown = variant === "unknown";
  const lastKnownRows =
    run.sink_reported_rows ?? run.staged_rows ?? run.rows_pushed;

  return (
    <section
      className={`failure-evidence ${unknown ? "is-clues" : ""}`}
      aria-labelledby={`evidence-${run.run_record_id}`}
    >
      <div className="failure-evidence-heading">
        <div>
          <h3 id={`evidence-${run.run_record_id}`}>{unknown ? "核对线索" : "当次运行证据"}</h3>
          <p>
            {unknown
              ? "照下面这份连接与参数去目标库核对——它们在发起时就固定了，不随后续配置修改而变化。"
              : "以下连接与参数在发起时固定，不随后续配置修改而变化。"}
          </p>
        </div>
        {/* 下一步该去哪儿（UX 评审 P1-11）。「没有可改的地方」也要说出口——
            十六个失败分类里有八个原来在这里什么都不出。 */}
        {remediation?.kind === "wizard" && (
          <button
            className="button is-ghost"
            type="button"
            onClick={() => onEditTask(remediation.step)}
          >
            <Pencil size={ICON.sm} aria-hidden="true" />
            {remediation.label}
          </button>
        )}
        {/* 数据源屏是一条真地址，所以这里给的是**链接**不是回调：抽屉与整屏两个宿主
            都不必替它转发一次导航，而它也因此可以被中键点开。 */}
        {remediation?.kind === "datasources" && (
          <a className="button is-ghost" href="#datasources">
            <Database size={ICON.sm} aria-hidden="true" />
            {remediation.label}
          </a>
        )}
        {remediation?.kind === "none" && (
          <p className="remediation-none">{remediation.reason}</p>
        )}
      </div>
      {/* 「重跑是安全的」要**在证据上面**：不知道发生了什么的时候，第一反应是不敢动。
          写入是按主键 upsert 的，幂等——这句先说，人才读得进下面那一堆地址。 */}
      {unknown && (
        <>
          <p className="clue-safety">
            <strong>重跑是安全的</strong>——写入是按主键幂等的，重跑不会写重。
          </p>
          {/* 这两栏**不在下面那份连接快照里**，是故意的：它们讲的是「这次跑到哪儿了」，
              不是「连的是哪台机器」。摆进快照里的话，一条没有快照的旧记录
              （最可能没有的正是 PROCESS_DISAPPEARED）会把它们一起吞掉，
              而那恰恰是最需要它们的一次。 */}
          <dl className="evidence-grid is-clue-facts">
            <EvidenceValue label="暂存表" value={run.staging_table ?? "—"} />
            <EvidenceValue
              label="最后已知行数"
              value={countFormatter.format(lastKnownRows)}
            />
          </dl>
        </>
      )}
      {missing ? (
        <div className="drawer-note">此运行记录创建时尚未记录连接快照。</div>
      ) : (
        <>
          <dl className="evidence-grid">
            <EvidenceValue label="源数据源" value={source.datasource_id} />
            <EvidenceValue label="Oracle 连接" value={source.connect_string} />
            <EvidenceValue label="Oracle 用户" value={source.username} />
            <EvidenceValue label="Oracle 客户端" value={source.client_lib_dir} />
            <EvidenceValue label="目标数据源" value={target.datasource_id} />
            <EvidenceValue label="MySQL 地址" value={`${target.host}:${target.port}`} />
            <EvidenceValue label="MySQL 库 / 用户" value={`${target.database} / ${target.username}`} />
            <EvidenceValue label="目标端 Agent" value={`${agent.name} · ${agent.base_url}`} />
            <EvidenceValue label="Agent 记录 / 实例" value={`${agent.agent_id} / ${agent.instance_id || "未钉住"}`} />
            <EvidenceValue label="目标表" value={parameters.target_table} />
            <EvidenceValue label="主键" value={parameters.primary_key.join(", ") || "—"} />
            <EvidenceValue
              label="字段映射"
              value={parameters.columns.map((column) => `${column.source} → ${column.target}`).join(", ") || "—"}
            />
          </dl>
          <pre className="evidence-sql"><HighlightedSql sql={parameters.source_sql} /></pre>
        </>
      )}
    </section>
  );
}

const countFormatter = new Intl.NumberFormat("zh-CN");

function EvidenceValue({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}
