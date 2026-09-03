import { Info, KeyRound, Mail, RefreshCw, Save, Send, Settings, UserRoundCheck } from "lucide-react";
import { useEffect, useState } from "react";
import type { FormEvent } from "react";

import {
  fetchEmailAlertSettings,
  fetchEmailDeliveries,
  fetchOperatorAccount,
  retryEmailDelivery,
  sendTestEmail,
  updateEmailAlertSettings,
  updateOperatorAccount,
} from "./api";
import type { EmailAlertSettings, EmailAlertSettingsInput, EmailDeliveryHistory, OperatorAccount } from "./api";
import { ICON } from "./components/DesignSystem";
import { messageFrom } from "./errors";
import { FormField, Modal } from "./ui";

type SettingsView = "email" | "operator";

export function SystemSettingsScreen() {
  const [view, setView] = useState<SettingsView>("email");
  return (
    <section className="card system-settings" aria-labelledby="settings-title">
      <header className="card-header">
        <div><h1 id="settings-title">系统设置</h1><span className="card-subtitle">仅管理员可见</span></div>
      </header>
      <div className="settings-layout">
        <nav className="settings-nav" aria-label="系统设置">
          <button className={view === "email" ? "is-active" : ""} type="button" onClick={() => setView("email")}>
            <Mail size={ICON.sm} aria-hidden="true" />邮件告警
          </button>
          <button className={view === "operator" ? "is-active" : ""} type="button" onClick={() => setView("operator")}>
            <UserRoundCheck size={ICON.sm} aria-hidden="true" />操作员账号
          </button>
        </nav>
        {view === "email" ? <EmailAlertSettingsPane /> : <OperatorAccountSettingsPane />}
      </div>
    </section>
  );
}

function EmailAlertSettingsPane() {
  const [settings, setSettings] = useState<EmailAlertSettings | null>(null);
  const [draft, setDraft] = useState<EmailAlertSettingsInput | null>(null);
  const [deliveries, setDeliveries] = useState<EmailDeliveryHistory[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [disableConfirmed, setDisableConfirmed] = useState(false);

  useEffect(() => {
    void Promise.all([fetchEmailAlertSettings(), fetchEmailDeliveries()]).then(([loaded, history]) => {
      setSettings(loaded);
      setDraft(inputFrom(loaded));
      setDeliveries(history);
    }).catch((loadError) => setError(messageFrom(loadError)));
  }, []);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (draft === null) return;
    if (settings?.enabled && !draft.enabled && !disableConfirmed) return;
    setBusy(true);
    setError(null);
    setSaved(null);
    try {
      const updated = await updateEmailAlertSettings(draft);
      setSettings(updated);
      setDraft(inputFrom(updated));
      setDisableConfirmed(false);
      setSaved("邮件告警设置已保存。");
    } catch (updateError) {
      setError(messageFrom(updateError));
    } finally {
      setBusy(false);
    }
  }

  async function testEmail() {
    setBusy(true);
    setError(null);
    setSaved(null);
    try {
      const result = await sendTestEmail();
      setSettings((current) => current === null ? current : {
        ...current,
        latest_test_result: result,
      });
      setSaved(result.status === "SUCCESS" ? "测试邮件已发送。" : "测试邮件发送失败，已生成详情。");
    } catch (testError) {
      setError(messageFrom(testError));
    } finally {
      setBusy(false);
    }
  }

  async function retryDelivery(deliveryId: string) {
    setBusy(true);
    setError(null);
    setSaved(null);
    try {
      const retried = await retryEmailDelivery(deliveryId);
      setDeliveries((current) => current.map((delivery) => (
        delivery.delivery_id === retried.delivery_id ? retried : delivery
      )));
      setSaved("邮件投递已重新进入发送队列。");
    } catch (retryError) {
      setError(messageFrom(retryError));
    } finally {
      setBusy(false);
    }
  }

  if (draft === null && error === null) {
    return <div className="loading-state" aria-live="polite">正在读取邮件告警设置...</div>;
  }
  return <EmailAlertSettingsView settings={settings} draft={draft} deliveries={deliveries} busy={busy} error={error} saved={saved} disableConfirmed={disableConfirmed} onDisableConfirmationChange={setDisableConfirmed} onChange={(next) => { setDraft(next); if (next.enabled) setDisableConfirmed(false); }} onSubmit={(event) => void submit(event)} onTest={() => void testEmail()} onRetry={(deliveryId) => void retryDelivery(deliveryId)} />;
}

