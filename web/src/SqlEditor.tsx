import { useEffect, useMemo, useRef, useState } from "react";
import { ICON } from "./components/DesignSystem";
import { Maximize2, Minimize2, WrapText } from "lucide-react";

import { formatSql, tokenize } from "./sql";

/**
 * 带高亮的 SQL 输入框——**两处共用**：自定义 SQL 那张卡，以及过滤条件的 WHERE 文本框。
 *
 * 高亮走**透明字 textarea 压在着色 `<pre>` 上**这条老路，不引第三方编辑器：
 * 这两格要的都是「看清结构」，不是补全、折叠、多光标。换成 CodeMirror 要多背 300KB，
 * 还要在它的样式系统和本仓的令牌之间再对一次账。
 *
 * **两层必须逐像素同框，否则光标会飘。** 这条不变式统治这个文件里的每一个决定：
 *
 * - 默认关掉软换行（`wrap="off"`）。开着的话两层要各自跑一遍换行算法，而 textarea 一旦
 *   出竖向滚动条，它的内容宽度会比 `<pre>` 少一条滚动条——同样的文本被排成不同的行，
 *   光标就此错位。关掉之后宽度不参与排版，这类错位在根上不成立，代价是长行要横向滚动。
 * - 打开软换行时（`wrap` 为 true，UX 评审 P1-3 新增），两层用**同一套**换行属性，
 *   并且都写死 `scrollbar-gutter: stable`——滚动条那一条宽度是唯一会让两层内容宽度分家的
 *   东西，两边都预留就再次相等。这不是把不变式放松了，是换了一种方式满足它。
 * - 行号是另画的一层，不进这两层里的任何一层：塞进 `<pre>` 会让两层的字符流不同。
 *   **软换行开着时不出行号**——一条逻辑行占几行显示行，右边第 7 行对不上左边的 7。
 */
export function HighlightedSqlInput({
  value,
  placeholder,
  label,
  required = false,
  rows,
  wrap = false,
  lineNumbers = false,
  textareaRef,
  onChange,
}: {
  value: string;
  placeholder: string;
  /** 无障碍名。这一格永远没有可见的 `<label>`，卡片标题不承担这个角色。 */
  label: string;
  required?: boolean;
  rows: number;
  /** 软换行。默认关，见上面那条不变式。 */
  wrap?: boolean;
  /** 左侧行号槽。软换行开着时自动不出。 */
  lineNumbers?: boolean;
  textareaRef?: React.RefObject<HTMLTextAreaElement | null>;
  onChange: (next: string) => void;
}) {
  const fallbackRef = useRef<HTMLTextAreaElement>(null);
  const inputRef = textareaRef ?? fallbackRef;
  const highlightRef = useRef<HTMLPreElement>(null);
  const gutterRef = useRef<HTMLPreElement>(null);
  const tokens = useMemo(() => tokenize(value), [value]);
  const showGutter = lineNumbers && !wrap;
  const lineCount = useMemo(() => value.split("\n").length, [value]);
  const digits = Math.max(2, String(lineCount).length);

  return (
    <div
      className={`sql-input-shell ${wrap ? "is-wrapped" : ""} ${showGutter ? "has-gutter" : ""}`}
      style={showGutter ? ({ "--sql-gutter": `${digits}ch` } as React.CSSProperties) : undefined}
    >
      {showGutter && (
        <pre className="sql-gutter" aria-hidden="true" ref={gutterRef}>
          {Array.from({ length: lineCount }, (_, index) => index + 1).join("\n")}
        </pre>
      )}
      <pre className="sql-highlight" aria-hidden="true" ref={highlightRef}>
        {tokens.map((token, index) =>
          token.kind === "whitespace" || token.kind === "word" ? (
            token.text
          ) : (
            <span className={`sql-t-${token.kind}`} key={index}>
              {token.text}
            </span>
          ),
        )}
        {/* 末尾换行不补一个字符就撑不出最后那一行的高度，两层的滚动量会差一行。 */}
        {value.endsWith("\n") ? "\n" : ""}
      </pre>
      <textarea
        className="sql-text-input"
        required={required}
        ref={inputRef}
        rows={rows}
        wrap={wrap ? "soft" : "off"}
        spellCheck={false}
        aria-label={label}
        value={value}
        placeholder={placeholder}
        onScroll={() => {
          const textarea = inputRef.current;
          const highlight = highlightRef.current;
          if (textarea === null || highlight === null) {
            return;
          }
          highlight.scrollTop = textarea.scrollTop;
          highlight.scrollLeft = textarea.scrollLeft;
          if (gutterRef.current !== null) {
            gutterRef.current.scrollTop = textarea.scrollTop;
          }
        }}
        onChange={(event) => onChange(event.target.value)}
      />
    </div>
  );
}

