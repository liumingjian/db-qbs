// 自定义 SQL 的词法：高亮与格式化共用这一份（ADR-0046 §1）。
//
// 两个功能各写一个扫描器，迟早在字符串字面量、注释、引号标识符这三处漂开——
// 而这三处恰好是「看错一个字符就改错语义」的地方。所以只有一个 `tokenize`，
// 高亮拿它上色，格式化拿它排版，谁也不许自己再认一遍字符。

/**
 * 记号类别。分档只按**高亮要不要区别对待**来定，不追求语法完整：
 * 这里没有分析器，也不打算有——真相源是结构化规格，SQL 只是被包裹的子查询（ADR-0045 §1）。
 */
export type SqlTokenKind =
  | "whitespace"
  | "comment"
  | "string"
  | "quoted"
  | "number"
  | "keyword"
  | "word"
  | "punct";

export interface SqlToken {
  kind: SqlTokenKind;
  text: string;
}

/**
 * 关键字表。**只收会影响高亮或换行的词**，不是 Oracle 保留字全集——
 * 收全了既维护不动，也不会让高亮更准（漏一个词的代价是它显示成普通标识符，仅此而已）。
 */
const KEYWORDS: ReadonlySet<string> = new Set([
  "SELECT", "FROM", "WHERE", "GROUP", "BY", "HAVING", "ORDER", "UNION", "ALL",
  "INTERSECT", "MINUS", "DISTINCT", "AS", "ON", "AND", "OR", "NOT", "IN",
  "EXISTS", "BETWEEN", "LIKE", "IS", "NULL", "CASE", "WHEN", "THEN", "ELSE",
  "END", "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "OUTER", "CROSS", "WITH",
  "CONNECT", "START", "PARTITION", "OVER", "ASC", "DESC", "FETCH", "OFFSET",
  "ROWS", "ONLY", "NULLS", "FIRST", "LAST",
]);