export function EmailAlertSettingsView({ settings, draft, deliveries = [], busy, error, saved, disableConfirmed = false, onDisableConfirmationChange = () => undefined, onChange, onSubmit, onTest, onRetry = () => undefined }: {
  settings: EmailAlertSettings | null;
  draft: EmailAlertSettingsInput | null;
  deliveries?: EmailDeliveryHistory[];
  busy: boolean;
  error: string | null;
  saved: string | null;
  disableConfirmed?: boolean;
  onDisableConfirmationChange?: (confirmed: boolean) => void;
  onChange: (draft: EmailAlertSettingsInput) => void;
  onSubmit: (event: FormEvent) => void;
  onTest: () => void;
  onRetry?: (deliveryId: string) => void;
}) {
  const patch = (change: Partial<EmailAlertSettingsInput>) => {
    if (draft !== null) onChange({ ...draft, ...change });
  };
  const [errorDetailsOpen, setErrorDetailsOpen] = useState(false);
  const errorDetails = emailErrorDetails(settings, deliveries, error);
  return (
    <div className="settings-pane">
      <header className="settings-pane-header">
        <div><h2>邮件告警</h2><p>失败运行的全局 SMTP 连接、发件人和收件人设置。</p></div>
        {settings !== null && <span className={`state ${settings.enabled ? "is-succeeded" : "is-unknown"}`}>{settings.enabled ? "已启用" : "未启用"}</span>}
      </header>
      {errorDetails.length > 0 && <EmailErrorNotice details={errorDetails} onDetails={() => setErrorDetailsOpen(true)} />}
      {saved !== null && <div className="inline-result" role="status">{saved}</div>}
      {draft !== null && (
        <form className="email-settings-form" onSubmit={onSubmit}>
          <label className="schedule-switch email-enabled">
            <input type="checkbox" checked={draft.enabled} onChange={(event) => patch({ enabled: event.target.checked })} />
            <span>{draft.enabled ? "启用失败邮件告警" : "邮件告警已停用"}</span>
          </label>
          {settings?.enabled && !draft.enabled && (
            <div className="form-error email-disable-warning" role="alert">
              <strong>停用会立即终止所有待发送和重试中的邮件。</strong>
              <label>
                <input type="checkbox" checked={disableConfirmed} onChange={(event) => onDisableConfirmationChange(event.target.checked)} />
                我确认终止这些投递；以后重新启用也不会补发。
              </label>
            </div>
          )}
          <fieldset className="settings-fieldset">
            <legend>SMTP 连接</legend>
            <div className="email-settings-grid">
              <FormField label="服务商预设">
                <select value={draft.provider_preset} onChange={(event) => {
                  const provider = event.target.value as EmailAlertSettingsInput["provider_preset"];
                  patch(provider === "TENCENT_EXMAIL" ? {
                    provider_preset: provider,
                    smtp_host: "smtp.exmail.qq.com",
                    smtp_port: 465,
                    smtp_security: "IMPLICIT_TLS",
                  } : { provider_preset: provider });
                }}>
                  <option value="TENCENT_EXMAIL">腾讯企业邮</option><option value="GENERIC">通用 SMTP</option>
                </select>
              </FormField>
              <FormField label="传输安全">
                <select value={draft.smtp_security} onChange={(event) => patch({ smtp_security: event.target.value as EmailAlertSettingsInput["smtp_security"] })}>
                  <option value="IMPLICIT_TLS">隐式 SSL/TLS</option><option value="STARTTLS">STARTTLS</option>
                </select>
              </FormField>
              <FormField label="SMTP 主机"><input required={draft.enabled} value={draft.smtp_host} onChange={(event) => patch({ smtp_host: event.target.value })} /></FormField>
              <FormField label="端口"><input required type="number" min={1} max={65535} value={draft.smtp_port} onChange={(event) => patch({ smtp_port: Number(event.target.value) })} /></FormField>
              <FormField label="用户名"><input required={draft.enabled} value={draft.smtp_username} autoComplete="username" onChange={(event) => patch({ smtp_username: event.target.value })} /></FormField>
              <FormField label="SMTP 密钥" neutralBadge badge={settings?.has_smtp_secret ? "已保存，留空不变" : "尚未设置"}>
                <input required={draft.enabled && !settings?.has_smtp_secret} type="password" value={draft.smtp_secret} autoComplete="new-password" onChange={(event) => patch({ smtp_secret: event.target.value })} />
              </FormField>
            </div>
          </fieldset>
          <fieldset className="settings-fieldset">
            <legend>发件人与收件人</legend>
            <div className="email-settings-grid">
              <FormField label="发件人地址"><input required={draft.enabled} type="email" value={draft.sender_address} onChange={(event) => patch({ sender_address: event.target.value })} /></FormField>
              <FormField label="发件人名称"><input required={draft.enabled} value={draft.sender_name} onChange={(event) => patch({ sender_name: event.target.value })} /></FormField>
              <div className="email-recipient-field"><FormField label="收件人（每行一个，最多 50 个）">
                <textarea required={draft.enabled} value={draft.recipients.join("\n")} onChange={(event) => patch({ recipients: event.target.value.split("\n") })} />
              </FormField></div>
            </div>
          </fieldset>
          <fieldset className="settings-fieldset">
            <legend>告警标识与重试</legend>
            <div className="email-settings-grid">
              <FormField label="实例名称"><input required value={draft.instance_name} onChange={(event) => patch({ instance_name: event.target.value })} /></FormField>
              <FormField label="最大重试小时数"><input required type="number" min={0} max={168} step={1} value={draft.max_retry_hours} onChange={(event) => patch({ max_retry_hours: Number(event.target.value) })} /></FormField>
              <div className="email-recipient-field"><FormField label="外部访问地址（可选）">
                <input type="url" placeholder="https://qbs.example.com" value={draft.external_base_url ?? ""} onChange={(event) => patch({ external_base_url: event.target.value || null })} />
              </FormField></div>
            </div>
          </fieldset>
          <div className="settings-actions">
            <button className="button is-primary" type="submit" disabled={busy || Boolean(settings?.enabled && !draft.enabled && !disableConfirmed)}><Save size={ICON.sm} aria-hidden="true" />保存设置</button>
            <button className="button" type="button" disabled={busy} onClick={onTest}><Send size={ICON.sm} aria-hidden="true" />发送测试邮件</button>
            <span className="card-subtitle">保存只校验配置，不会连接 SMTP 服务器。</span>
          </div>
        </form>
      )}
      {settings?.latest_test_result !== null && settings?.latest_test_result !== undefined && (
        <section className="email-test-result" aria-labelledby="email-test-result-title">
          <div className="settings-pane-header">
            <div><h3 id="email-test-result-title">最新测试结果</h3><p>{settings.latest_test_result.tested_at}</p></div>
            <span className={`state ${settings.latest_test_result.status === "SUCCESS" ? "is-succeeded" : "is-failed"}`}>
              {settings.latest_test_result.status === "SUCCESS" ? "发送成功" : "发送失败"}
            </span>
          </div>
          {settings.latest_test_result.error !== null && <p className="email-test-result-note" role="status">失败原因已收进“查看详情”。</p>}
        </section>
      )}
      <EmailDeliveryHistoryView deliveries={deliveries} busy={busy} onRetry={onRetry} />
      {errorDetailsOpen && errorDetails.length > 0 && <EmailErrorDialog details={errorDetails} onClose={() => setErrorDetailsOpen(false)} />}
    </div>
  );
}

