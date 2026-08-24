import { Database, Pencil, Plus, PlugZap, Trash2 } from "lucide-react";
import { useMemo, useState } from "react";
import type { FormEvent } from "react";

import {
  createDatasource,
  deleteDatasource,
  referencedTasksFrom,
  testDatasourceDraft,
  updateDatasource,
} from "./api";
import type {
  Agent,
  Datasource,
  DatasourceInput,
  DatasourceKind,
  Task,
} from "./api";
import {
  agentLabel,
  agentStatusOf,
  canSaveDatasource,
  connectionFingerprint,
  connectionSummary,
  deleteRefusalMessage,
  draftFrom,
  referenceCounts,
} from "./datasource";
import { messageFrom } from "./errors";
import { ActionButton, FormField, Modal, ModalFooter } from "./ui";

/**
 * 一行「测试连接」的三态。与对话框里那份同形，但**各存各的**：
 * 对话框那份还兼着「测通才让存」的门槛，行内这份纯粹是一次问答。
 */
type RowTestState =
  | { kind: "testing" }
  | { kind: "ok"; elapsedMs: number; label: string }
  | { kind: "failed"; message: string };

/**
 * 数据源管理屏（ADR-0039 §1~§4）。
 *
 * ADR-0037 §5 判废了 ADR-0024 §2 的「UI 不提供连接配置的读写面」，这一屏就是那条判废的兑现。
 * 形态全部由 ADR-0039 定死，本屏不重新设计：
 *
 * - **不给搜索框、不给类型筛选**——现场只有 3~5 条，一屏看得完（所有者 2026-08-19 裁定 1）。
 *   任务屏那条工具条是因为任务会长到几十个才成立的，不要照抄。
 * - **不给「连接状态」列**——要么后台轮询所有库，要么显示一个过期的绿点，两者都比不显示更坏
 *   （同 ADR-0026「禁用状态本身会说谎」）。连不连得上，点「测试连接」当场问。
 * - **给「被引用」列**——那份计数为了判删除的 409 本来就要查，摆出来是零成本的，
 *   而它正好回答「这条能不能删」。
 */
type DatasourceDialogState =
  | { kind: "create" }
  | { kind: "edit"; datasource: Datasource }
  | { kind: "delete"; datasource: Datasource }
  | null;

