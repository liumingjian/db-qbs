import { useState } from "react";
import type { FormEvent } from "react";

import { changePassword } from "./api";
import { messageFrom } from "./errors";
import {
  emptyPasswordForm,
  gateReasonVisible,
  passwordGate,
} from "./session";
import type { PasswordForm } from "./session";
import { FormField, Modal, ModalFooter } from "./ui";

/**
 * 改口令。**要先输当前口令**——改密入口挂在一个已经登录的会话后面，而「已经登录」
 * 证明的是这台浏览器有票据，不是坐在它前面的还是同一个人。
 *
 * 改完**除了当前这一张之外的会话全部失效**（服务端做的，见 `AuthStore::change_password`）。
 * 那正是改口令这个动作该有的后果：它的常见动机就是「我怀疑别处有人登着」。
 * 这句话摆在对话框里而不是藏进文档——别处那台浏览器下一次点击就会被弹回登录页，
 * 事先不知道的话，那看起来像一次故障。
 */
export function ChangePasswordDialog({
  onClose,
  onChanged,
}: {
  onClose: () => void;
  /** 改成功之后的去处。当前这张票留着，所以**不回登录页**，只报一句然后关掉。 */
  onChanged: () => void;
}) {
  const [form, setForm] = useState<PasswordForm>(emptyPasswordForm);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const gate = passwordGate(form);

  function edit(patch: Partial<PasswordForm>) {
    setForm((current) => ({ ...current, ...patch }));
    setError(null);
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (gate.kind === "blocked" || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await changePassword(form.current, form.next);
      onChanged();
    } catch (caught) {
      // 「当前口令不正确」就是从这里读到的：服务端判，前端不预判。
      setError(messageFrom(caught));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal title="修改口令" onClose={onClose} busy={busy} narrow>
      <form onSubmit={(event) => void submit(event)}>
        <div className="modal-body form-stack">
          <FormField label="当前口令">
            <input
              autoFocus
              type="password"
              autoComplete="current-password"
              value={form.current}
              disabled={busy}
              onChange={(event) => edit({ current: event.target.value })}
            />
          </FormField>

          <FormField label="新口令">
            <input
              type="password"
              autoComplete="new-password"
              value={form.next}
              disabled={busy}
              onChange={(event) => edit({ next: event.target.value })}
            />
          </FormField>

          <FormField label="再输一次新口令">
            <input
              type="password"
              autoComplete="new-password"
              value={form.confirm}
              disabled={busy}
              onChange={(event) => edit({ confirm: event.target.value })}
            />
          </FormField>

          {error !== null && <p className="form-error">{error}</p>}
          {error === null && gate.kind === "blocked" && gateReasonVisible(form) && (
            <p className="form-error">{gate.reason}</p>
          )}

          <p className="modal-context">
            改完之后，<strong>这台浏览器继续登着</strong>
            ，其它地方登着的同一个账号会立刻失效，下一次点击就会被弹回登录页。
            忘了口令没有找回入口：只能在 source 主机上跑{" "}
            <code>db-qbs-source reset-password --config &lt;source.toml&gt;</code>
            ，口令会回到出厂值。
          </p>
        </div>

        <ModalFooter
          onClose={onClose}
          busy={busy}
          submitDisabled={gate.kind === "blocked"}
          submitLabel="修改"
        />
      </form>
    </Modal>
  );
}
