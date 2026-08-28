import { Eye, EyeOff } from "lucide-react";
import { useState } from "react";

import type { RunPhase } from "../runStage";
import { RUN_PHASES, stageLabel } from "../runStage";

const phases: Array<{ code: RunPhase; label: string }> = RUN_PHASES.map(
  (code) => ({ code, label: stageLabel(code) }),
);

/**
 * 图标尺寸的**唯一一张表**（2026-08 UX 评审 P2）。
 *
 * 原来它是逐处手写的：13、14、15、16、17、18、22 七个数散在八个文件里，同一个角色
 * 在两屏上差一两个像素——差得看不出来，也就永远改不回来。lucide 的 `size` 是 JS 数字，
 * 进不了 CSS 变量，所以这张表在 TS 这边；`tokens.css` 里那三条 `--icon-*` 是它的另一份，
 * 供 CSS 侧对齐，两边要一起改。
 */
export const ICON = {
  /** 按钮里、文字行内。与正文齐平。 */
  sm: 14,
  /** 表格行内动作位、卡片工具条。 */
  md: 16,
  /** 抽屉与对话框的关闭键——容器更大，图标跟着大一档。 */
  lg: 18,
  /** 空状态里那枚示意图标。它不是控件，是插图，所以不在上面三档里。 */
  empty: 22,
} as const;

export function PhaseLine({ current }: { current: RunPhase | null }) {
  const currentIndex = current === null ? -1 : phases.findIndex(({ code }) => code === current);
  return (
    <div className="phase-line" aria-label="运行阶段">
      {phases.map((phase, index) => (
        <span className="phase-item-wrap" key={phase.code}>
          {index > 0 && <span className="phase-arrow" aria-hidden="true">→</span>}
          <span
            className={`phase-item ${index < currentIndex ? "is-done" : ""} ${index === currentIndex ? "is-current" : ""}`}
          >
            <span className="phase-dot" aria-hidden="true" />
            <span>{phase.code}</span>
            <span className="phase-label">{phase.label}</span>
          </span>
        </span>
      ))}
      <span className="phase-after">→ 终态待定</span>
    </div>
  );
}

/**
 * 轴二有三个词，而**只有一个是「整表换过」**（2026-08 UX 评审 P0-1、#264）。
 *
 * `SWAPPED` 是目标端的协议词，本义只是「这次写入被目标端认下了」；实际打的是
 * `INSERT ... ON DUPLICATE KEY UPDATE`（无主键时是纯 `INSERT`，见 `writeMode.ts`）
 * ——**按主键合并**。原话「目标表已切换」把它读成了
 * 一次整表替换，于是目标表会被当成源端的全量快照拿去用，而**源端删掉的行还留在里面**
 * （CONTEXT.md 记的那笔刻意欠债）。
 *
 * `REPLACED` 才是整表换过：先清空再导入那一档（#264），跑完之后目标表精确等于本次
 * 查询的结果。它和 `SWAPPED` **不能共用一句话**——那正是这一栏存在的意义，
 * 而共用会让一半的运行历史读起来是假的。
 *
 * `DISCARDED` 一个字不动：那半边本来就是准的——目标表确实没被碰过。
 *
 * 标签本身照旧是一个词，长话在 `UpsertNote` 里。
 */
const TERMINAL_COPY: Readonly<Record<string, string>> = {
  SWAPPED: "已按主键合并写入",
  REPLACED: "目标表已整表替换",
  DISCARDED: "目标表未被触碰",
  UNKNOWN: "目标表效果未知",
};

/**
 * 轴二。`effect` 是**服务端给的原字符串**，不是筛过的枚举（见 `history.ts`）。
 *
 * 认得的词配一句人话；不认得的词照样摆出来，只是没有那句人话可配——产品不认识它，
 * 也就没有资格替它说是什么意思。吞掉它才是最坏的一种：那意味着跑数的那一端比这块
 * 屏幕新，而这件事只会在这一格里露头。
 */
