import { Eye, EyeOff } from "lucide-react";
import { useState } from "react";

export type RunPhase = "PREPARING" | "STREAMING" | "COMMITTING";

const phases: Array<{ code: RunPhase; label: string }> = [
  { code: "PREPARING", label: "准备中" },
  { code: "STREAMING", label: "传输中" },
  { code: "COMMITTING", label: "提交中" },
];

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

export function TerminalBlock({ effect }: { effect: "SWAPPED" | "DISCARDED" }) {
  return (
    <span className={`terminal-block is-${effect.toLowerCase()}`}>
      <span>{effect}</span>
      <span className="terminal-copy">
        {effect === "SWAPPED" ? "目标表已切换" : "目标表未被触碰"}
      </span>
    </span>
  );
}

export function ErrorCodeTag({
  code,
  httpStatus,
  conclusion,
}: {
  code: string;
  httpStatus: number;
  conclusion: string;
}) {
  const category = httpStatus >= 500 ? "is-internal" : "is-rejected";
  return (
    <span className="error-summary">
      <span>{conclusion}</span>
      <span className={`error-code ${category}`}>
        {code} <span className="http-code">HTTP {httpStatus}</span>
      </span>
    </span>
  );
}

export function SensitiveValue({ column, value }: { column?: string; value: string }) {
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
          <RevealIcon size={14} aria-hidden="true" />
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
        <dt>value</dt>
        <dd>{value}</dd>
      </dl>
    </section>
  );
}
