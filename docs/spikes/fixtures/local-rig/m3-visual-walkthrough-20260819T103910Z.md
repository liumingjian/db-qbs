# M3 渲染面走查记录 · 2026-08-19T10:39:10Z

**触发条件**：第 2 条 —— [#132](https://github.com/liumingjian/db-qbs/issues/132) 直接改了
`web/src/app.css` 里 `.precheck-reports` 的布局（两栏 grid → 单段），并动了
`web/src/RunScreen.tsx` 上那个容器的类名。触发源是 ADR-0036 §5 取消 SQL 形状预检
（[ADR-0040](../../../adr/0040-v1-acceptance-criteria-and-rig-fourth-entry.md) §6.2），
不是 #123。`DiagnosticTable` 的列结构一个字没动。

**造态手段与清单原文的偏差（必须先读这一段）**：清单写的是「复用 `run-m3-acceptance.sh`
的 B1–B6 造态」。本次**仍然没有用台架** —— M1/M2/M3 三份现在就是红的（调用面对不上
ADR-0035/0036/0038 之后的报文），改造归 [#134](https://github.com/liumingjian/db-qbs/issues/134)，
此刻跑不起来。沿用 2026-08-19T02:56:34Z 那次的处置：桩后端 + 真实 `web/dist` 构建产物造态，
取机器观察（只观察不断言，照 ADR-0028 §1）。工具在工作区的 `.playwright/`
（`m3-mock.py` / `m3-probe.py` / 本次新增的 `run-w-walkthrough.sh`），**不进版本库**。
桩里的 `TaskSpec.columns` 本次顺手改成了 `ColumnMapping` 形状（①/#127 之后的形态），
否则构建器那三条（W3/W4/W5）根本渲染不出来。
**这只回答「渲染出来没有」，一个数据正确性问题都不回答**；B 系列自动验收未跑。

跑在 mac 上（`rexec`），构建产物 `web/dist/assets/index-CSpYL99t.css`（含本票改动）。
截图落在 mac 的 `/tmp/m3-visual/`（`w1-w6-1440.png`、`w2-1024.png`、
`w3-w4-column-fetch.png`、`w5-rejected.png`、`builder.png`），W1 与 W2 两张已缩图取回**人眼看过**。

## 先记一处与 #126 正文不符的事实：**「半屏空洞」没有真的渲染出来**

#126 与 #132 的正文都写「`.precheck-reports` 仍是两栏 grid，而 `RunScreen.tsx` 只渲染一段
→ **半屏空洞**」。**实测不成立**：`PrecheckReports` 渲染出来的容器一直带着 `is-map-failed`，
而 `.precheck-reports.is-map-failed > section { grid-column: 1 / -1 }` 早就把那一段拉成整宽了，
两栏的第二栏从来没有机会露面（`background: var(--line)` 只在 1px 的 `gap` 处见光，
单个跨全栏的子项不产生 gap）。2026-08-19T02:56:34Z 那份记录其实已经写着同一件事
（「映射失败态本来走的就是整宽这一支」），只是没往前推一步。

所以本票**改掉的是一套已经没有对象的两栏机关**（grid 容器 + `is-map-failed` 跨栏修饰 +
`.is-passed` / `.is-skipped` 两个形状预检才有的态修饰 + 两条响应式分栏覆盖），
**不是修一个看得见的空洞**。判据也因此变得干脆：**改动前后几何逐数相同**，见 W2。

## 逐条观察

**W1（1440 视口，映射预检失败）**：预检表表头是**五列** `列 / 源端 / 目标端 / 规则 / 建议`；
6 行逐列一条，`empty_suggestion_cells` 为空数组 —— 第五列**没有一个空格子**，值全部来自桩后端
填的 `suggestion`（web 侧一行判定都没重算）。首行实际内容：
`PAYLOAD | CLOB | <missing> | 目标表缺列 | 在目标表加列，或把该列从源 SQL 里去掉`。
末尾一行「总计 6 项问题」，不分组、不折叠、不截断。截图上「映射预检」卡片顶边是那条
3px 的 `--crit` 红线（`.is-failed` 未受本票影响），右上角 `sink` 小字仍在。

**W2（1024 与 1440 各一次）**：两个视口下 `.precheck-reports` 都只有 **1 个 section 且整宽**——

| 视口 | section 宽 | 容器宽 | section 高 | 表格 `scrollWidth-clientWidth` | 文档级横向溢出 |
|---|---|---|---|---|---|
| 1440 | 1168px | 1170px | 323.109px | **0** | **0** |
| 1024 | 752px | 754px | 357.203px | **0** | **0** |

左右各 1px 是容器边框，没有第二栏、没有残留空栏；第五列「建议」全在框内，不需要横滚。
1024 下表格靠增高（357px vs 1440 的 323px）吸收换行，没有回退成两栏——
截图上 `N_TOO_WIDE` 与 `N_BARE` 两行的「目标端 / 规则 / 建议」确实是折行显示的。

**这四个数与改动前逐数相同**（2026-08-19T02:56:34Z 的记录：1440 下 1168/1170、高 323；
1024 下 752/754、高 357）。这正是本票该有的结果：撤掉的是死机关，渲染面一像素没动。

**W3（取列 · 三档标记）**：describe 类型那一列实际渲染为
`NUMBER` / `NUMBER [待配精度]` / `VARCHAR2` / `TIMESTAMP` / `DATE`。带标记的格子仍是
**裸 `<td class="mono">` 纯文字**：`color: rgb(23, 35, 61)`（正文色）、
`background: rgba(0, 0, 0, 0)`（透明），没有 `--warn` / `--crit`，没有套成标签元素。
五个格子的 `tagName` 全是 `TD`。

**W4（建表 SQL 占位符）**：裸 `NUMBER` 列吐出 `DECIMAL(<p>,<s>)`，命中 1 个 `.ddl-placeholder`；
**整份 DDL 照给** —— 11 行、以 `) DEFAULT CHARSET=utf8mb4;` 收尾，没有因为有占位符就整份失败。

**W5（白名单外的列）**：列清单**照给** —— `ROW_ID` / `PAYLOAD` / `BF` / `LOAD_DATE` 四行都在；
`.ddl-output` 区块**不存在**，位置换成 `.row-size-warning.is-crit`：
「这些列无法生成目标表定义，整份建表 SQL 不给。」+ 逐列原因两行
（`PAYLOAD（CLOB）` / `BF（BINARY_FLOAT）`）+ 「请按逐列原因修正后重新取列。」列表没有跟着消失。

**W6（值域校核记录混排）**：值域校核那条实际落在同一张五列表的第 6 行：
`N_BARE | NUMBER | DECIMAL(10,2) | 值域校核：3 行超出目标 DECIMAL(10,2) |
调整任务定义和目标 DECIMAL 的 (p,s)，或改源 SQL / CAST 收窄值域`，
与其余五条逐列规则**同形**，没有另起区块、没有另起标题，总计行把它一并计入（6 项）。

## 清单之外顺手记的（不属 W1–W6，也不属本票）

- **1024 视口下 `.run-identity` 末尾有一个空格子**：五条身份字段摆进四列 grid，第五条换行后
  右侧空着一格底色。那是 `.run-identity` 不是 `.precheck-reports`，**本票不碰**——
  记在这里免得下一次走查以为是本票带出来的。
- 构建器面（旁证，与 02:56:34Z 那次一致）：条件面 + 排序面共 3 行 `.condition-row`，
  主键勾选框 5 个，源端 SQL 区块只有 `<pre>` 没有 `textarea`，运行参数清单只列 `load_date`。
  投影现在走 `ColumnMapping` 的 `a.X AS X` 恒等形态，与 ① 合并后的 SQL 逐字一致。
