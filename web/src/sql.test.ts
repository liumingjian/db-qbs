import { describe, expect, it } from "vitest";

import { formatSql, tokenize } from "./sql";

/** 记号序列去掉空白后的字面量。格式化的安全性就是拿它比出来的。 */
function shape(sql: string): string[] {
  return tokenize(sql)
    .filter((token) => token.kind !== "whitespace")
    .map((token) => token.text);
}

describe("tokenize（高亮与格式化共用的词法）", () => {
  it("原文可无损还原——扫描器不许吞掉任何字符", () => {
    const samples = [
      "SELECT * FROM t",
      "select a /* 注释 */, b -- 尾注\nfrom t where s = 'it''s'",
      "SELECT \"混合Case\" FROM APP.T@LINK WHERE x >= 1 AND y != 2",
      "",
      "((((",
      "'没收尾的字符串",
    ];
    for (const sql of samples) {
      expect(tokenize(sql).map((token) => token.text).join("")).toBe(sql);
    }
  });

  it("字符串里的 '' 是转义不是收尾，整段仍是一个 string 记号", () => {
    const tokens = tokenize("WHERE s = 'it''s' AND t = 1");
    const strings = tokens.filter((token) => token.kind === "string");
    expect(strings).toEqual([{ kind: "string", text: "'it''s'" }]);
  });

  it("双引号标识符与单引号字面量分成两类——高亮上不能同色", () => {
    const kinds = tokenize("SELECT \"id\" FROM t WHERE s = 'id'")
      .filter((token) => token.kind === "string" || token.kind === "quoted")
      .map((token) => `${token.kind}:${token.text}`);
    expect(kinds).toEqual(["quoted:\"id\"", "string:'id'"]);
  });

  it("行注释吃到行尾，块注释吃到 */，都不越界", () => {
    const tokens = tokenize("SELECT a -- 说明\nFROM /* 中间 */ t");
    expect(tokens.filter((token) => token.kind === "comment")).toEqual([
      { kind: "comment", text: "-- 说明" },
      { kind: "comment", text: "/* 中间 */" },
    ]);
  });

  it("关键字不分大小写，但记号里存的是原样写法", () => {
    expect(tokenize("select")).toEqual([{ kind: "keyword", text: "select" }]);
  });

  it("没收尾的引号不会把扫描器卡死，剩下的全算这一个记号", () => {
    expect(tokenize("'开着的")).toEqual([{ kind: "string", text: "'开着的" }]);
  });
});

describe("formatSql（只动空白）", () => {
  it("不变式：格式化前后的非空白记号序列逐字相等", () => {
    const samples = [
      "select a,b from t where x=1 and y=2",
      "SELECT NVL(a, b) AS c FROM APP.T@LINK t LEFT JOIN u ON t.id=u.id",
      "select a -- 尾注\n, b from t",
      "select * from (select x from inner_t) q where q.x >= 1",
      "SELECT 'it''s', \"混合Case\" FROM t",
      "select   a   from   t",
      "",
    ];
    for (const sql of samples) {
      expect(shape(formatSql(sql))).toEqual(shape(sql));
    }
  });

  it("子句各起一行，投影逐列换行，AND / OR 缩进挂在 WHERE 下", () => {
    expect(formatSql("select a,b from t where x=1 and y=2")).toBe(
      ["select a,", "  b", "from t", "where x = 1", "  and y = 2"].join("\n"),
    );
  });

  it("LEFT JOIN 是一行，不是 LEFT 一行 JOIN 一行", () => {
    expect(formatSql("select a from t left join u on t.id=u.id")).toBe(
      ["select a", "from t", "left join u on t.id = u.id"].join("\n"),
    );
  });

  it("括号内压成单行——没有分析器，猜不出里面是子查询还是函数参数", () => {
    expect(formatSql("select a from (select x,y from u) q")).toBe(
      ["select a", "from (select x, y from u) q"].join("\n"),
    );
  });

  it("`.` 与 `@` 两侧不留空格：插一个空格就不是同一个名字了", () => {
    expect(formatSql("select t.a from app.t_customer@poc_link_a t")).toBe(
      ["select t.a", "from app.t_customer@poc_link_a t"].join("\n"),
    );
  });

  it("`>=` / `!=` 不被拆开，也不被改写成 ≥ / ≠", () => {
    expect(formatSql("select a from t where x>=1 and y!=2")).toBe(
      ["select a", "from t", "where x >= 1", "  and y != 2"].join("\n"),
    );
  });

  it("函数调用不留空格，`IN (…)` 留——左括号前站的是标识符还是关键字", () => {
    expect(formatSql("select nvl(a,0) from t where x in (1,2)")).toBe(
      ["select nvl(a, 0)", "from t", "where x in (1, 2)"].join("\n"),
    );
  });

  it("行注释后面的东西一定另起一行，不会被它注释掉", () => {
    const formatted = formatSql("select a -- 尾注\n, b from t");
    expect(formatted.split("\n")[0]).toBe("select a -- 尾注");
    expect(shape(formatted)).toEqual(shape("select a -- 尾注\n, b from t"));
  });

  it("反复格式化是幂等的", () => {
    const once = formatSql("select a,b from t where x=1 and y=2");
    expect(formatSql(once)).toBe(once);
  });
});
