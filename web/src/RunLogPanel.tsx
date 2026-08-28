import { useEffect, useId, useReducer, useRef, useState } from "react";

import { fetchRunLogs } from "./api";
import { messageFrom } from "./errors";
import {
  INITIAL_FOLLOW,
  backToLatestLabel,
  follow,
  showsBackToLatest,
} from "./logFollow";
import { formatRunLogLines } from "./runLogLine";
import type { LogLineView } from "./runLogLine";

/**
 * 一次运行的日志面板。
 *
 * 它是**两个纯模块的壳**：`runLogLine.ts` 把原文翻成人话，`logFollow.ts` 决定还跟不跟着
 * 往下滚。这里只剩三件带副作用的事——按游标轮询、把滚动条推到底、把滚动位置喂回状态机。
 * 判断全在纯模块里，所以这一层没有能被测出来的逻辑，也就不需要一个浏览器来测它。
 *
 * 轮询节奏抄的是运行详情屏那一份（`RunScreen.tsx`）：自重排的 `setTimeout`，
 * 只在「还活着 + 页面可见 + 手上没有在飞的请求」时续下一次。
 * **不用 SSE、不开长连接**：source 是同步阻塞栈、没有异步运行时，
 * 一条挂着不放的连接会整根占死一个工作线程。
 */
const LOG_POLL_INTERVAL_MS = 1000;

export function RunLogPanel({
  runRecordId,
  focus = false,
  embedded = false,
}: {
  runRecordId: string;
  /** 深链 `#runs/<id>/logs` 进来时为真：进屏就把这一段滚到眼前。 */
  focus?: boolean;
  /**
   * 摆在运行详情抽屉里（而不是整屏详情里）。
   *
   * 只换外壳：抽屉里的每一段都是 `.panel` + `<h3>`，整屏那边是 `.card` + `<h2>`。
   * 里面那份日志一个字不变——两处看到的必须是同一份东西，否则「详情里有日志」
   * 这句话就得分两种说法。
   */
  embedded?: boolean;
}) {
  const [lines, setLines] = useState<LogLineView[]>([]);
  const [followState, dispatch] = useReducer(follow, INITIAL_FOLLOW);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [live, setLive] = useState(true);
  const headingId = useId();
  const listRef = useRef<HTMLOListElement | null>(null);
  const sectionRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    let effectActive = true;
    let runIsLive = true;
    let requestInFlight = false;
    /** 游标：已经拿到的最后一个 `seq`。换一条运行就从 0 重来。 */
    let cursor = 0;
    let pollTimer: number | undefined;

    setLines([]);
    setLoaded(false);
    setLoadError(null);
    setLive(true);

    function canLoadLogs() {
      return (
        effectActive &&
        runIsLive &&
        document.visibilityState === "visible" &&
        !requestInFlight
      );
    }

    function scheduleNextLoad(delayMs: number) {
      if (!canLoadLogs() || pollTimer !== undefined) {
        return;
      }
      pollTimer = window.setTimeout(() => {
        pollTimer = undefined;
        void load();
      }, delayMs);
    }

    async function load() {
      if (!canLoadLogs()) {
        return;
      }
      requestInFlight = true;
      // 一页取满时立刻带着新游标再来一次：积压的行不该排队等下一个轮询周期。
      let nextDelay = LOG_POLL_INTERVAL_MS;
      try {
        const page = await fetchRunLogs(runRecordId, cursor);
        if (!effectActive) {
          return;
        }
        cursor = page.next_after;
        runIsLive = page.live;
        nextDelay = page.has_more ? 0 : LOG_POLL_INTERVAL_MS;
        if (page.lines.length > 0) {
          const appended = formatRunLogLines(page.lines);
          setLines((previous) => [...previous, ...appended]);
          dispatch({ type: "appended", count: appended.length });
        }
        setLive(page.live);
        setLoaded(true);
        setLoadError(null);
      } catch (error) {
        if (effectActive) {
          setLoadError(messageFrom(error));
        }
      } finally {
        requestInFlight = false;
        scheduleNextLoad(nextDelay);
      }
    }

    function handleVisibilityChange() {
      if (pollTimer !== undefined) {
        window.clearTimeout(pollTimer);
        pollTimer = undefined;
      }
      if (canLoadLogs()) {
        void load();
      }
    }

    document.addEventListener("visibilitychange", handleVisibilityChange);
    void load();
    return () => {
      effectActive = false;
      if (pollTimer !== undefined) {
        window.clearTimeout(pollTimer);
      }
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  }, [runRecordId]);

  // 跟随时把滚动条推到底。推到底本身会再触发一次滚动事件，但它落在底部，
  // 状态机据此判「继续跟随」——所以这里不需要一个「这次是我自己滚的」标志位。
  useEffect(() => {
    if (!followState.following) {
      return;
    }
    const node = listRef.current;
    if (node !== null) {
      node.scrollTop = node.scrollHeight;
    }
  }, [lines, followState.following]);

  useEffect(() => {
    if (focus) {
      sectionRef.current?.scrollIntoView?.({ block: "start" });
    }
  }, [focus, runRecordId]);

  /** 标题与右边那组动作是**两个兄弟节点**：卡片头靠 `space-between` 把它们推开，
      抽屉里那个 `<h3>` 做同一件事。可读名字只取标题，不把「实时」念进去。 */
  const title = <span id={headingId}>运行日志</span>;
  const actions = (
    <span className="run-logs-actions">
      {live && <span className="run-logs-live">实时</span>}
      {showsBackToLatest(followState) && (
        <button
          type="button"
          className="button is-ghost"
          onClick={() => dispatch({ type: "back-to-latest" })}
        >
          {backToLatestLabel(followState)}
        </button>
      )}
    </span>
  );

  return (
    <section
      className={embedded ? "panel run-logs is-embedded" : "card run-logs"}
      aria-labelledby={headingId}
      ref={sectionRef}
    >
      {embedded ? (
        <h3>
          {title}
          {actions}
        </h3>
      ) : (
        <header className="card-header">
          <h2>{title}</h2>
          {actions}
        </header>
      )}
      {loadError !== null && (
        <div className="form-error" role="alert">
          {loadError}
        </div>
      )}
      <ol
        className="log-lines"
        role="log"
        ref={listRef}
        onScroll={(event) => {
          const node = event.currentTarget;
          dispatch({
            type: "scrolled",
            distanceFromBottom: node.scrollHeight - node.scrollTop - node.clientHeight,
          });
        }}
      >
        {lines.map((line) => (
          <li
            key={line.seq}
            className={`log-line is-${line.tone}${line.known ? "" : " is-unknown"}`}
          >
            <span className="log-time">{line.time}</span>
            {/* 认不出来的事件在这里自陈身份：屏幕上多一个看不懂的词，
                好过把「两端版本不一致」这件事抹平成一片安静。 */}
            <span className="log-event" title={line.known ? undefined : "未知事件，按原样显示"}>
              {line.event}
            </span>
            <span className="log-text">{line.text}</span>
          </li>
        ))}
      </ol>
      {lines.length === 0 && (
        <p className="empty-value">
          {!loaded
            ? "正在读取日志…"
            : live
              ? "这次运行还没有写下任何日志。"
              : "没有日志可看。原始日志只保留 7 天，更早的运行只剩下运行历史那一行。"}
        </p>
      )}
    </section>
  );
}
