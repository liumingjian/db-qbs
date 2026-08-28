import { Pencil, Radio, RefreshCw, Trash2 } from "lucide-react";
import { useState } from "react";
import { ICON } from "./components/DesignSystem";
import type { FormEvent } from "react";

import {
  deleteAgent,
  probeAgent,
  referencedDatasourcesFrom,
  registerAgent,
  updateAgent,
} from "./api";
import type { Agent, AgentInput, AgentStatus, Datasource } from "./api";
import { messageFrom } from "./errors";
import { formatTimestamp } from "./history";
import { ActionButton, FormField, Modal, ModalFooter } from "./ui";

/**
 * 目标端 Agent 屏（ADR-0044 §6）——导航第一项。
 *
 * **它排在数据源前面是有顺序含义的**：一条 MySQL 数据源必须绑一台已注册的 agent，
 * 所以「先把 agent 注册上」是新装一台机器之后的第一件事，而不是数据源里的一个附属字段。
 *
 * 这一屏与数据源屏有一处**故意的不对称**：数据源屏没有「连接状态」列（ADR-0039 §2：
 * 要么后台轮询所有库、要么显示一个过期的绿点），这一屏**有**。理由是两者性质不同——
 * agent 是**本产品自己的进程**，探它只要一个 `GET /v1/agent/info`，不碰任何业务库、
 * 不占任何数据库连接；而「这台 agent 现在活着吗」正是本屏存在的全部意义：
 * 现场撞到的那件事（把 agent 停掉、同步照跑）在界面上必须看得见。
 */
type AgentDialogState =
  | { kind: "register" }
  | { kind: "edit"; agent: Agent }
  | { kind: "delete"; agent: Agent }
  | null;

const STATUS_LABELS: Record<AgentStatus, string> = {
  online: "在线",
  offline: "不在线",
  mismatch: "身份不符",
};

/**
 * 状态色沿用列表里那套 `.state` 标签，不新开样式（走查成本）。
 *
 * **`mismatch` 用的是红色、`offline` 用的是灰色**，不是反过来：不在线是「东西没起来」，
 * 身份不符是「这个地址后面站着的不是你以为的那台」——后者更该被当成事故。
 */
const STATUS_CLASS: Record<AgentStatus, string> = {
  online: "is-succeeded",
  offline: "is-unknown",
  mismatch: "is-failed",
};