/**
 * 自定义 SQL 的编辑器。
 *
 * 2026-08（UX 评审 P1-3）从 300px 的左栏搬进主区，并补上一个真正编辑器该有的东西：
 * 行号、软换行开关、格式化、全屏。理由是用法——**这里的 SQL 基本都是从别处粘过来的**，
 * 粘进来的第一件事是通读一遍确认粘对了，而原来那个 145px 高、300px 宽、不换行的框，
 * 一条三十行的语句要在里面上下左右各滚一遍。
 *
 * 软换行**默认关**：SQL 的缩进是它的结构，折行会把对齐好的 `SELECT` 列表打散。
 * 要读一条没格式化过的长语句时再开，或者直接按「格式化」。
 */
export function SqlEditor({
  value,
  placeholder,
  onChange,
  onFormat,
}: {
  value: string;
  placeholder: string;
  /** 人改了 SQL——结果列可能不再是同一批，调用方要清掉已读的列。 */
  onChange: (next: string) => void;
  /** 只动了空白（`formatSql` 的不变式），语义与结果列都没变，调用方**不该**清列。 */
  onFormat: (next: string) => void;
}) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const formatted = useMemo(() => formatSql(value), [value]);
  const [wrapped, setWrapped] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  // **跟着语句长**，上下都有界：十行以下的语句不该配一个半屏高的空框，
  // 而两百行的语句也不该把下面的映射表推到三屏之外——超过上界的用全屏读。
  const visibleRows = useMemo(
    () => Math.min(26, Math.max(10, value.split("\n").length + 2)),
    [value],
  );

  // 全屏时 Escape 归这里管，而且要在向导那个 window 上的退出监听之前拦下来：
  // 否则按一下 Escape 是「退出整个向导」，而人只是想收起全屏。捕获阶段先跑，
  // `stopImmediatePropagation` 把同一目标上剩下的监听也挡掉。
  useEffect(() => {
    if (!fullscreen) {
      return;
    }
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") {
        return;
      }
      event.stopImmediatePropagation();
      event.preventDefault();
      setFullscreen(false);
    }
    window.addEventListener("keydown", handleKeyDown, true);
    return () => window.removeEventListener("keydown", handleKeyDown, true);
  }, [fullscreen]);

  return (
    <div className={`source-sql-editor ${fullscreen ? "is-fullscreen" : ""}`}>
      <div className="sql-editor-toolbar">
        {/* 卡片标题已经写着「自定义 SQL」，这里不再挂一个同名标签。 */}
        <small className="spec-note">
          读取列后可以只勾要搬的列。实际执行时会在这条 SQL 外层套一层投影，
          只取勾选的列并改名成目标字段——没勾的列不会过线。
        </small>
        <span className="sql-editor-tools">
          <button
            className={`button is-ghost ${wrapped ? "is-on" : ""}`}
            type="button"
            aria-pressed={wrapped}
            title={wrapped ? "关掉软换行：长行改为横向滚动，行号回来" : "打开软换行：长行折行显示，行号会关掉"}
            onClick={() => setWrapped((current) => !current)}
          >
            <WrapText size={ICON.sm} />
            换行
          </button>
          <button
            className="button is-ghost"
            type="button"
            /* 已经是格式化后的样子就禁用：点一下什么都不动，比让人怀疑「按钮坏了」好。 */
            disabled={value.trim() === "" || formatted === value}
            title="按子句换行重排，只动空白，不改任何一个字符"
            onClick={() => {
              onFormat(formatted);
              textareaRef.current?.focus();
            }}
          >
            格式化
          </button>
          <button
            className="button is-ghost"
            type="button"
            title={fullscreen ? "退出全屏（Esc）" : "全屏编辑"}
            aria-label={fullscreen ? "退出全屏" : "全屏编辑"}
            onClick={() => setFullscreen((current) => !current)}
          >
            {fullscreen ? <Minimize2 size={ICON.sm} /> : <Maximize2 size={ICON.sm} />}
          </button>
        </span>
      </div>
      <HighlightedSqlInput
        value={value}
        placeholder={placeholder}
        label="自定义 SQL"
        rows={fullscreen ? 32 : visibleRows}
        wrap={wrapped}
        lineNumbers
        textareaRef={textareaRef}
        onChange={onChange}
      />
    </div>
  );
}
