import { X } from "lucide-react";
import { useEffect } from "react";
import type { ReactNode } from "react";

import { PAGE_SIZE_OPTIONS } from "./listing";

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


/**
 * 列表底部的分页条。**纯客户端分页**：当前 API 没有 `limit/offset`，
 * 这里翻的是已经取回来的那一整份清单——**不装成服务端分页**（ADR-0042 §2 原样有效）。
 *
 * 2026-08-21（ADR-0043 §走查触发 X11）**只改形态不改规则**：
 * 从「第 x / y 页」那对箭头改成参照物那套 `共 N 条` + 页码按钮（当前页填主色）+
 * 上一页 / 下一页 + 每页条数下拉。三条规则一字未动：
 *
 * - **总数不超过一页时整条不出**——只有一页时，页码按钮与两个按不动的箭头只是噪声。
 * - `共 N 条` 里的 N 是**筛完之后**的条数，不是服务端那份的总数：分页翻的就是这一份。
 * - 页码越界一律夹回（判定在 `paginate`，不在这里）。
 *
 * 换每页条数**回到第 1 页**：留在第 7 页而每页从 20 变 100，落点是一屏跟刚才毫无关系的行。
 */
export function Pagination({
  page,
  pageCount,
  total,
  pageSize,
  unit = "条",
  onPage,
  onPageSize,
}: {
  page: number;
  pageCount: number;
  total: number;
  pageSize: number;
  unit?: string;
  onPage: (page: number) => void;
  /** 不给就不出「每页条数」下拉——分页条本身照旧。 */
  onPageSize?: (pageSize: number) => void;
}) {
  // **总数不超过一页时整条不出**——只有一页时，页码按钮与两个按不动的箭头只是噪声。
  // 唯一的例外是「每页条数已经被人改过」：那时把整条藏掉会**关死唯一一条回去的路**
  // （选了 100 / 页，列表一页装下了，于是控件消失，再也换不回 20）。
  // 默认那一档下的行为一字未变，X11 观察到的仍是「一页时整条不出」。
  if (total <= pageSize && (onPageSize === undefined || pageSize === PAGE_SIZE_OPTIONS[0])) {
    return null;
  }
  return (
    <nav className="list-pagination" aria-label="分页">
      <span className="pagination-total">
        共 {total} {unit}
      </span>
      <button
        className="page-btn"
        type="button"
        title="上一页"
        aria-label="上一页"
        disabled={page <= 1}
        onClick={() => onPage(page - 1)}
      >
        ‹
      </button>
      {pageWindow(page, pageCount).map((entry, index) =>
        entry === null ? (
          // 省略号是**占位不是按钮**：能点的页码必须真的能点到。
          <span key={`gap-${index}`} className="pagination-total" aria-hidden="true">
            …
          </span>
        ) : (
          <button
            key={entry}
            className={`page-btn ${entry === page ? "is-active" : ""}`}
            type="button"
            aria-label={`第 ${entry} 页`}
            aria-current={entry === page ? "page" : undefined}
            onClick={() => onPage(entry)}
          >
            {entry}
          </button>
        ),
      )}
      <button
        className="page-btn"
        type="button"
        title="下一页"
        aria-label="下一页"
        disabled={page >= pageCount}
        onClick={() => onPage(page + 1)}
      >
        ›
      </button>
      {onPageSize !== undefined && (
        <select
          className="page-size"
          aria-label="每页条数"
          value={pageSize}
          onChange={(event) => onPageSize(Number(event.target.value))}
        >
          {PAGE_SIZE_OPTIONS.map((size) => (
            <option key={size} value={size}>
              {size} / 页
            </option>
          ))}
        </select>
      )}
    </nav>
  );
}

/**
 * 要摆出来的页码，`null` = 省略号。
 *
 * 窗口固定在当前页两侧，首尾两页**永远在**——「跳回第 1 页」与「跳到最后一页」是翻长列表时
 * 最常用的两个动作，把它们藏进省略号里等于逼人一页一页按过去。
 */
function pageWindow(page: number, pageCount: number): (number | null)[] {
  if (pageCount <= 7) {
    return Array.from({ length: pageCount }, (_, index) => index + 1);
  }
  const around = [page - 1, page, page + 1].filter(
    (candidate) => candidate > 1 && candidate < pageCount,
  );
  const entries: (number | null)[] = [1];
  if (around[0] !== undefined && around[0] > 2) {
    entries.push(null);
  }
  entries.push(...around);
  if (around[around.length - 1] !== undefined && around[around.length - 1] < pageCount - 1) {
    entries.push(null);
  }
  entries.push(pageCount);
  return entries;
}
