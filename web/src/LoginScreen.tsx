import { Eye, EyeOff, Lock, User } from "lucide-react";
import { useState } from "react";
import type { FormEvent } from "react";

import { login } from "./api";
import type { SessionState } from "./api";
import { messageFrom } from "./errors";
import { loginGate } from "./session";

/**
 * 登录页——**整个产品里唯一的深色整屏**，也是唯一一屏不在应用外壳里：
 * 没有侧栏，没有顶栏，没有面包屑。那三样都是「已经进来了」的语汇。
 *
 * 形态照参考图（`cover.png`，x2doris 的登录页）：分屏，左宣传右卡片，深蓝底上
 * 压一枚更亮的大圆。这与设计系统那句「照搬 x2doris」是同一条裁定，不是新起一套。
 *
 * **左半屏没有按钮。** 参考图那边挂着「使用文档」「反馈问题」两个外链，而 db-qbs
 * 是内部工具，没有文档站也没有反馈渠道——照抄只会得到两个点了没反应的按钮。
 * 摆的是产品名、一句话说明和版本号：装机和排障时第一眼想知道的就是版本，
 * 而版本号是这一屏上**唯一**对还没通过认证的人可见的事实，构建细节一个字不给。
 */
export function LoginScreen({
  onAuthenticated,
}: {
  onAuthenticated: (session: SessionState) => void;
}) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [revealed, setRevealed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const gate = loginGate({ username, password });

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (gate.kind === "blocked" || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      onAuthenticated(await login(username.trim(), password));
    } catch (caught) {
      setError(messageFrom(caught));
      // 失败时**只清口令，不清账号**：重打一遍账号是纯粹的惩罚，
      // 而留着填错的口令会让下一次尝试变成「在错字上改错字」。
      setPassword("");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="login-screen">
      <section className="login-pitch">
        <h1>
          让 Oracle 的数据
          <br />
          落进 MySQL
          <br />
          <strong>更简单</strong>
        </h1>
        <p>
          db-qbs 是一套离线数据导入工具：在源端查 Oracle，把结果整批搬进目标端的
          MySQL。建表语句、字段映射与一致性校验都在流程里，点几下就能跑完一次搬运。
        </p>
        <span className="login-version">版本 {__APP_VERSION__}</span>
      </section>

      <form className="login-card" onSubmit={submit}>
        <span className="login-brand">db-qbs</span>

        <label className="login-field">
          <User size={16} aria-hidden="true" />
          <input
            type="text"
            name="username"
            autoComplete="username"
            placeholder="账号"
            aria-label="账号"
            value={username}
            disabled={busy}
            onChange={(event) => setUsername(event.target.value)}
          />
        </label>

        <label className="login-field">
          <Lock size={16} aria-hidden="true" />
          <input
            type={revealed ? "text" : "password"}
            name="password"
            autoComplete="current-password"
            placeholder="口令"
            aria-label="口令"
            value={password}
            disabled={busy}
            onChange={(event) => setPassword(event.target.value)}
          />
          {/* 明文开关**不是装饰**：这一栏没有「忘记口令」可点，打错了只能重来，
              而重来的前提是看得见自己打了什么。 */}
          <button
            className="login-reveal"
            type="button"
            title={revealed ? "隐藏口令" : "显示口令"}
            aria-label={revealed ? "隐藏口令" : "显示口令"}
            aria-pressed={revealed}
            onClick={() => setRevealed((current) => !current)}
          >
            {revealed ? (
              <EyeOff size={16} aria-hidden="true" />
            ) : (
              <Eye size={16} aria-hidden="true" />
            )}
          </button>
        </label>

        {/* 失败那句话占着一个**固定的位置**，不挤动下面的按钮：
            按钮在报错的一瞬间往下跳，正好赶上有人第二次点它。 */}
        <div className="login-error" role="alert">
          {error ?? (gate.kind === "blocked" && password !== "" ? gate.reason : "")}
        </div>

        <button
          className="button is-primary login-submit"
          type="submit"
          disabled={gate.kind === "blocked" || busy}
        >
          {busy ? "登录中…" : "登 录"}
        </button>
      </form>
    </div>
  );
}