export function DatasourceScreen({
  datasources,
  agents,
  tasks,
  loading,
  onChanged,
}: {
  datasources: Datasource[];
  /** 注册表里那几台 agent（ADR-0044 §6）：MySQL 那一列显示名字，表单里是必选项。 */
  agents: Agent[];
  tasks: Task[];
  loading: boolean;
  onChanged: () => Promise<void>;
}) {
  const [dialog, setDialog] = useState<DatasourceDialogState>(null);
  /**
   * 行内测连结果，按数据源 id 存（P1）。**瞬态**：不进库、不轮询、刷新即没。
   *
   * 这不是 ADR-0039 §2 判掉的那个「连接状态」列——那一条判的是**常驻**列，
   * 它要么后台轮询所有库，要么显示一个过期的绿点。这里显示的是**你刚才亲手问的那一次**，
   * 有明确的提问时刻，不会替一分钟前的事实背书。
   */
  const [rowTests, setRowTests] = useState<Record<string, RowTestState>>({});

  const counts = useMemo(() => referenceCounts(tasks), [tasks]);

  function clearRowTest(datasourceId: string) {
    setRowTests((current) => {
      if (!(datasourceId in current)) {
        return current;
      }
      const next = { ...current };
      delete next[datasourceId];
      return next;
    });
  }

  /**
   * 行内测连走的是**草稿测连那条端点**，不是按 id 那条。
   *
   * 理由只有一个：按 id 那条只回 `{ ok: true }`，凑不出「连接成功 · 186 ms · dw_stage」
   * 这一行——耗时与库名都在草稿那条里。口令传空串，服务端按「留空 = 去库里取那一份」
   * 解释（ADR-0037 §5），与保存面同一条规则，界面仍然一个字的口令都没回读。
   */
  async function testRow(datasource: Datasource) {
    const id = datasource.datasource_id;
    setRowTests((current) => ({ ...current, [id]: { kind: "testing" } }));
    try {
      const result = await testDatasourceDraft(draftFrom(datasource), id);
      setRowTests((current) => ({
        ...current,
        [id]: { kind: "ok", elapsedMs: result.elapsed_ms, label: result.label },
      }));
    } catch (testError) {
      // 原样回显驱动报错，**不出错误码标签**——测连不属于任何 run（ADR-0039 §3）。
      setRowTests((current) => ({
        ...current,
        [id]: { kind: "failed", message: messageFrom(testError) },
      }));
    }
  }

  return (
    <section className="card" id="datasources" aria-labelledby="datasources-title">
      <header className="card-header">
        <div>
          <h1 id="datasources-title">数据源</h1>
          <span className="card-subtitle">
            {loading ? "正在读取" : `共 ${datasources.length} 个`}
          </span>
        </div>
        <button
          className="button is-primary"
          type="button"
          onClick={() => setDialog({ kind: "create" })}
        >
          <Plus size={15} aria-hidden="true" />
          新建数据源
        </button>
      </header>

      {loading && datasources.length === 0 && (
        <div className="loading-state" aria-live="polite">
          正在加载数据源...
        </div>
      )}

      {!loading && datasources.length === 0 && (
        <div className="empty-state">
          <div className="empty-icon">
            <Database size={22} aria-hidden="true" />
          </div>
          <h2>还没有数据源</h2>
          <p>先录一条 Oracle 源库与一条 MySQL 目标库，任务才有得选。</p>
          <button
            className="button is-primary"
            type="button"
            onClick={() => setDialog({ kind: "create" })}
          >
            <Plus size={15} aria-hidden="true" />
            新建数据源
          </button>
        </div>
      )}

      {datasources.length > 0 && (
        <DatasourceTable
          datasources={datasources}
          agents={agents}
          referenceCounts={counts}
          rowTests={rowTests}
          onTest={testRow}
          onAction={setDialog}
        />
      )}

      {dialog?.kind === "create" && (
        <DatasourceFormDialog
          title="新建数据源"
          existing={null}
          agents={agents}
          onClose={() => setDialog(null)}
          onChanged={onChanged}
        />
      )}
      {dialog?.kind === "edit" && (
        <DatasourceFormDialog
          title={`编辑 · ${dialog.datasource.name}`}
          existing={dialog.datasource}
          agents={agents}
          onClose={() => setDialog(null)}
          onChanged={async () => {
            // 连接字段可能刚被改过，上一次的行内结果当场作废。
            clearRowTest(dialog.datasource.datasource_id);
            await onChanged();
          }}
        />
      )}
      {dialog?.kind === "delete" && (
        <DatasourceDeleteDialog
          datasource={dialog.datasource}
          onClose={() => setDialog(null)}
          onChanged={async () => {
            clearRowTest(dialog.datasource.datasource_id);
            await onChanged();
          }}
        />
      )}
    </section>
  );
}

const KIND_LABELS: Record<DatasourceKind, string> = {
  oracle: "Oracle",
  mysql: "MySQL",
};