export function EmailDeliveryHistoryView({ deliveries, busy, onRetry }: {
  deliveries: EmailDeliveryHistory[];
  busy: boolean;
  onRetry: (deliveryId: string) => void;
}) {
  return (
    <section className="email-delivery-history" aria-labelledby="email-delivery-history-title">
      <div className="settings-pane-header">
        <div><h3 id="email-delivery-history-title">投递历史</h3><p>按收件人显示发送状态与重试进度，异常详情通过提示查看。</p></div>
      </div>
      {deliveries.length === 0 ? (
        <div className="empty-state">暂无邮件投递记录</div>
      ) : (
        <div className="table-wrap">
          <table className="data-grid email-delivery-grid">
            <thead><tr><th>告警 / 任务</th><th>收件人</th><th>状态</th><th>尝试</th><th>最近尝试 / 下次尝试</th><th><span className="visually-hidden">操作</span></th></tr></thead>
            <tbody>{deliveries.map((delivery) => (
              <tr key={delivery.delivery_id}>
                <td><strong>{delivery.task_name}</strong><span className="table-side mono">{delivery.alert_id}</span></td>
                <td className="mono">{delivery.recipient}</td>
                <td><span className={`state ${deliveryStateClass(delivery.state)}`}>{deliveryStateLabel(delivery.state)}</span></td>
                <td>{delivery.attempt_count}</td>
                <td><span className="table-cell">{delivery.last_attempt_at ?? "尚未尝试"}</span><span className="table-side">{delivery.next_attempt_at === null ? `截止 ${delivery.retry_deadline_at}` : `下次 ${delivery.next_attempt_at}`}</span></td>
                <td>{delivery.state === "FAILED" && <button className="icon-button" type="button" disabled={busy} title="重新发送" aria-label={`重新发送给 ${delivery.recipient}`} onClick={() => onRetry(delivery.delivery_id)}><RefreshCw size={ICON.sm} aria-hidden="true" /></button>}</td>
              </tr>
            ))}</tbody>
          </table>
        </div>
      )}
    </section>
  );
}

