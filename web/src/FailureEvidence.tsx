import { Pencil } from "lucide-react";

import type { RunHistory } from "./api";
import { remediationFor } from "./troubleshooting";
import type { Step } from "./wizard";

export function FailureEvidence({
  run,
  onEditTask,
}: {
  run: RunHistory;
  onEditTask: (step: Step) => void;
}) {
  const remediation = remediationFor(run);
  const { source, target, agent, parameters } = run.evidence ?? {};
  const missing = source == null || target == null || agent == null || parameters == null;

  return (
    <section className="failure-evidence" aria-labelledby={`evidence-${run.run_record_id}`}>
      <div className="failure-evidence-heading">
        <div>
          <h3 id={`evidence-${run.run_record_id}`}>当次运行证据</h3>
          <p>以下连接与参数在发起时固定，不随后续配置修改而变化。</p>
        </div>
        {remediation !== null && (
          <button
            className="button is-ghost"
            type="button"
            onClick={() => onEditTask(remediation.step)}
          >
            <Pencil size={15} aria-hidden="true" />
            {remediation.label}
          </button>
        )}
      </div>
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
          <pre className="evidence-sql">{parameters.source_sql}</pre>
        </>
      )}
    </section>
  );
}

function EvidenceValue({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}