// 标识符的首字符与后续字符。`$` 与 `#` 是 Oracle 允许的（`SYS_$` 这类名字真的存在）；
// 非 ASCII 一律并进标识符——中文列名在现场库里有，把它切碎只会让高亮更难看。
const WORD_START = /[A-Za-z_-￿]/;
const WORD_PART = /[A-Za-z0-9_$#-￿]/;

/**
 * 扫成记号序列。**原文可无损还原**：`tokenize(s).map(t => t.text).join("") === s` 恒成立，
 * 格式化的安全性整个压在这条上（见 `formatSql`）。
 *
 * 认不出来的字符一律落 `punct` 且**一个字符一个记号**，不做贪心合并——
 * `>=` 和 `!=` 因此是两个 punct，这不影响高亮，也不影响格式化（两者之间不插空格，见下）。
 * 认不出来也绝不吞掉，扫描器不许在任何输入上停住。
 */
export function tokenize(sql: string): SqlToken[] {
  const tokens: SqlToken[] = [];
  let index = 0;
  while (index < sql.length) {
    const ch = sql[index]!;
    if (/\s/.test(ch)) {
      const start = index;
      while (index < sql.length && /\s/.test(sql[index]!)) {
        index += 1;
      }
      tokens.push({ kind: "whitespace", text: sql.slice(start, index) });
      continue;
    }
    if (ch === "-" && sql[index + 1] === "-") {
      const end = sql.indexOf("\n", index);
      const stop = end === -1 ? sql.length : end;
      tokens.push({ kind: "comment", text: sql.slice(index, stop) });
      index = stop;
      continue;
    }
    if (ch === "/" && sql[index + 1] === "*") {
      const end = sql.indexOf("*/", index + 2);
      const stop = end === -1 ? sql.length : end + 2;
      tokens.push({ kind: "comment", text: sql.slice(index, stop) });
      index = stop;
      continue;
    }
    if (ch === "'" || ch === '"') {
      // 引号内的 `''` / `""` 是转义而不是收尾，认错一个就把后面整段的着色反了过来。
      const quote = ch;
      let cursor = index + 1;
      while (cursor < sql.length) {
        if (sql[cursor] === quote) {
          if (sql[cursor + 1] === quote) {
            cursor += 2;
            continue;
          }
          cursor += 1;
          break;
        }
        cursor += 1;
      }
      tokens.push({
        kind: quote === "'" ? "string" : "quoted",
        text: sql.slice(index, cursor),
      });
      index = cursor;
      continue;
    }
    if (/[0-9]/.test(ch)) {
      const start = index;
      while (index < sql.length && /[0-9.]/.test(sql[index]!)) {
        index += 1;
      }
      tokens.push({ kind: "number", text: sql.slice(start, index) });
      continue;
    }
    if (WORD_START.test(ch)) {
      const start = index;
      index += 1;
      while (index < sql.length && WORD_PART.test(sql[index]!)) {
        index += 1;
      }
      const text = sql.slice(start, index);
      tokens.push({
        kind: KEYWORDS.has(text.toUpperCase()) ? "keyword" : "word",
        text,
      });
      continue;
    }
    tokens.push({ kind: "punct", text: ch });
    index += 1;
  }
  return tokens;
}

/** 一行的开头（`SELECT` / `FROM` / …）。`BY` 不在内：它跟在 `GROUP` / `ORDER` 后面同行。 */
const CLAUSE_STARTERS: ReadonlySet<string> = new Set([
  "SELECT", "FROM", "WHERE", "GROUP", "HAVING", "ORDER", "UNION", "INTERSECT",
  "MINUS", "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "CROSS", "CONNECT",
  "START", "FETCH", "OFFSET",
]);

/** `LEFT JOIN` 是一行不是两行——`JOIN` 跟在这些词后面时不另起。 */
const JOIN_PREFIXES: ReadonlySet<string> = new Set([
  "INNER", "LEFT", "RIGHT", "FULL", "CROSS", "OUTER",
]);

/** 这两个符号两侧不留空格：`A.B`、`T@LINK` 中间插一个空格就不是同一个名字了。 */
const TIGHT_PUNCT: ReadonlySet<string> = new Set([".", "@"]);

function needsSpace(previous: SqlToken, next: SqlToken): boolean {
  if (previous.kind === "punct") {
    if (previous.text === "(" || TIGHT_PUNCT.has(previous.text)) {
      return false;
    }
    // `>=` / `!=` / `||` 都是被拆成两个 punct 扫进来的，中间不能插空格。
    if (next.kind === "punct" && next.text !== "(") {
      return false;
    }
  }
  if (next.kind === "punct") {
    // `NVL(a, b)` 是函数调用，`IN (…)` / `EXISTS (…)` 是关键字带一个括号组：
    // 前者不留空格，后者留。区别只在于左括号前面站的是标识符还是关键字。
    if (next.text === "(") {
      return previous.kind !== "word" && previous.kind !== "quoted";
    }
    return !(next.text === ")" || next.text === "," || TIGHT_PUNCT.has(next.text));
  }
  return true;
}

const INDENT = "  ";

/**
 * 格式化。**只动空白，一个字符都不改**——
 * `tokenize(formatSql(s))` 去掉空白记号后与 `tokenize(s)` 去掉空白记号后逐字相等，
 * 这条不变式由测试守着（`sql.test.ts`）。
 *
 * 因此这里**不把关键字改成大写**。看着像是格式化器的本分，但在 Oracle 上改大小写
 * 对引号标识符是致命的，而「哪些词是引号标识符外的普通词」需要一个这里没有的分析器；
 * 更要紧的是，一旦允许改字符，「格式化没改坏我的 SQL」就从可证明降成了要人肉核对（ADR-0046 §1）。
 *
 * 括号内一律压成单行：子查询与函数参数不缩进。这是在「排版好看」与「排版可预测」之间
 * 选了后者——没有分析器，猜不出括号里那段是子查询还是 `NVL(a, b)`，猜错就排出一坨。
 */
export function formatSql(sql: string): string {
  const tokens = tokenize(sql).filter((token) => token.kind !== "whitespace");
  const lines: string[] = [];
  let current = "";
  let depth = 0;
  let previous: SqlToken | null = null;
  let previousWord = "";

  const flush = () => {
    if (current.trim() !== "") {
      lines.push(current);
    }
    current = "";
    previous = null;
  };

  for (const token of tokens) {
    const upper = token.kind === "keyword" ? token.text.toUpperCase() : "";
    if (depth === 0 && upper !== "") {
      const isJoinTail = upper === "JOIN" && JOIN_PREFIXES.has(previousWord);
      if (CLAUSE_STARTERS.has(upper) && !isJoinTail) {
        flush();
      } else if (upper === "AND" || upper === "OR") {
        flush();
        current = INDENT;
      }
    }
    if (previous !== null && needsSpace(previous, token)) {
      current += " ";
    }
    current += token.text;
    previous = token;
    if (upper !== "") {
      previousWord = upper;
    } else if (token.kind !== "comment") {
      previousWord = "";
    }
    // 行注释吃到行尾，后面接任何东西都会被它注释掉——它必须是本行最后一个记号。
    if (token.kind === "comment" && token.text.startsWith("--")) {
      flush();
      continue;
    }
    if (token.kind === "punct") {
      if (token.text === "(") {
        depth += 1;
      } else if (token.text === ")") {
        depth = Math.max(0, depth - 1);
      } else if (token.text === "," && depth === 0) {
        flush();
        current = INDENT;
      }
    }
  }
  flush();
  return lines.join("\n");
}
