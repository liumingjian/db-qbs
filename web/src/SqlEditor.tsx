import { useMemo, useRef } from "react";
import { WrapText } from "lucide-react";

import { formatSql, tokenize } from "./sql";

/**
 * 自定义 SQL 的输入框：高亮 + 一键格式化（ADR-0046 §1）。
 *
 * 高亮走**透明字 textarea 压在着色 `<pre>` 上**这条老路，不引第三方编辑器：
 * 这一格要的是「看清结构」，不是补全、折叠、多光标。换成 CodeMirror 要多背 300KB，
 * 还要在它的样式系统和本仓的令牌之间再对一次账。
 *
 * 两层必须逐像素同框，否则光标会飘。为此 textarea 关掉了软换行（`wrap="off"`）：
 * 开着的话两层要各自跑一遍换行算法，而 textarea 一旦出竖向滚动条，它的内容宽度会比
 * `<pre>` 少一条滚动条——同样的文本被排成不同的行，光标就此错位。关掉之后宽度不参与排版，
 * 这类错位在根上不成立，代价是长行要横向滚动（正好由「格式化」这个按钮兜住）。
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
  const highlightRef = useRef<HTMLPreElement>(null);
  const tokens = useMemo(() => tokenize(value), [value]);
  const formatted = useMemo(() => formatSql(value), [value]);

  return (
    <div className="source-sql-editor">
      <div className="sql-editor-toolbar">
        {/* 卡片标题已经写着「自定义 SQL」，这里不再挂一个同名标签。 */}
        <small className="spec-note">
          读取列后可以只勾要搬的列。实际执行时会在这条 SQL 外层套一层投影，
          只取勾选的列并改名成目标字段——没勾的列不会过线。
        </small>
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
          <WrapText size={15} />
          格式化
        </button>
      </div>
      <div className="sql-input-shell">
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
          required
          ref={textareaRef}
          rows={8}
          wrap="off"
          spellCheck={false}
          aria-label="自定义 SQL"
          value={value}
          placeholder={placeholder}
          onScroll={() => {
            const textarea = textareaRef.current;
            const highlight = highlightRef.current;
            if (textarea === null || highlight === null) {
              return;
            }
            highlight.scrollTop = textarea.scrollTop;
            highlight.scrollLeft = textarea.scrollLeft;
          }}
          onChange={(event) => onChange(event.target.value)}
        />
      </div>
    </div>
  );
}
