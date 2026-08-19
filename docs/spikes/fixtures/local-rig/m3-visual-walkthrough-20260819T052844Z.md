# M3 渲染面走查记录 · 2026-08-19T05:28:44Z

**触发条件**：第 2 条 —— [#118](https://github.com/liumingjian/db-qbs/issues/118) 动了构建器的
渲染面（`TaskFormDialog` 顶部新增一段「数据源」+ 两个 select，`api.ts` 的三个函数改签名）。
**没有**改 `web/src/app.css` 里 `.precheck-reports` 的布局，也**没有**改 `DiagnosticTable`
的列结构——W1/W2/W6 这三条是**回归性复核**，看的是「本票没有把它们碰坏」。

**造态手段与清单原文的偏差（沿用上一份，未变）**：清单写的是复用 `run-m3-acceptance.sh`
的 B1–B6 造态。三份台架仍是退役形态（改造归 [#122](https://github.com/liumingjian/db-qbs/issues/122)），
此刻跑不起来。仍用工作区 `.playwright/` 下的桩后端 + 真实 `web/dist` 构建产物造态
（`m3-mock.py` / `m3-probe.py`，**不进版本库**，与 M2 走查脚本同一处置）。本票给桩后端补了
`GET /api/datasources` 与 `DATASOURCES` 夹具、给 `TASK` 补了两个绑定 id，给探针补了
`datasource_selects` 观察。
**这只回答「渲染出来没有」，一个数据正确性问题都不回答**；B 系列自动验收未跑，见 #118 收口记账。
截图落在 mac 的 `/tmp/m3-visual/`（`w1-w6-1440.png`、`w2-1024.png`、`w3-w4-column-fetch.png`、
`w5-rejected.png`、`builder.png`）；`builder.png` 本轮取回服务器**逐像素看过**，其余取机器观察。

## 逐条观察

**W1（1440 视口，映射预检失败）**：预检表表头仍是**五列** `列 / 源端 / 目标端 / 规则 / 建议`；
6 行逐列一条，`empty_suggestion_cells` 为**空数组**——第五列没有一个空格子。首行实际内容
`PAYLOAD | CLOB | <missing> | 目标表缺列 | 在目标表加列，或把该列从源 SQL 里去掉`，
末尾「总计 6 项问题」。不分组、不折叠、不截断。与 02:56 那次逐字一致，本票没有碰坏它。

**W2（1024 与 1440 各一次）**：两个视口下 `.precheck-reports` 都只有 **1 个 section 且整宽**——
1440 下 section 宽 1168px（容器 1170px），1024 下 752px（容器 754px），左右各 1px 是容器边框。
`.diagnostic-table-wrap` 的 `scrollWidth - clientWidth` 两个视口下都是 **0**，
文档级 `body` 横向溢出也是 **0**：第五列全在框内，不需要横滚。
1024 下靠增高吸收换行（357.20px vs 1440 的 323.11px），没有回退成两栏。
清单里那句「对照 `shape-failed` 仍是两栏」已随 2026-08-19 订正段失去对象，不再核。

**W3（取列 · 三档标记）**：字典类型列渲染为
`NUMBER` / `NUMBER [待配精度]` / `VARCHAR2` / `TIMESTAMP` / `DATE`。
带标记的格子仍是裸 `<td class="mono">` 纯文字：`color: rgb(23, 35, 61)`（正文色）、
`background: rgba(0, 0, 0, 0)`（透明），没有 `--warn` / `--crit`，没有套成标签元素。
`mark_element_tags` 五个全是 `TD`，没有嵌套 span/标签。

**W4（建表 SQL 占位符）**：裸 `NUMBER` 列吐 `DECIMAL(<p>,<s>)`，命中 1 个 `.ddl-placeholder`；
**整份 DDL 照给**——11 行完整，没有因为有占位符就整份失败。

**W5（白名单外的列）**：列清单**照给**——`ROW_ID` / `PAYLOAD` / `BF` / `LOAD_DATE` 四行都在；
`.ddl-output` 区块不存在，位置换成 `.row-size-warning.is-crit`：
「这些列无法生成目标表定义，整份建表 SQL 不给。」+ 逐列原因两行（`PAYLOAD（CLOB）` /
`BF（BINARY_FLOAT）`）+「请按逐列原因修正后重新取列。」列表没有跟着消失。

**W6（值域校核记录混排）**：值域校核那条仍落在同一张五列表的第 6 行：
`N_BARE | NUMBER | DECIMAL(10,2) | 值域校核：3 行超出目标 DECIMAL(10,2) |
调整任务定义和目标 DECIMAL 的 (p,s)，或改源 SQL / CAST 收窄值域`，与其余五条同形，
没有另起区块、没有另起标题，总计行把它一并计入（6 项）。

## 本票新增面（清单之外，逐像素看过 `builder.png`）

新增的「数据源」段**不在 W1–W6 清单里**——清单的六条是 ADR-0032 §8 定的五处新信息位 +
一处布局裁定，本票加的是第七处。按 ADR-0028 §1 只观察不断言，记在这里：

- 段落落在编辑对话框**最顶部**，在「源表」之上，用的是与「源表」「过滤条件」「排序」
  **同一种卡片壳**（同样的圆角边框、同样的 14px 段标题 + 12px 灰副题两行头），
  没有引入新的视觉元素，**`docs/design-system/` 一个字没改**。
- 段标题「数据源」，副题「凭据存在本机、口令加密落盘；目标端凭据随本次运行交给 sink」——
  这是 [ADR-0037](../../../adr/0037-datasource-model-and-credential-boundary.md) §4 那笔
  「口令过线明文」账在界面上唯一的显影，措辞照 §4 的口径写成「交给 sink」而不是「安全传输」。
- 两个 select 半宽并排（标签 `源端（Oracle）` / `目标端（MySQL）`），布局与「源表」段
  `数据库链接（可选）` + `Oracle 表` 那一行同构。探针读到实际选中值
  `ds-oracle` / `ds-mysql`，选项集各为 `["请选择", "源库（走查）"]` /
  `["请选择", "目标库（走查）"]`——桩后端只喂了各一条数据源。
- **没有数据源的增删改入口**：这段只有两个下拉，没有「新建数据源」按钮，也没有管理屏。
  **这是有意的中间态**，管理屏归 [#123](https://github.com/liumingjian/db-qbs/issues/123)，
  本版建数据源只能走 `POST /api/datasources`。
- 顺带复核仍成立的既有形态：源端 SQL 区块只有 `<pre>`、**没有 `textarea`**（ADR-0036 §1）；
  主键勾选框每列一个（5 个）+ 页脚「主键必选：撤掉 DELETE 之后，去重全靠它（ADR-0035 §2）。」；
  条件面 + 排序面共 3 行 `.condition-row`；运行参数只列 `load_date · LOAD_DATE · 日期`，
  写死常量的 `text_floor` 不在里面。

## 没看的东西

- **`m3-visual-walkthrough.md` 清单原文本轮没动**。新增的「数据源」段是否要立成 W7，
  留给 [#122](https://github.com/liumingjian/db-qbs/issues/122)（两份视觉门禁章节的合并本来就归它，
  见 CLAUDE.md「M3 visual gate」段与 #122）。
- **数据源本身的真实连通性一个字都没验**：桩后端只回夹具 JSON，`POST /v1/target/test-connection`
  与 Oracle 侧 `test_connection` 都没有对着真库跑过。归 #122 的台架改造。
- 口令加密落盘的**形态**（`hex(nonce)||hex(ct)`、`datasource.key` 0600）只有 Rust 单测覆盖，
  界面上看不出来，本次走查不回答。