export interface EmailErrorDetail {
  id: string;
  source: "邮件操作" | "测试邮件" | "后台投递";
  severity: "error" | "warn";
  error: string;
  occurredAt: string | null;
  taskName: string | null;
  runRecordId: string | null;
  recipient: string | null;
  state: EmailDeliveryHistory["state"] | null;
  attemptCount: number | null;
  nextAttemptAt: string | null;
  retryDeadlineAt: string | null;
  smtpHost: string | null;
  smtpPort: number | null;
  smtpSecurity: EmailAlertSettings["smtp_security"] | null;
}

function emailErrorDetails(
  settings: EmailAlertSettings | null,
  deliveries: EmailDeliveryHistory[],
  requestError: string | null,
): EmailErrorDetail[] {
  const details: EmailErrorDetail[] = [];
  const smtp = settings === null ? null : {
    smtpHost: settings.smtp_host,
    smtpPort: settings.smtp_port,
    smtpSecurity: settings.smtp_security,
  };
  if (requestError !== null) {
    details.push({
      id: "email-request",
      source: "邮件操作",
      severity: "error",
      error: requestError,
      occurredAt: null,
      taskName: null,
      runRecordId: null,
      recipient: null,
      state: null,
      attemptCount: null,
      nextAttemptAt: null,
      retryDeadlineAt: null,
      smtpHost: smtp?.smtpHost ?? null,
      smtpPort: smtp?.smtpPort ?? null,
      smtpSecurity: smtp?.smtpSecurity ?? null,
    });
  }
  const test = settings?.latest_test_result;
  if (test?.status === "FAILED") {
    details.push({
      id: "email-test",
      source: "测试邮件",
      severity: "error",
      error: test.error ?? "测试邮件未发送成功",
      occurredAt: test.tested_at,
      taskName: null,
      runRecordId: null,
      recipient: null,
      state: null,
      attemptCount: null,
      nextAttemptAt: null,
      retryDeadlineAt: null,
      smtpHost: smtp?.smtpHost ?? null,
      smtpPort: smtp?.smtpPort ?? null,
      smtpSecurity: smtp?.smtpSecurity ?? null,
    });
  }
  for (const delivery of deliveries) {
    if (delivery.last_error === null || (delivery.state !== "FAILED" && delivery.state !== "PENDING")) {
      continue;
    }
    details.push({
      id: delivery.delivery_id,
      source: "后台投递",
      severity: delivery.state === "PENDING" ? "warn" : "error",
      error: delivery.last_error,
      occurredAt: delivery.last_attempt_at ?? delivery.failed_at,
      taskName: delivery.task_name,
      runRecordId: delivery.run_record_id,
      recipient: delivery.recipient,
      state: delivery.state,
      attemptCount: delivery.attempt_count,
      nextAttemptAt: delivery.next_attempt_at,
      retryDeadlineAt: delivery.retry_deadline_at,
      smtpHost: smtp?.smtpHost ?? null,
      smtpPort: smtp?.smtpPort ?? null,
      smtpSecurity: smtp?.smtpSecurity ?? null,
    });
  }
  return details;
}

