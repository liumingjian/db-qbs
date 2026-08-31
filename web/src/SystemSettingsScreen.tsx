import { KeyRound, Mail, Save, Send, Settings, UserRoundCheck } from "lucide-react";
import { useEffect, useState } from "react";
import type { FormEvent } from "react";

import {
  fetchEmailAlertSettings,
  fetchOperatorAccount,
  sendTestEmail,
  updateEmailAlertSettings,
  updateOperatorAccount,
} from "./api";
import type { EmailAlertSettings, EmailAlertSettingsInput, OperatorAccount } from "./api";
import { ICON } from "./components/DesignSystem";
import { messageFrom } from "./errors";
import { FormField } from "./ui";

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
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  useEffect(() => {
    void fetchEmailAlertSettings().then((loaded) => {
      setSettings(loaded);
      setDraft(inputFrom(loaded));
    }).catch((loadError) => setError(messageFrom(loadError)));
  }, []);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (draft === null) return;
    setBusy(true);
    setError(null);
    setSaved(null);
    try {
      const updated = await updateEmailAlertSettings(draft);
      setSettings(updated);
      setDraft(inputFrom(updated));
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
      setSaved(result.status === "SUCCESS" ? "测试邮件已发送。" : "测试邮件发送失败，请查看最新结果。");
    } catch (testError) {
      setError(messageFrom(testError));
    } finally {
      setBusy(false);
    }
  }

  if (draft === null && error === null) {
    return <div className="loading-state" aria-live="polite">正在读取邮件告警设置...</div>;
  }
  return <EmailAlertSettingsView settings={settings} draft={draft} busy={busy} error={error} saved={saved} onChange={setDraft} onSubmit={(event) => void submit(event)} onTest={() => void testEmail()} />;
}

export function EmailAlertSettingsView({ settings, draft, busy, error, saved, onChange, onSubmit, onTest }: {
  settings: EmailAlertSettings | null;
  draft: EmailAlertSettingsInput | null;
  busy: boolean;
  error: string | null;
  saved: string | null;
  onChange: (draft: EmailAlertSettingsInput) => void;
  onSubmit: (event: FormEvent) => void;
  onTest: () => void;
}) {
  const patch = (change: Partial<EmailAlertSettingsInput>) => {
    if (draft !== null) onChange({ ...draft, ...change });
  };
  return (
    <div className="settings-pane">
      <header className="settings-pane-header">
        <div><h2>邮件告警</h2><p>失败运行的全局 SMTP 连接、发件人和收件人设置。</p></div>
        {settings !== null && <span className={`state ${settings.enabled ? "is-succeeded" : "is-unknown"}`}>{settings.enabled ? "已启用" : "未启用"}</span>}
      </header>
      {error !== null && <div className="form-error" role="alert">{error}</div>}
      {saved !== null && <div className="inline-result" role="status">{saved}</div>}
      {draft !== null && (
        <form className="email-settings-form" onSubmit={onSubmit}>
          <label className="schedule-switch email-enabled">
            <input type="checkbox" checked={draft.enabled} onChange={(event) => patch({ enabled: event.target.checked })} />
            <span>{draft.enabled ? "启用失败邮件告警" : "邮件告警已停用"}</span>
          </label>
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
            <button className="button is-primary" type="submit" disabled={busy}><Save size={ICON.sm} aria-hidden="true" />保存设置</button>
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
          {settings.latest_test_result.error !== null && <div className="form-error" role="status">{settings.latest_test_result.error}</div>}
        </section>
      )}
    </div>
  );
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
