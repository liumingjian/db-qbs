import { X } from "lucide-react";
import { useEffect } from "react";
import type { ReactNode } from "react";

/**
 * 任务屏与数据源屏共用的四个外壳件。
 *
 * 它们原来长在 `App.tsx` 里，数据源屏（ADR-0039 §1~§4）要用同一套对话框与表单行，
 * 于是搬到这里——**一个字都没改形态**，只换了住处。这不是新组件：
 * `docs/design-system/README.md` §7 的组件清单不因此增减（ADR-0039 §9「零设计系统改动」）。
 */
export function Modal({
  title,
  onClose,
  busy,
  narrow = false,
  wide = false,
  children,
}: {
  title: string;
  onClose: () => void;
  busy: boolean;
  narrow?: boolean;
  wide?: boolean;
  children: ReactNode;
}) {
  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !busy) {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [busy, onClose]);

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !busy) {
          onClose();
        }
      }}
    >
      <section
        className={`modal ${narrow ? "is-narrow" : ""} ${wide ? "is-wide" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby="modal-title"
      >
        <header className="modal-header">
          <h2 id="modal-title">{title}</h2>
          <button
            className="icon-button"
            type="button"
            title="关闭"
            aria-label="关闭"
            onClick={onClose}
            disabled={busy}
          >
            <X size={16} aria-hidden="true" />
          </button>
        </header>
        {children}
      </section>
    </div>
  );
}

export function ModalFooter({
  onClose,
  busy,
  submitLabel,
  submitDisabled = false,
}: {
  onClose: () => void;
  busy: boolean;
  submitLabel: string;
  /** 「测通才让存」（ADR-0039 §3）：门槛没过时按钮禁用，但那不是「正在保存」。 */
  submitDisabled?: boolean;
}) {
  return (
    <footer className="modal-footer">
      <button
        className="button is-ghost"
        type="button"
        onClick={onClose}
        disabled={busy}
      >
        取消
      </button>
      <button
        className="button is-primary"
        type="submit"
        disabled={busy || submitDisabled}
      >
        {busy ? "正在保存" : submitLabel}
      </button>
    </footer>
  );
}

export function FormField({
  label,
  badge,
  neutralBadge = false,
  children,
}: {
  label: string;
  badge?: string;
  /**
   * 中性徽标（`.field-badge.is-neutral`，ADR-0039 §9 表第 2 条）。
   * 口令的「已设置 / 未设置」是**事实陈述，不是成功或失败**，所以它不着成功色也不着告警色。
   */
  neutralBadge?: boolean;
  children: ReactNode;
}) {
  return (
    <label className="form-field">
      <span className="field-label">
        {label}
        {badge !== undefined && (
          <span className={`field-badge ${neutralBadge ? "is-neutral" : ""}`}>
            {badge}
          </span>
        )}
      </span>
      {children}
    </label>
  );
}

/**
 * 表格行里的图标动作位。`title` 默认就是 `label`——只有需要说明**为什么按不动**时才另给
 * （禁用态的按钮自己不会解释自己，原因只能挂在悬停提示上）。
 */
export function ActionButton({
  label,
  icon,
  danger = false,
  disabled = false,
  title,
  onClick,
}: {
  label: string;
  icon: ReactNode;
  danger?: boolean;
  disabled?: boolean;
  title?: string;
  onClick: () => void;
}) {
  return (
    <button
      className={`icon-button ${danger ? "is-danger" : ""}`}
      type="button"
      title={title ?? label}
      aria-label={label}
      disabled={disabled}
      onClick={onClick}
    >
      {icon}
    </button>
  );
}