function emailErrorGuide(error: string): {
  title: string;
  notice: string;
  explanation: string;
  causes: string[];
  actions: string[];
} {
  if (error.includes("必须保存完整")) {
    return {
      title: "邮件配置还不完整",
      notice: "请先补齐 SMTP、发件人和收件人信息。",
      explanation: "系统还没有足够的配置来尝试发送邮件。",
      causes: ["SMTP 密钥尚未保存", "发件人或收件人地址为空或格式不正确"],
      actions: ["补齐带星号的邮件配置", "点击“保存设置”后再发送测试邮件"],
    };
  }
  if (error.includes("TLS")) {
    return {
      title: "安全连接没有建立",
      notice: "邮件服务器的加密连接没有建立成功。",
      explanation: "source 已连接到邮件发送流程，但 TLS 握手或证书校验没有完成。",
      causes: ["端口与加密方式不匹配", "邮件服务器证书名称与 SMTP 主机不一致"],
      actions: ["465 端口使用隐式 SSL/TLS，587 端口使用 STARTTLS", "确认 SMTP 主机名与服务器证书一致"],
    };
  }
  if (error.includes("超时")) {
    return {
      title: "邮件服务器没有及时响应",
      notice: "连接或发送等待超时，邮件没有完成投递。",
      explanation: "source 在等待邮件服务器时超过了允许时间。",
      causes: ["source 主机到 SMTP 服务器的网络不通", "防火墙或邮件服务器当前响应缓慢"],
      actions: ["从运行 source 的机器检查 SMTP 主机和端口连通性", "确认出口防火墙允许该 SMTP 端口"],
    };
  }
  if (error.includes("暂时拒绝")) {
    return {
      title: "邮件服务器暂时拒绝发送",
      notice: "邮件会按当前重试策略再次尝试。",
      explanation: "邮件服务器返回了暂时性拒绝，当前投递仍可能处于等待重试状态。",
      causes: ["服务器限流或暂时繁忙", "发件策略要求稍后重试"],
      actions: ["等待下一次自动重试", "如果持续失败，请联系邮件服务商确认限流策略"],
    };
  }
  if (error.includes("服务器拒绝") || error.includes("认证")) {
    return {
      title: "邮件服务器拒绝了请求",
      notice: "请检查 SMTP 账号和发件设置。",
      explanation: "source 已到达邮件服务器，但服务器没有接受这次认证或发信请求。",
      causes: ["SMTP 用户名或密钥不正确", "发件地址不符合服务商的发信策略"],
      actions: ["确认用户名和 SMTP 密钥有效", "确认发件地址已被邮件服务商允许"],
    };
  }
  if (error.includes("无法连接")) {
    return {
      title: "无法连接邮件服务器",
      notice: "source 没有连通配置的 SMTP 主机。",
      explanation: "邮件发送请求没有完成与 SMTP 服务器的连接。",
      causes: ["SMTP 主机名解析失败或端口未开放", "source 主机的网络出口被拦截"],
      actions: ["在 source 主机上检查 DNS 和 SMTP 端口", "确认主机、端口和安全模式与服务商文档一致"],
    };
  }
  return {
    title: "邮件操作没有完成",
    notice: "请打开详情查看系统给出的诊断信息。",
    explanation: "系统没有完成这次邮件操作，但保留了可供排查的安全诊断。",
    causes: ["配置、网络或邮件服务器状态异常"],
    actions: ["核对邮件配置后重新测试", "如果问题持续，请提供此处的系统诊断给管理员"],
  };
}

function emailErrorCode(error: string): string | null {
  if (error.includes("超时")) return "SMTP_TIMEOUT";
  if (error.includes("TLS")) return "SMTP_TLS";
  if (error.includes("暂时拒绝")) return "SMTP_TRANSIENT";
  if (error.includes("服务器拒绝") || error.includes("认证")) return "SMTP_PERMANENT";
  if (error.includes("无法连接")) return "SMTP_NETWORK";
  return null;
}

