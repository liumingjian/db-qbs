import { ChevronLeft, Info, KeyRound, LogOut, UserRound } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { AboutPanel } from "./AboutPanel";
import { ICON } from "./components/DesignSystem";

/**
 * 顶栏右上角的用户入口：**修改口令 / 关于 / 退出登录**。
 *
 * 「关于」原来是这个位置上一颗独立的按钮（UX 评审 P1-12）。它并进来，是因为顶栏右上角
 * 一共只有这一格：并排摆两颗按钮，第二颗就得跟第一颗抢那点宽度，而其中一颗还只是
 * 一份装机时看一次的只读清单。所有人闭眼都知道去右上角找「退出登录」，
 * 「关于」跟着搬进同一个抽屉，比自己占一格更容易被找到。
 *
 * **不是模态**，与它取代的那颗按钮同一形态：Escape 与点外面都关，打开时焦点进浮层，
 * 关掉时回到触发它的按钮。不装焦点陷阱——把 Tab 圈在一个三行的菜单里毫无道理。
 *
 * 浮层里有两屏（菜单 / 关于），不是把关于摊在菜单下面：那份清单有二十来行，
 * 摊开会把「退出登录」推到一屏之外，而它是这颗按钮最常被点的那一项。
 */
export function UserMenu({
  username,
  onChangePassword,
  onLogout,
}: {
  username: string;
  onChangePassword: () => void;
  onLogout: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [view, setView] = useState<"menu" | "about">("menu");
  const wrap = useRef<HTMLSpanElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);

  function close() {
    setOpen(false);
    // 下次打开一律从菜单起步：上一次翻到「关于」是上一次的事，
    // 隔了半天再点开却撞上一屏配置清单，会让人以为自己点错了。
    setView("menu");
  }

  useEffect(() => {
    if (!open) return;
    panel.current?.focus();
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setOpen(false);
        setView("menu");
        trigger.current?.focus();
      }
    }
    function handleMouseDown(event: MouseEvent) {
      if (
        event.target instanceof Node &&
        wrap.current?.contains(event.target) !== true
      ) {
        setOpen(false);
        setView("menu");
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("mousedown", handleMouseDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("mousedown", handleMouseDown);
    };
  }, [open]);

  return (
    <span className="about-wrap" ref={wrap}>
      <button
        className="button is-ghost about-trigger"
        type="button"
        ref={trigger}
        aria-expanded={open}
        onClick={() => (open ? close() : setOpen(true))}
      >
        <UserRound size={ICON.sm} aria-hidden="true" />
        {username}
      </button>
      {open && (
        <div
          className={`about-popover ${view === "menu" ? "is-menu" : ""}`}
          ref={panel}
          role="dialog"
          aria-label={view === "about" ? "关于 db-qbs" : "账号"}
          tabIndex={-1}
        >
          {view === "menu" ? (
            <>
              <header>
                <strong>{username}</strong>
                <span>已登录</span>
              </header>
              <div className="user-menu">
                <button
                  type="button"
                  onClick={() => {
                    close();
                    onChangePassword();
                  }}
                >
                  <KeyRound size={ICON.sm} aria-hidden="true" />
                  修改口令
                </button>
                <button type="button" onClick={() => setView("about")}>
                  <Info size={ICON.sm} aria-hidden="true" />
                  关于 db-qbs
                </button>
                {/* 退出摆在最末、单独一组：它是这个菜单里唯一一个**做完就走**的动作。 */}
                <button
                  className="is-parting"
                  type="button"
                  onClick={() => {
                    close();
                    onLogout();
                  }}
                >
                  <LogOut size={ICON.sm} aria-hidden="true" />
                  退出登录
                </button>
              </div>
            </>
          ) : (
            <>
              <header>
                <button
                  className="text-button user-menu-back"
                  type="button"
                  onClick={() => setView("menu")}
                >
                  <ChevronLeft size={ICON.sm} aria-hidden="true" />
                  返回
                </button>
                <span>本版设置只读</span>
              </header>
              <AboutPanel />
            </>
          )}
        </div>
      )}
    </span>
  );
}