export function TerminalBlock({ effect }: { effect: string }) {
  const copy = TERMINAL_COPY[effect] ?? null;
  return (
    <span className={`terminal-block is-${copy === null ? "unrecognised" : effect.toLowerCase()}`}>
      <span>{effect}</span>
      {copy !== null && <span className="terminal-copy">{copy}</span>}
    </span>
  );
}

// 这两句话原来是常量，现在是 `writeMode.ts` 里的两个函数（#261）：目标表无主键时
// 「按主键 upsert」是句假话，而恰恰是那条路最需要这行字。留下的只有渲染壳
// `UpsertNote`，文本由调用方按写法算出来给它。
/**
 * 跟在轴二后面的那一行长话——**语义常驻，不是错误也不是告警**。
 *
 * 它不着 --crit / --warn：这不是出了问题，是这个产品的写入语义本来如此。
 * 常驻是有意的（不折叠、不「知道了」关掉）：这条边界每次都要读到，
 * 一旦被收起来，第一次用的人就又只剩「SWAPPED」这一个词可读了。
 */
export function UpsertNote({ text }: { text: string }) {
  return <p className="upsert-note">{text}</p>;
}

/**
 * 「结局不明」那一格。运行详情整屏与抽屉两处**共用这一份**。
 *
 * `reason` 可空——旧记录里就是空的。空的时候不挂修饰类：原来两处各写一遍
 * `is-${reason?.toLowerCase()}`，于是 `null` 会渲染成 `class="… is-undefined"`
 * （2026-08 UX 评审复审）。CSS 里只有 `.is-service_restarted` 一条，
 * 别的取值本来也只是挂着好看。
 */
export function UnknownConclusion({
  reason,
  conclusion,
}: {
  reason: string | null;
  conclusion: string;
}) {
  const modifier = reason === null ? "" : ` is-${reason.toLowerCase()}`;
  return (
    <div className={`unknown-conclusion${modifier}`}>
      <strong>结局不明</strong>
      <span>{conclusion}</span>
      <small>无法确认目标表是否被修改，请到目标库核对。</small>
    </div>
  );
}

export function ErrorCodeTag({
  code,
  httpStatus,
  conclusion,
}: {
  code: string;
  httpStatus?: number;
  conclusion: string;
}) {
  const category =
    httpStatus === undefined || httpStatus >= 500
      ? "is-internal"
      : "is-rejected";
  // 设计系统 §64「码名右边永远跟一句中文人话结论」：标签在前，结论在右。
  return (
    <span className="error-summary">
      <span className={`error-code ${category}`}>
        {code}
        {httpStatus !== undefined && (
          <span className="http-code">HTTP {httpStatus}</span>
        )}
      </span>
      <span>{conclusion}</span>
    </span>
  );
}

export function SensitiveValue({
  column,
  value,
}: {
  column?: string;
  value?: string;
}) {
  const [revealed, setRevealed] = useState(false);
  const RevealIcon = revealed ? EyeOff : Eye;
  return (
    <section className="sensitive-value" aria-label="源库业务值">
      <header>
        <span>源库真实业务值</span>
        <span className="sensitive-warning">显示即把源库真实值送进这台浏览器</span>
        <button
          className="sensitive-toggle"
          type="button"
          onClick={() => setRevealed((valueIsRevealed) => !valueIsRevealed)}
        >
          <RevealIcon size={ICON.sm} aria-hidden="true" />
          {revealed ? "隐藏" : "显示"}
        </button>
      </header>
      <dl className={revealed ? "" : "is-masked"}>
        {column !== undefined && (
          <>
            <dt>column</dt>
            <dd>{column}</dd>
          </>
        )}
        {value !== undefined && (
          <>
            <dt>value</dt>
            <dd>{value}</dd>
          </>
        )}
      </dl>
    </section>
  );
}