function emailSecurityLabel(security: EmailAlertSettings["smtp_security"]): string {
  return security === "IMPLICIT_TLS" ? "隐式 SSL/TLS" : "STARTTLS";
}

function emailErrorStateLabel(state: EmailErrorDetail["state"]): string {
  return state === "PENDING" ? "等待重试" : "发送失败";
}

function EmailErrorNotice({ details, onDetails }: {
  details: EmailErrorDetail[];
  onDetails: () => void;
}) {
  const failed = details.some((detail) => detail.severity === "error");
  const guide = emailErrorGuide(details[0].error);
  const title = details.length === 1
    ? guide.title
    : failed ? "邮件发送存在异常" : "邮件投递等待重试";
  const notice = details.length === 1
    ? guide.notice
    : `检测到 ${details.length} 条邮件记录需要关注。`;
  return (
    <div className={`notice email-error-notice ${failed ? "" : "is-warn"}`} role="alert">
      <div>
        <strong>{title}</strong>
        <span className="notice-side">{notice}</span>
      </div>
      <div className="notice-actions">
        <button className="text-button email-error-details-button" type="button" onClick={onDetails}>
          <Info size={ICON.sm} aria-hidden="true" />查看详情
        </button>
      </div>
    </div>
  );
}

export function EmailErrorDialog({ details, onClose }: {
  details: EmailErrorDetail[];
  onClose: () => void;
}) {
  return (
    <Modal title="邮件错误详情" onClose={onClose} busy={false}>
      <div className="modal-body email-error-dialog-body">
        <p className="email-error-dialog-intro">下面的信息用于定位邮件发送问题，不包含 SMTP 密钥或服务器原始响应。</p>
        <div className="email-error-detail-list">
          {details.map((detail) => {
            const guide = emailErrorGuide(detail.error);
            const code = emailErrorCode(detail.error);
            return (
              <section className="email-error-detail" key={detail.id}>
                <header className="email-error-detail-header">
                  <div><span className="email-error-source">{detail.source}</span><h3>{guide.title}</h3></div>
                  <span className={`state ${detail.severity === "error" ? "is-failed" : "is-unknown"}`}>{emailErrorStateLabel(detail.state)}</span>
                </header>
                <p className="email-error-explanation">{guide.explanation}</p>
                <dl className="email-error-facts">
                  <div><dt>发生时间</dt><dd className="mono">{detail.occurredAt ?? "本次操作"}</dd></div>
                  {detail.taskName !== null && <div><dt>任务</dt><dd>{detail.taskName}</dd></div>}
                  {detail.runRecordId !== null && <div><dt>运行记录</dt><dd className="mono">{detail.runRecordId}</dd></div>}
                  {detail.recipient !== null && <div><dt>收件人</dt><dd className="mono">{detail.recipient}</dd></div>}
                  {detail.state !== null && <div><dt>当前状态</dt><dd>{emailErrorStateLabel(detail.state)}</dd></div>}
                  {detail.attemptCount !== null && <div><dt>已尝试</dt><dd>{detail.attemptCount} 次</dd></div>}
                  {detail.nextAttemptAt !== null && <div><dt>下次尝试</dt><dd className="mono">{detail.nextAttemptAt}</dd></div>}
                  {detail.retryDeadlineAt !== null && <div><dt>重试截止</dt><dd className="mono">{detail.retryDeadlineAt}</dd></div>}
                  {detail.smtpHost !== null && <div><dt>连接目标</dt><dd className="mono">{detail.smtpHost}:{detail.smtpPort}</dd></div>}
                  {detail.smtpSecurity !== null && <div><dt>安全方式</dt><dd>{emailSecurityLabel(detail.smtpSecurity)}</dd></div>}
                </dl>
                <div className="email-error-section"><h4>可能原因</h4><ul>{guide.causes.map((cause) => <li key={cause}>{cause}</li>)}</ul></div>
                <div className="email-error-section"><h4>建议处理</h4><ul>{guide.actions.map((action) => <li key={action}>{action}</li>)}</ul></div>
                <div className="email-error-diagnostic"><span>系统诊断</span><code>{detail.error}</code>{code !== null && <small>{code}</small>}</div>
              </section>
            );
          })}
        </div>
      </div>
    </Modal>
  );
}