export function AgentScreen({
  agents,
  datasources,
  loading,
  onChanged,
}: {
  agents: Agent[];
  datasources: Datasource[];
  loading: boolean;
  onChanged: () => Promise<void>;
}) {
  const [dialog, setDialog] = useState<AgentDialogState>(null);
  const [probing, setProbing] = useState<string | null>(null);

  async function probe(agent: Agent) {
    setProbing(agent.agent_id);
    try {
      await probeAgent(agent.agent_id);
      await onChanged();
    } catch {
      // 探测失败也是 200，走不到这里；真走到了说明请求本身没发出去，
      // 下一次列表刷新会把状态带回来，不必在这里再编一句话。
      await onChanged();
    } finally {
      setProbing(null);
    }
  }

  const counts = new Map<string, number>();
  for (const datasource of datasources) {
    if (datasource.kind === "mysql") {
      counts.set(
        datasource.agent_id,
        (counts.get(datasource.agent_id) ?? 0) + 1,
      );
    }
  }

  return (
    <section className="card" id="agents" aria-labelledby="agents-title">
      <header className="card-header">
        <div>
          <h1 id="agents-title">目标端 Agent</h1>
          <span className="card-subtitle">
            {loading ? "正在读取" : `共 ${agents.length} 台`}
          </span>
        </div>
        <button
          className="button is-primary"
          type="button"
          onClick={() => setDialog({ kind: "register" })}
        >
          <Radio size={ICON.sm} aria-hidden="true" />
          注册 Agent
        </button>
      </header>

      {loading && agents.length === 0 && (
        <div className="loading-state" aria-live="polite">
          正在加载目标端 agent...
        </div>
      )}

      {!loading && agents.length === 0 && (
        <div className="empty-state">
          <div className="empty-icon">
            <Radio size={ICON.empty} aria-hidden="true" />
          </div>
          <h2>还没有注册目标端 Agent</h2>
          <p>
            先在目标端主机上把 agent 起起来，再用它的地址在这里注册；
            MySQL 数据源只能经已注册的 agent 访问。
          </p>
          <button
            className="button is-primary"
            type="button"
            onClick={() => setDialog({ kind: "register" })}
          >
            <Radio size={ICON.sm} aria-hidden="true" />
            注册 Agent
          </button>
        </div>
      )}

      {agents.length > 0 && (
        <div className="table-wrap">
          <table className="data-grid">
            <thead>
              <tr>
                <th>名称</th>
                <th>地址</th>
                <th>状态</th>
                <th>身份</th>
                <th>版本</th>
                <th>MySQL</th>
                <th>最近可见</th>
                <th>被引用</th>
                <th className="action-column">操作</th>
              </tr>
            </thead>
            <tbody>
              {agents.map((agent) => {
                const count = counts.get(agent.agent_id) ?? 0;
                return (
                  <tr key={agent.agent_id}>
                    <td>
                      <span className="task-name">{agent.name}</span>
                      <span className="task-id">{agent.agent_id}</span>
                    </td>
                    <td className="mono">{agent.base_url}</td>
                    <td>
                      <span
                        className={`state ${STATUS_CLASS[agent.status]}`}
                        // 失败原因挂在 `title` 上：列表要窄，但那句人话不该只能靠猜。
                        title={agent.last_error ?? STATUS_LABELS[agent.status]}
                      >
                        {STATUS_LABELS[agent.status]}
                      </span>
                      {agent.status !== "online" && agent.last_error !== null && (
                        <span className="row-test-result is-failed" role="alert">
                          {agent.last_error}
                        </span>
                      )}
                    </td>
                    {/* 身份只显示前 8 位：它是给人比对「换没换过」的，不是给人抄的。 */}
                    <td className="mono">
                      {agent.instance_id === ""
                        ? "未探测"
                        : agent.instance_id.slice(0, 8)}
                    </td>
                    <td className="mono">
                      {agent.version === "" ? "—" : agent.version}
                    </td>
                    {/* agent 连的那台 MySQL（#257）。**没报过就写「未知」**，不是「8.0」：
                        agent 自己不持有目标库凭据，要等它经手过一次目标端检查或一次开跑
                        才知道；旧版本 agent 则永远报不出来。字符序挂在 `title` 上——
                        它是建表语句要用的那一项，列表里摊开会把这一栏撑爆。 */}
                    <td
                      className="mono"
                      title={
                        agent.mysql_collation === null
                          ? "这台 agent 还没报过它所连 MySQL 的字符序"
                          : `utf8mb4 默认字符序：${agent.mysql_collation}`
                      }
                    >
                      {agent.mysql_version === null ? "未知" : agent.mysql_version}
                    </td>
                    {/* 与列表屏同一种时间格式（`formatTimestamp`）：同一个时刻在两屏上
                        不该长得不一样，原始的 RFC 3339 串还会在窄列里从中间折断。 */}
                    <td>
                      {agent.last_seen_at === null
                        ? "从未"
                        : formatTimestamp(agent.last_seen_at, true)}
                    </td>
                    <td>{count === 0 ? "未被引用" : `${count} 条数据源`}</td>
                    <td className="action-column">
                      <div className="row-actions">
                        <ActionButton
                          label={
                            probing === agent.agent_id ? "正在探测" : "探测"
                          }
                          icon={
                            <RefreshCw
                              className={
                                probing === agent.agent_id ? "is-spinning" : ""
                              }
                              size={ICON.md}
                            />
                          }
                          disabled={probing === agent.agent_id}
                          onClick={() => void probe(agent)}
                        />
                        <span className="divider" />
                        <ActionButton
                          label="编辑 Agent"
                          icon={<Pencil size={ICON.md} />}
                          onClick={() => setDialog({ kind: "edit", agent })}
                        />
                        <span className="divider" />
                        <ActionButton
                          label="删除 Agent"
                          danger
                          icon={<Trash2 size={ICON.md} />}
                          onClick={() => setDialog({ kind: "delete", agent })}
                        />
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {dialog?.kind === "register" && (
        <AgentFormDialog
          title="注册目标端 Agent"
          existing={null}
          onClose={() => setDialog(null)}
          onChanged={onChanged}
        />
      )}
      {dialog?.kind === "edit" && (
        <AgentFormDialog
          title={`编辑 · ${dialog.agent.name}`}
          existing={dialog.agent}
          onClose={() => setDialog(null)}
          onChanged={onChanged}
        />
      )}
      {dialog?.kind === "delete" && (
        <AgentDeleteDialog
          agent={dialog.agent}
          onClose={() => setDialog(null)}
          onChanged={onChanged}
        />
      )}
    </section>
  );
}

/**
 * 注册 / 编辑（ADR-0044 §3）。
 *
 * **没有「测试连接」按钮，因为提交本身就是一次连接**：服务端探不通就不落库，
 * 所以「保存成功」这四个字在这里的含义就是「刚才那一刻它活着、身份已钉住」。
 * 多摆一个测连按钮只是让人多点一次，买不到任何新信息。
 */
function AgentFormDialog({
  title,
  existing,
  onClose,
  onChanged,
}: {
  title: string;
  existing: Agent | null;
  onClose: () => void;
  onChanged: () => Promise<void>;
}) {
  const [draft, setDraft] = useState<AgentInput>(() => ({
    name: existing?.name ?? "",
    base_url: existing?.base_url ?? "http://127.0.0.1:8080",
  }));
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      if (existing === null) {
        await registerAgent(draft);
      } else {
        await updateAgent(existing.agent_id, draft);
      }
      await onChanged();
      onClose();
    } catch (submitError) {
      setError(messageFrom(submitError));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Modal title={title} onClose={onClose} busy={submitting} narrow>
      <form onSubmit={(event) => void handleSubmit(event)}>
        <div className="modal-body form-stack">
          <FormField label="名称">
            <input
              autoFocus
              value={draft.name}
              placeholder="留空则用 agent 自报的名字"
              onChange={(event) =>
                setDraft((current) => ({ ...current, name: event.target.value }))
              }
            />
          </FormField>
          <FormField label="地址">
            <input
              required
              value={draft.base_url}
              placeholder="如 http://127.0.0.1:8080"
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  base_url: event.target.value,
                }))
              }
            />
          </FormField>
          <p className="drawer-note">
            填的是 source 这台机器能打通的 agent 地址。走隧道时它通常就是本机回环上的隧道入口
            （如 <code>http://127.0.0.1:8080</code>）。协议只收 http——机密性由部署者自建的隧道提供。
          </p>
          <p className="drawer-note">
            保存即连接：连不通就不会存下来。存下来之后，这台 agent 自报的身份会被钉在这条记录上；
            日后同一个地址上换了另一台 agent 应答，状态会变成「身份不符」，而不是照常放行。
          </p>
          {error !== null && (
            <div className="form-error" role="alert">
              {error}
            </div>
          )}
        </div>
        <ModalFooter
          onClose={onClose}
          busy={submitting}
          submitLabel={existing === null ? "注册" : "保存"}
        />
      </form>
    </Modal>
  );
}

/** 删除确认。被数据源引用时服务端回 409，这里把它点名列出来（同 ADR-0039 §4）。 */
function AgentDeleteDialog({
  agent,
  onClose,
  onChanged,
}: {
  agent: Agent;
  onClose: () => void;
  onChanged: () => Promise<void>;
}) {
  const [error, setError] = useState<string | null>(null);
  const [referenced, setReferenced] = useState<string[]>([]);
  const [submitting, setSubmitting] = useState(false);

  async function handleDelete() {
    setSubmitting(true);
    setError(null);
    try {
      await deleteAgent(agent.agent_id);
      await onChanged();
      onClose();
    } catch (deleteError) {
      setReferenced(referencedDatasourcesFrom(deleteError));
      setError(messageFrom(deleteError));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Modal title="删除目标端 Agent" onClose={onClose} busy={submitting} narrow>
      <div className="modal-body delete-copy">
        <p>
          确认删除 agent“<strong>{agent.name}</strong>”？绑着它的数据源必须改绑别的 agent
          才能继续用。
        </p>
        <span className="task-id">{agent.base_url}</span>
        {error !== null && (
          <div className="form-error" role="alert">
            {error}
          </div>
        )}
        {referenced.length > 0 && (
          <ul>
            {referenced.map((name) => (
              <li key={name}>{name}</li>
            ))}
          </ul>
        )}
      </div>
      <footer className="modal-footer">
        <button
          className="button is-ghost"
          type="button"
          onClick={onClose}
          disabled={submitting}
        >
          取消
        </button>
        <button
          className="button is-danger"
          type="button"
          onClick={() => void handleDelete()}
          disabled={submitting}
        >
          {submitting ? "正在删除" : "删除"}
        </button>
      </footer>
    </Modal>
  );
}