function DatasourceTable({
  datasources,
  agents,
  referenceCounts,
  rowTests,
  onTest,
  onAction,
}: {
  datasources: Datasource[];
  agents: Agent[];
  referenceCounts: Map<string, number>;
  rowTests: Record<string, RowTestState>;
  onTest: (datasource: Datasource) => void;
  onAction: (dialog: DatasourceDialogState) => void;
}) {
  return (
    <div className="table-wrap">
      <table className="data-grid">
        <thead>
          <tr>
            <th>名称</th>
            <th>类型</th>
            <th>连接</th>
            {/* 「目标端 Agent」列（ADR-0044 §6）：这条库经哪台 agent 访问，此刻通不通。
                它回答的是现场那句「我把 agent 停了，为什么还在同步」——现在停了就看得见。 */}
            <th>目标端 Agent</th>
            <th>用户</th>
            {/* 「口令」列退役（ADR-0046 §3）：数据源要测通才存得下（ADR-0039 §3），
                存下来的每一条口令都是设过的，这一列因此恒为「已设置」——一列常量，
                占着一格宽度却答不了任何问题。表单里那个「已设置 · 留空 = 不改」的徽标
                照旧留着，它答的是「这次留空会怎样」，是另一回事。 */}
            <th>被引用</th>
            <th className="action-column">操作</th>
          </tr>
        </thead>
        <tbody>
          {datasources.map((datasource) => {
            const count = referenceCounts.get(datasource.datasource_id) ?? 0;
            const test = rowTests[datasource.datasource_id];
            return (
              <tr key={datasource.datasource_id}>
                <td>
                  <span className="task-name">{datasource.name}</span>
                  <span className="task-id">{datasource.datasource_id}</span>
                </td>
                <td>{KIND_LABELS[datasource.kind]}</td>
                <td className="mono">{connectionSummary(datasource)}</td>
                <td>
                  <AgentCell datasource={datasource} agents={agents} />
                </td>
                <td className="mono">{datasource.username}</td>
                <td>{count === 0 ? "未被引用" : `${count} 个任务`}</td>
                <td className="action-column">
                  {/* 行内动作**全用图标**、靠 `title` 认（ADR-0043 §5；ADR-0042 §6 已作废）。
                      「正在连接」这一态不再靠按钮文字自陈——它挂在 `title` / `aria-label` 上，
                      图标同时转起来，禁用的只有这一行自己。 */}
                  <div className="row-actions">
                    <ActionButton
                      label={
                        test?.kind === "testing" ? "正在连接" : "测试连接"
                      }
                      icon={
                        <PlugZap
                          className={test?.kind === "testing" ? "is-spinning" : ""}
                          size={16}
                        />
                      }
                      disabled={test?.kind === "testing"}
                      onClick={() => onTest(datasource)}
                    />
                    <span className="divider" />
                    <ActionButton
                      label="编辑数据源"
                      icon={<Pencil size={16} />}
                      onClick={() => onAction({ kind: "edit", datasource })}
                    />
                    <span className="divider" />
                    <ActionButton
                      label="删除数据源"
                      danger
                      icon={<Trash2 size={16} />}
                      onClick={() => onAction({ kind: "delete", datasource })}
                    />
                  </div>
                  {test?.kind === "ok" && (
                    <span className="inline-result row-test-result" role="status">
                      连接成功 · {test.elapsedMs} ms · {test.label}
                    </span>
                  )}
                  {test?.kind === "failed" && (
                    <span className="row-test-result is-failed" role="alert">
                      {test.message}
                    </span>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/**
 * 「目标端 Agent」格。Oracle 行是**空的**——源库由 source 直连，这一栏对它没有含义，
 * 写「不适用」会让人以为这里少配了一样东西。
 *
 * MySQL 行显示 agent 名字；不在线 / 身份不符时跟一个状态标签，因为**此刻这条数据源不能用**：
 * 点测试连接会失败、发起运行会被拒。把它藏起来只会让人在发起那一刻才发现。
 */
function AgentCell({
  datasource,
  agents,
}: {
  datasource: Datasource;
  agents: Agent[];
}) {
  if (datasource.kind === "oracle") {
    return null;
  }
  const label = agentLabel(datasource, agents);
  const status = agentStatusOf(datasource, agents);
  return (
    <>
      <span>{label}</span>
      {status !== null && status !== "online" && (
        <span className={`state ${status === "mismatch" ? "is-failed" : "is-unknown"}`}>
          {status === "mismatch" ? "身份不符" : "不在线"}
        </span>
      )}
    </>
  );
}

type TestState =
  | { kind: "idle" }
  | { kind: "testing" }
  | { kind: "ok"; elapsedMs: number; label: string }
  | { kind: "failed"; message: string };

/**
 * 新建 / 编辑数据源（ADR-0039 §3）。
 *
 * **保存门槛（所有者 2026-08-19 裁定 2）：当前表单的连接字段组合必须有过一次成功测连。**
 * 连接字段一改，上一次的结果当场作废。**唯一例外是只改名称**——那组值一个字没动，
 * 重测买不到任何新信息，却要求库在改名的那一刻恰好在线。
 *
 * 代价认下：库还没开通、网络还没放行时这条数据源根本录不进来。换来的是库里每一条都曾经
 * 真的连通过，「连不上」不会推迟到发起运行那一刻才炸。
 */
function DatasourceFormDialog({
  title,
  existing,
  agents,
  onClose,
  onChanged,
}: {
  title: string;
  existing: Datasource | null;
  agents: Agent[];
  onClose: () => void;
  onChanged: () => Promise<void>;
}) {
  const [draft, setDraft] = useState<DatasourceInput>(() => draftFrom(existing));
  const [test, setTest] = useState<TestState>({ kind: "idle" });
  const [testedFingerprint, setTestedFingerprint] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const initial = useMemo(() => draftFrom(existing), [existing]);
  const fingerprint = connectionFingerprint(draft);
  // 只改名称：连接字段一字未动，免测直接存。
  const untouchedConnection =
    existing !== null && fingerprint === connectionFingerprint(initial);
  const canSave = canSaveDatasource(draft, existing === null ? null : initial, testedFingerprint);

  function patch(changes: Partial<DatasourceInput>) {
    setDraft((current) => ({ ...current, ...changes }) as DatasourceInput);
    // 连接字段一改，上一次的测连结果当场作废——名称改动不走这条路（它不进指纹）。
    setTest({ kind: "idle" });
  }

  function switchKind(kind: DatasourceKind) {
    setTest({ kind: "idle" });
    setTestedFingerprint(null);
    setDraft((current) =>
      kind === "oracle"
        ? {
            name: current.name,
            kind: "oracle",
            connect_string: "",
            username: current.username,
            password: "",
          }
        : {
            name: current.name,
            kind: "mysql",
            // 只有一台 agent 时直接预选它：现场绝大多数部署就是一台，
            // 让人在一个只有一个选项的下拉里再点一次没有任何意义。
            agent_id: agents.length === 1 ? agents[0].agent_id : "",
            host: "",
            port: 3306,
            database: "",
            username: current.username,
            password: "",
          },
    );
  }

  async function handleTest() {
    setTest({ kind: "testing" });
    setError(null);
    try {
      const result = await testDatasourceDraft(
        draft,
        existing?.datasource_id ?? null,
      );
      setTest({ kind: "ok", elapsedMs: result.elapsed_ms, label: result.label });
      setTestedFingerprint(connectionFingerprint(draft));
    } catch (testError) {
      // **原样回显驱动报错**再跟一句人话，不出错误码标签——测连不属于任何 run，
      // 画一个 `SINK_ENVIRONMENT` 标签会让人以为这是一次运行的失败（ADR-0039 §3）。
      setTest({ kind: "failed", message: messageFrom(testError) });
      setTestedFingerprint(null);
    }
  }

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      if (existing === null) {
        await createDatasource(draft);
      } else {
        await updateDatasource(existing.datasource_id, draft);
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
              required
              value={draft.name}
              placeholder="如 生产核心库"
              onChange={(event) =>
                setDraft((current) => ({ ...current, name: event.target.value }))
              }
            />
          </FormField>

          <FormField label="类型">
            {existing === null ? (
              <select
                value={draft.kind}
                onChange={(event) =>
                  switchKind(event.target.value as DatasourceKind)
                }
              >
                <option value="oracle">Oracle（源端）</option>
                <option value="mysql">MySQL（目标端）</option>
              </select>
            ) : (
              // **编辑时类型不可改**：改类型等于换一个数据源，而任务已按 id 绑定了它
              // （ADR-0037 §8）。
              <input readOnly value={KIND_LABELS[draft.kind]} />
            )}
          </FormField>

          {draft.kind === "oracle" ? (
            <FormField label="连接串">
              <input
                required
                value={draft.connect_string}
                placeholder="如 //oracle-host:1521/ORCLPDB"
                onChange={(event) => patch({ connect_string: event.target.value })}
              />
            </FormField>
          ) : (
            <>
              <FormField label="目标端 Agent">
                <select
                  required
                  value={draft.agent_id}
                  onChange={(event) => patch({ agent_id: event.target.value })}
                >
                  <option value="">请选择</option>
                  {agents.map((agent) => (
                    <option key={agent.agent_id} value={agent.agent_id}>
                      {agent.name}
                      {agent.status === "online" ? "" : "（不可用）"}
                    </option>
                  ))}
                </select>
              </FormField>
              {agents.length === 0 && (
                <p className="drawer-note">
                  还没有注册任何目标端 agent。目标库只能经 agent 访问，
                  请先到「目标端 Agent」屏注册一台。
                </p>
              )}
              <FormField label="主机">
                <input
                  required
                  value={draft.host}
                  placeholder="如 10.0.0.12"
                  onChange={(event) => patch({ host: event.target.value })}
                />
              </FormField>
              <FormField label="端口">
                <input
                  required
                  type="number"
                  min={1}
                  max={65535}
                  value={draft.port}
                  onChange={(event) =>
                    patch({ port: Number(event.target.value) })
                  }
                />
              </FormField>
              <FormField label="库名">
                <input
                  required
                  value={draft.database}
                  placeholder="如 dw_stage"
                  onChange={(event) => patch({ database: event.target.value })}
                />
              </FormField>
            </>
          )}

          <FormField label="用户名">
            <input
              required
              value={draft.username}
              onChange={(event) => patch({ username: event.target.value })}
            />
          </FormField>

          <FormField
            label="口令"
            neutralBadge
            badge={
              existing === null
                ? undefined
                : existing.has_password
                  ? "已设置 · 留空 = 不改"
                  : "未设置"
            }
          >
            <input
              type="password"
              autoComplete="new-password"
              value={draft.password}
              placeholder={existing === null ? "" : "留空表示不改"}
              onChange={(event) => patch({ password: event.target.value })}
            />
          </FormField>

          <div className="row-actions">
            <button
              className="button is-ghost"
              type="button"
              onClick={() => void handleTest()}
              disabled={submitting || test.kind === "testing"}
            >
              {test.kind === "testing" ? "正在连接" : "测试连接"}
            </button>
            {untouchedConnection && test.kind === "idle" && (
              <span className="card-subtitle">
                连接字段未改动，只改名称可直接保存。
              </span>
            )}
          </div>

          {/* 成功是**一行纯文字**，不是标签、不套成功色（ADR-0039 §3）。 */}
          {test.kind === "ok" && (
            <div className="inline-result" role="status">
              连接成功 · {test.elapsedMs} ms · {test.label}
            </div>
          )}
          {test.kind === "failed" && (
            <div className="form-error" role="alert">
              {test.message}
              <br />
              请确认地址、账号和网络放行后重新测试。
            </div>
          )}
          {error !== null && (
            <div className="form-error" role="alert">
              {error}
            </div>
          )}
          {!canSave && test.kind !== "failed" && (
            <p className="card-subtitle">
              保存前请先测试连接。
            </p>
          )}
        </div>
        <ModalFooter
          onClose={onClose}
          busy={submitting}
          submitDisabled={!canSave}
          submitLabel={existing === null ? "新建" : "保存"}
        />
      </form>
    </Modal>
  );
}

/**
 * 删数据源（ADR-0039 §4 / ADR-0037 §7）。
 *
 * 仍被任务引用时服务端回 409，**报文里点名列出是哪几个任务**——那份引用清单为了判 409
 * 本来就要查。这里把它摆成 `.delete-copy` 正文里的一段列表，零新元素；红底那段话只留
 * 数量与动作，名字不跟着再列一遍（`deleteRefusalMessage`，#139）。
 */
function DatasourceDeleteDialog({
  datasource,
  onClose,
  onChanged,
}: {
  datasource: Datasource;
  onClose: () => void;
  onChanged: () => Promise<void>;
}) {
  const [error, setError] = useState<string | null>(null);
  const [blockedBy, setBlockedBy] = useState<string[]>([]);
  const [submitting, setSubmitting] = useState(false);

  async function handleDelete() {
    setSubmitting(true);
    setError(null);
    setBlockedBy([]);
    try {
      await deleteDatasource(datasource.datasource_id);
      await onChanged();
      onClose();
    } catch (deleteError) {
      setError(messageFrom(deleteError));
      setBlockedBy(referencedTasksFrom(deleteError));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Modal title="删除数据源" onClose={onClose} busy={submitting} narrow>
      <div className="modal-body delete-copy">
        <p>
          确认删除数据源“<strong>{datasource.name}</strong>”？
        </p>
        <span className="task-id">{datasource.datasource_id}</span>
        {error !== null && (
          <div className="form-error" role="alert">
            {deleteRefusalMessage(error, blockedBy)}
          </div>
        )}
        {blockedBy.length > 0 && (
          <ul>
            {blockedBy.map((name) => (
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
