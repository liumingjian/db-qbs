import { useMemo, useRef, useState } from "react";
import type { RefObject } from "react";
import { ICON } from "./components/DesignSystem";
import { Maximize2, Minimize2, WrapText } from "lucide-react";

import { useDialogFocus } from "./dialogFocus";
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
/**
 * 只读 SQL 的高亮。可编辑的那个是 `HighlightedSqlInput`，两者共用 `tokenize`。
 *
 * 住在这里而不是向导里，是因为读 SQL 的地方不止向导一处：运行详情抽屉的
 * `pre.drawer-sql` 与失败证据里的 `.evidence-sql` 原来都是纯文本
 * （2026-08 UX 评审 P1-3）——恰恰是出了事回头核对那一句查询的时候，最需要一眼
 * 认出哪个是字符串字面量、哪个是被双引号括起来的标识符。
 */
export function HighlightedSql({ sql }: { sql: string }) {
  return <>{tokenize(sql).map((token, index) =>
    token.kind === "whitespace" || token.kind === "word"
      ? token.text
      : <span className={`sql-t-${token.kind}`} key={index}>{token.text}</span>,
  )}</>;
}

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

type SqlEditorProps = {
  value: string;
  placeholder: string;
  /** 人改了 SQL——结果列可能不再是同一批，调用方要清掉已读的列。 */
  onChange: (next: string) => void;
  /** 只动了空白（`formatSql` 的不变式），语义与结果列都没变，调用方**不该**清列。 */
  onFormat: (next: string) => void;
};

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
export function SqlEditor(props: SqlEditorProps) {
  const [fullscreen, setFullscreen] = useState(false);

  // 全屏时按 Escape 是收起全屏，这件事**没有**单独的监听：全屏层挂着的时候它就是
  // `useDialogFocus` 排队里最上面那一层，按键归它管（见下面的 `FullscreenFocusTrap`）。
  // 这里原来还有一段捕获阶段的拦截，专门抢在向导那个 window 级退出监听之前
  // `stopImmediatePropagation`；向导的监听收进向导容器、并且会向这个队列让路之后，
  // 那段拦截就只是第二道机制了，已随 #242 撤掉。
  return <SqlEditorPanel {...props} fullscreen={fullscreen} onFullscreen={setFullscreen} />;
}

/**
 * 全屏那一层的焦点陷阱、初始焦点与收起后的焦点归位，与对话框、运行详情抽屉共用同一份实现
 * （`useDialogFocus`，UX 评审 P0-5 那次立的规矩；本次是 #241 的采纳）。
 *
 * 单独做成一个组件，是因为 `useDialogFocus` 只认**挂载与卸载**——而全屏是同一个
 * `SqlEditor` 里的一个 state。让这层只在全屏时挂载，进全屏就等于入队、退全屏就等于出队，
 * 按键因此天然归最上面那一层管；卸载时它把焦点还给开全屏的那个按钮。
 *
 * 它不渲染任何东西：全屏面板还是原来那个 `div`，class 一换即可，DOM 不重建——重建的话
 * 「开全屏」那个按钮会连人带焦点一起没掉，退出时就没有可归位的目标了。
 */
function FullscreenFocusTrap({
  panel,
  onExit,
}: {
  panel: RefObject<HTMLDivElement | null>;
  onExit: () => void;
}) {
  useDialogFocus(panel, { onEscape: onExit });
  return null;
}

/**
 * 编辑器的样子。全屏与否由外面给，是为了让 `renderToStaticMarkup` 的测试能直接渲染全屏态——
 * 那个状态平时只有点一下全屏按钮才到得了，而这套测试没有 DOM、点不了。
 */
export function SqlEditorPanel({
  value,
  placeholder,
  onChange,
  onFormat,
  fullscreen,
  onFullscreen,
}: SqlEditorProps & {
  fullscreen: boolean;
  onFullscreen: (next: boolean) => void;
}) {
  const panelRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const formatted = useMemo(() => formatSql(value), [value]);
  const [wrapped, setWrapped] = useState(false);
  // **跟着语句长**，上下都有界：十行以下的语句不该配一个半屏高的空框，
  // 而两百行的语句也不该把下面的映射表推到三屏之外——超过上界的用全屏读。
  const visibleRows = useMemo(
    () => Math.min(26, Math.max(10, value.split("\n").length + 2)),
    [value],
  );

  return (
    <div
      className={`source-sql-editor ${fullscreen ? "is-fullscreen" : ""}`}
      ref={panelRef}
      /* 全屏是一层盖住整页的浮层，就得照浮层报：有陷阱撑着，`aria-modal` 才不是空话。 */
      role={fullscreen ? "dialog" : undefined}
      aria-modal={fullscreen ? true : undefined}
      aria-label={fullscreen ? "自定义 SQL 全屏编辑" : undefined}
      /* 里面没有可聚焦元素时的兜底落点，与对话框同形。 */
      tabIndex={fullscreen ? -1 : undefined}
    >
      {fullscreen && (
        <FullscreenFocusTrap panel={panelRef} onExit={() => onFullscreen(false)} />
      )}
      <div className="sql-editor-toolbar">
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
            onClick={() => onFullscreen(!fullscreen)}
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
