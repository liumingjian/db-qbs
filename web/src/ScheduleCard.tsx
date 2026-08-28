/**
 * 周期调度那一格（#265）——**向导与任务清单共用同一份**。
 *
 * 两处要说的是同一件事：一行 cron 原文、一个启停开关、以及「服务器那台机器什么时候会
 * 真的把它发出去」。抄成两份的话，时区那句话、防抖那 300ms、「永远不会触发」那条判定
 * 迟早会在两屏上说出两个答案——而这一格存在的全部理由，就是让人事先看到那个答案。
 *
 * 它自己不保存任何东西：值从外面进来，改动从回调出去。谁来落盘由调用方决定——
 * 向导落进草稿，清单上那个对话框直接 `PUT` 回去。
 */

import { useEffect, useId, useState } from "react";

import { fetchSchedulePreview } from "./api";
import type { SchedulePreview } from "./api";
import { messageFrom } from "./errors";

/** 停手多久之后才去问一次「下次触发」。边敲边问只会让红字在打字时一直闪。 */
const SCHEDULE_PREVIEW_DEBOUNCE_MS = 300;

export function ScheduleCard({
  cron,
  enabled,
  onCron,
  onEnabled,
}: {
  cron: string;
  enabled: boolean;
  onCron: (cron: string) => void;
  onEnabled: (enabled: boolean) => void;
}) {
  const [preview, setPreview] = useState<SchedulePreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const cronInputId = useId();

  useEffect(() => {
    let abandoned = false;
    // 边敲边问会把每一个中间态都发出去，而中间态几乎全是非法的——那会让红字在人打字时
    // 一直闪。停手之后再问。
    const timer = window.setTimeout(() => {
      void (async () => {
        try {
          const answer = await fetchSchedulePreview(cron.trim() === "" ? null : cron);
          if (!abandoned) {
            setPreview(answer);
            setError(null);
          }
        } catch (failure) {
          if (!abandoned) {
            setError(messageFrom(failure));
          }
        }
      })();
    }, SCHEDULE_PREVIEW_DEBOUNCE_MS);
    return () => {
      abandoned = true;
      window.clearTimeout(timer);
    };
  }, [cron]);

  const configured = cron.trim() !== "";
  return (
    <section className="schedule-card">
      <header>
        <div>
          <strong>周期调度</strong>
          <span>到点自动发起这个任务</span>
        </div>
        <label className="schedule-switch">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(event) => onEnabled(event.target.checked)}
          />
          <span>{enabled ? "已启用" : "已停用"}</span>
        </label>
      </header>
      <label className="schedule-expression" htmlFor={cronInputId}>
        cron 表达式
        <input
          id={cronInputId}
          value={cron}
          placeholder="0 2 * * *（分 时 日 月 周）"
          spellCheck={false}
          onChange={(event) => onCron(event.target.value)}
        />
      </label>
      {/* 时区永远在，哪怕还没写表达式——它是这一格里唯一一句不依赖输入的话。 */}
      <dl className="schedule-readout">
        <div>
          <dt>时区</dt>
          <dd>
            {preview === null
              ? "读取中…"
              : `服务器本地时区 ${preview.timezone}（UTC${preview.utc_offset}），此刻 ${preview.now}`}
          </dd>
        </div>
        <div>
          <dt>下次触发</dt>
          <dd>
            {error !== null ? (
              <span className="schedule-error">{error}</span>
            ) : !configured ? (
              <span className="schedule-none">没配周期，只能手动发起</span>
            ) : preview === null ? (
              "读取中…"
            ) : preview.next_fire_times.length === 0 ? (
              <span className="schedule-error">这条表达式永远不会触发</span>
            ) : (
              preview.next_fire_times.map((fire) => (
                <span className="schedule-fire" key={fire}>
                  {fire}
                </span>
              ))
            )}
          </dd>
        </div>
      </dl>
    </section>
  );
}