function deliveryStateLabel(state: EmailDeliveryHistory["state"]): string {
  return { PENDING: "待发送", SENT: "已发送", FAILED: "失败", NOT_SENT: "未发送", SUPPRESSED: "已抑制" }[state];
}

function deliveryStateClass(state: EmailDeliveryHistory["state"]): string {
  if (state === "SENT") return "is-succeeded";
  if (state === "FAILED") return "is-failed";
  return "is-unknown";
}

function inputFrom(settings: EmailAlertSettings): EmailAlertSettingsInput {
  const { has_smtp_secret: _hasSecret, latest_test_result: _latestTest, ...input } = settings;
  return { ...input, smtp_secret: "" };
}

function OperatorAccountSettingsPane() {
  const [account, setAccount] = useState<OperatorAccount | null>(null);
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  useEffect(() => {
    void fetchOperatorAccount().then(setAccount).catch((loadError) => setError(messageFrom(loadError)));
  }, []);

  async function update(input: { enabled: boolean; password?: string }, message: string) {
    setBusy(true);
    setError(null);
    setSaved(null);
    try {
      setAccount(await updateOperatorAccount(input));
      setPassword("");
      setSaved(message);
    } catch (updateError) {
      setError(messageFrom(updateError));
    } finally {
      setBusy(false);
    }
  }

  async function submitPassword(event: FormEvent) {
    event.preventDefault();
    if (account === null || password === "") return;
    await update({ enabled: account.enabled, password }, account.has_password ? "操作员口令已重置。" : "操作员口令已设置。");
  }
  return <OperatorAccountView account={account} password={password} busy={busy} error={error} saved={saved} onPassword={setPassword} onSubmitPassword={(event) => void submitPassword(event)} onToggle={() => {
    if (account !== null) void update({ enabled: !account.enabled }, account.enabled ? "操作员账号已停用。" : "操作员账号已启用。");
  }} />;
}

export function OperatorAccountView({ account, password, busy, error, saved, onPassword, onSubmitPassword, onToggle }: {
  account: OperatorAccount | null;
  password: string;
  busy: boolean;
  error: string | null;
  saved: string | null;
  onPassword: (password: string) => void;
  onSubmitPassword: (event: FormEvent) => void;
  onToggle: () => void;
}) {
  if (account === null && error === null) return <div className="loading-state" aria-live="polite">正在读取操作员账号...</div>;
  return (
    <div className="settings-pane">
      <header className="settings-pane-header">
        <div><h2>操作员账号</h2><p>固定账号 operator，用于日常任务与运行操作。</p></div>
        {account !== null && <span className={`state ${account.enabled ? "is-succeeded" : "is-unknown"}`}>{account.enabled ? "已启用" : "未启用"}</span>}
      </header>
      {error !== null && <div className="form-error" role="alert">{error}</div>}
      {saved !== null && <div className="inline-result" role="status">{saved}</div>}
      {account !== null && <>
        <dl className="account-facts">
          <div><dt>账号</dt><dd className="mono">{account.username}</dd></div><div><dt>角色</dt><dd>操作员</dd></div><div><dt>口令</dt><dd>{account.has_password ? "已设置" : "未设置"}</dd></div>
        </dl>
        <form className="operator-password-form" onSubmit={onSubmitPassword}>
          <FormField label={account.has_password ? "新口令" : "初始口令"}><input type="password" autoComplete="new-password" value={password} onChange={(event) => onPassword(event.target.value)} /></FormField>
          <button className="button is-primary" type="submit" disabled={busy || password === ""}><KeyRound size={ICON.sm} aria-hidden="true" />{account.has_password ? "重置口令" : "设置口令"}</button>
        </form>
        <div className="settings-actions">
          <button className={`button ${account.enabled ? "is-danger" : "is-primary"}`} type="button" disabled={busy || (!account.enabled && !account.has_password)} onClick={onToggle}><Settings size={ICON.sm} aria-hidden="true" />{account.enabled ? "停用账号" : "启用账号"}</button>
          {!account.enabled && !account.has_password && <span className="card-subtitle">先设置口令，再启用账号。</span>}
        </div>
      </>}
    </div>
  );
}
