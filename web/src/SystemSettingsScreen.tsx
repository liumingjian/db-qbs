import { KeyRound, Settings, UserRoundCheck } from "lucide-react";
import { useEffect, useState } from "react";
import type { FormEvent } from "react";

import { fetchOperatorAccount, updateOperatorAccount } from "./api";
import type { OperatorAccount } from "./api";
import { ICON } from "./components/DesignSystem";
import { messageFrom } from "./errors";
import { FormField } from "./ui";

export function SystemSettingsScreen() {
  const [account, setAccount] = useState<OperatorAccount | null>(null);
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  useEffect(() => {
    void fetchOperatorAccount()
      .then(setAccount)
      .catch((loadError) => setError(messageFrom(loadError)));
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
    await update(
      { enabled: account.enabled, password },
      account.has_password ? "操作员口令已重置。" : "操作员口令已设置。",
    );
  }

  return (
    <section className="card system-settings" aria-labelledby="settings-title">
      <header className="card-header">
        <div>
          <h1 id="settings-title">系统设置</h1>
          <span className="card-subtitle">仅管理员可见</span>
        </div>
      </header>
      <div className="settings-layout">
        <nav className="settings-nav" aria-label="系统设置">
          <span className="is-active">
            <UserRoundCheck size={ICON.sm} aria-hidden="true" />
            操作员账号
          </span>
        </nav>
        <OperatorAccountView
          account={account}
          password={password}
          busy={busy}
          error={error}
          saved={saved}
          onPassword={setPassword}
          onSubmitPassword={(event) => void submitPassword(event)}
          onToggle={() => {
            if (account !== null) {
              void update(
                { enabled: !account.enabled },
                account.enabled ? "操作员账号已停用。" : "操作员账号已启用。",
              );
            }
          }}
        />
      </div>
    </section>
  );
}

export function OperatorAccountView({
  account,
  password,
  busy,
  error,
  saved,
  onPassword,
  onSubmitPassword,
  onToggle,
}: {
  account: OperatorAccount | null;
  password: string;
  busy: boolean;
  error: string | null;
  saved: string | null;
  onPassword: (password: string) => void;
  onSubmitPassword: (event: FormEvent) => void;
  onToggle: () => void;
}) {
  if (account === null && error === null) {
    return <div className="loading-state" aria-live="polite">正在读取操作员账号...</div>;
  }
  return (
    <div className="settings-pane">
      <header className="settings-pane-header">
        <div>
          <h2>操作员账号</h2>
          <p>固定账号 operator，用于日常任务与运行操作。</p>
        </div>
        {account !== null && (
          <span className={`state ${account.enabled ? "is-succeeded" : "is-unknown"}`}>
            {account.enabled ? "已启用" : "未启用"}
          </span>
        )}
      </header>
      {error !== null && <div className="form-error" role="alert">{error}</div>}
      {saved !== null && <div className="inline-result" role="status">{saved}</div>}
      {account !== null && (
        <>
          <dl className="account-facts">
            <div><dt>账号</dt><dd className="mono">{account.username}</dd></div>
            <div><dt>角色</dt><dd>操作员</dd></div>
            <div><dt>口令</dt><dd>{account.has_password ? "已设置" : "未设置"}</dd></div>
          </dl>
          <form className="operator-password-form" onSubmit={onSubmitPassword}>
            <FormField label={account.has_password ? "新口令" : "初始口令"}>
              <input
                type="password"
                autoComplete="new-password"
                value={password}
                onChange={(event) => onPassword(event.target.value)}
              />
            </FormField>
            <button className="button is-primary" type="submit" disabled={busy || password === ""}>
              <KeyRound size={ICON.sm} aria-hidden="true" />
              {account.has_password ? "重置口令" : "设置口令"}
            </button>
          </form>
          <div className="settings-actions">
            <button
              className={`button ${account.enabled ? "is-danger" : "is-primary"}`}
              type="button"
              disabled={busy || (!account.enabled && !account.has_password)}
              onClick={onToggle}
            >
              <Settings size={ICON.sm} aria-hidden="true" />
              {account.enabled ? "停用账号" : "启用账号"}
            </button>
            {!account.enabled && !account.has_password && (
              <span className="card-subtitle">先设置口令，再启用账号。</span>
            )}
          </div>
        </>
      )}
    </div>
  );
}
