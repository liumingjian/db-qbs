# M3 渲染面走查记录 · 2026-08-19T02:56:34Z

**触发条件**：第 2 条 —— [#121](https://github.com/liumingjian/db-qbs/issues/121) 大改前端，
且直接改了 `web/src/app.css` 里 `.precheck-reports` 的构成与 `DiagnosticTable` 的调用面。

**造态手段与清单原文的偏差（必须先读这一段）**：清单写的是「复用 `run-m3-acceptance.sh`
的 B1–B6 造态」。本次**没有用台架**——#121 换了写入模型（ADR-0035）与任务定义形态（ADR-0036），
三份台架的报文与判据都还是退役形态，改造归 [#122](https://github.com/liumingjian/db-qbs/issues/122)，
此刻跑不起来。改用一个桩后端 + 真实 `web/dist` 构建产物造出这六种态，取机器观察（只观察不断言，
照 ADR-0028 §1）。走查工具放在工作区的 `.playwright/`（`m3-mock.py` / `m3-probe.py`），
**不进版本库**——与 M2 那几支走查脚本同一处置。
**这只回答「渲染出来没有」，一个数据正确性问题都不回答**；B 系列自动验收未跑，见收口记账。
截图落在 mac 的 `/tmp/m3-visual/`（`w1-w6-1440.png`、`w2-1024.png`、`w3-w4-column-fetch.png`、
`w5-rejected.png`、`builder.png`）。

## 结构性变更：预检报告从两栏变成一栏

原来 `.precheck-reports` 是并排两段：source 本地的 SQL 形状预检 + sink 的映射预检。
形状预检整段随 ADR-0036 §5 取消，**W2 里那句「对照：`shape-failed` 态仍是两栏并置」
连带失去对象**——`shape-failed` 这个态在 v1 已经不存在。CSS 侧只删掉了
`.is-skipped` 那一支用到的占位分栏，`.precheck-reports.is-map-failed > section
{ grid-column: 1 / -1 }` 一个字没动，映射失败态本来走的就是整宽这一支。

## 逐条观察

**W1（1440 视口，映射预检失败）**：预检表表头是**五列** `列 / 源端 / 目标端 / 规则 / 建议`；
6 行逐列一条，`empty_suggestion_cells` 为空数组——第五列**没有一个空格子**，
值全部来自桩后端填的 `suggestion`（web 侧一行判定都没重算）。首行实际内容：
`PAYLOAD | CLOB | <missing> | 目标表缺列 | 在目标表加列，或把该列从源 SQL 里去掉`。
末尾一行「总计 6 项问题」，不分组、不折叠、不截断。

**W2（1024 与 1440 各一次）**：两个视口下 `.precheck-reports` 都只有 **1 个 section
且整宽堆叠**——1440 下 section 宽 1168px（容器 1170px），1024 下宽 752px（容器 754px），
左右各 1px 是容器边框，没有第二栏、没有残留空栏。
`.diagnostic-table-wrap` 的 `scrollWidth - clientWidth` 两个视口下都是 **0**，
文档级横向溢出也是 **0**——第五列全在框内，不需要横滚。
1024 下表格靠增高（357px vs 1440 的 323px）吸收换行，没有回退成两栏。

**W3（取列 · 三档标记）**：describe 类型那一列实际渲染为
`NUMBER` / `NUMBER [待配精度]` / `VARCHAR2` / `TIMESTAMP` / `DATE`。
带标记的格子仍是**裸 `<td class="mono">` 纯文字**：`color: rgb(23, 35, 61)`（正文色）、
`background: rgba(0, 0, 0, 0)`（透明），没有 `--warn` / `--crit`，没有套成标签元素。

**W4（建表 SQL 占位符）**：裸 `NUMBER` 列吐出 `DECIMAL(<p>,<s>)`，命中 1 个
`.ddl-placeholder`；**整份 DDL 照给**——11 行、以 `) DEFAULT CHARSET=utf8mb4;` 收尾，
没有因为有占位符就整份失败。顺带记一笔本票带来的形态变化：DDL 里主键列是
`` `ROW_ID` DECIMAL(8,0) NOT NULL `` + `` PRIMARY KEY (`ROW_ID`) ``，
不再是 `KEY idx_<日期列>`（ADR-0035 §2 增补三）。

**W5（白名单外的列）**：列清单**照给**——`ROW_ID` / `PAYLOAD` / `BF` / `LOAD_DATE` 四行都在；
`.ddl-output` 区块**不存在**，位置换成 `.row-size-warning.is-crit`：
「这些列无法生成目标表定义，整份建表 SQL 不给。」+ 逐列原因两行 + 「请按逐列原因修正后重新取列。」
列表没有跟着消失。

**W6（值域校核记录混排）**：值域校核那条实际落在同一张五列表的第 6 行：
`N_BARE | NUMBER | DECIMAL(10,2) | 值域校核：3 行超出目标 DECIMAL(10,2) |
调整任务定义和目标 DECIMAL 的 (p,s)，或改源 SQL / CAST 收窄值域`，
与其余五条逐列规则**同形**，没有另起区块、没有另起标题，总计行把它一并计入（6 项）。

## 清单之外顺手记的（本票新增面，不属 W1–W6）

- 构建器里条件面 + 排序面共 3 行 `.condition-row`（2 条条件 + 1 条排序），
  主键勾选框每列一个（5 个），页脚提示「主键必选：撤掉 DELETE 之后，去重全靠它（ADR-0035 §2）。」
- 源端 SQL 区块**没有 `textarea`**，只有 `<pre>`：v1 没有手改入口（ADR-0036 §1）。
  实际渲染的 SQL：
  ```
  SELECT a.ROW_ID AS ROW_ID,
         a.N_BARE AS N_BARE,
         a.V_TEXT AS V_TEXT,
         a.PAYLOAD AS PAYLOAD,
         a.LOAD_DATE AS LOAD_DATE
    FROM SPIKE.T_M3_B1 a
   WHERE a.LOAD_DATE = TO_DATE(:load_date,'YYYY-MM-DD')
     AND a.V_TEXT > :text_floor
   ORDER BY a.ROW_ID DESC
  ```
- 运行参数清单只列「运行时填」的那条：`load_date · LOAD_DATE · 日期`；
  写死常量的 `text_floor` 不在里面（它每次都一样，进运行参数集不增加区分度）。
- 发起面按参数逐条取值（`date` 类型渲染成日期输入框），不再是一个写死的「业务日期」框。
