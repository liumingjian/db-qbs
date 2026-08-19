# M2 渲染面走查记录 —— 2026-08-19T11:22Z（整份 V1–V25，第一版跑一次即封）

**清单来源**：[`m2-visual-walkthrough.md`](m2-visual-walkthrough.md)（规格挂 ADR-0028 §6）。
**触发条件**：命中第 2 条——本轮改了 `docs/design-system/README.md`（§5 末段的判废句、§7 预留位置
补数据源屏、§7 SQL 构建器那条的「两段预检」订正）。`docs/design-system/tokens.css` **一个字节没动**。
**落点依据**：[ADR-0039](../../../adr/0039-v1-ui-increments.md) §9 裁定「认下整份 V1–V25、不找豁免」，
[ADR-0040](../../../adr/0040-v1-acceptance-criteria-and-rig-extension.md) §6.1 定了跑在
[#133](https://github.com/liumingjian/db-qbs/issues/133)、**跑完即封**。
**被走查的改动**：④⑤⑥ 三张界面票（[#130](https://github.com/liumingjian/db-qbs/issues/130) /
[#131](https://github.com/liumingjian/db-qbs/issues/131) / [#132](https://github.com/liumingjian/db-qbs/issues/132)）
合并后的 `main` @ `e63c492` + 本票的 README 三处文字订正（工作区）。
**配套自动验收**：**没有**。M1/M2/M3 三份台架此刻仍是退役调用面（改造归 #134），第四入口
`run-v1-acceptance.sh` 归 #135 还不存在。见下节「造态手段与偏差」。

## 造态手段与偏差（先说清楚，免得后面每条都要重申一遍）

历轮 V 系列靠 `M2_KEEP_RIG=1 run-m2-acceptance.sh` 留下的台架实例造态。**这一轮做不到**：
那三份脚本的报文与判据都还是 ADR-0035/0036/0038 之前的形状，起不来。

因此本轮沿用 #130/#131/#132 三票同一套姿势：**桩后端（`.playwright/v-mock.py`）+ mac 上
`npm run build` 出来的真实 `web/dist`**，用 Playwright 取渲染文本、计算样式、几何位置与真实像素亮度
（`.playwright/v-probe.py`，两支都在未跟踪的 `.playwright/` 下，不进版本库）。

- **偏差**：喂给界面的运行记录是桩造的，不是真跑出来的。**只观察渲染，不断言数据正确性**。
- **没有违反 ADR-0028 §1**：一行 DOM 断言都没进验收套件或任何测试文件。
- 造态开关是发起对话框里的业务日期：`2026-01-01`~`2026-01-08` 一个日期对应一个 run 态。
- 视口 1440×1000、`devicePixelRatio = 1`。

## 结论速览

| 判定 | 条目 | 数 |
|---|---|---|
| **符合** | V1 V2 V3 V4 V5 V7 V8 V9 V11(第二张卡) V13 V14 V15 V16 V17 V19 V20 V22 V23 V24 V25 | 19 又半 |
| **N/A（判据已随 ADR 退役，编号保留）** | V6 V10 V11(第一张卡) V12 V18 V21 | 5 又半 |
| **不符合** | 无 | 0 |

**本轮的主要产出不是 19 个「符合」，是那 5 条半的 N/A**：V 系列是 M2 设计系统的定盘星，
而第一版的三条 ADR（0036 §5 取消形状预检、0036 §2 SQL 改由规格现算、0038 §3 开出目标端元数据面）
把其中六条判据的**对象**拿掉或反了向。裁定与理由见 [ADR-0040 增补（2026-08-19）](../../../adr/0040-v1-acceptance-criteria-and-rig-extension.md)，
清单文件 `m2-visual-walkthrough.md` 已就地标注，**编号一个都不重编**（同 M2 的 A3/A6 先例）。

另有一条量测口径的教训记在第八节。

## 一、三条形状轴不得互换

### V1 · 运行详情 · 进行中
- **实际观察**：`load_date=2026-01-01` 造出停在 STREAMING 的 run（`rec-live`）。`.phase-item`
  **恰好 3 个**：`PREPARING 准备中`（`is-done`，圆点 `rgb(25,190,107)`）、`STREAMING 传输中`
  （`is-current`，圆点 `rgb(44,138,240)`）、`COMMITTING 提交中`（未到，`rgb(128,134,149)`），
  三点均 `8px × 8px`、`border-radius: 50%`。串尾 `→ 终态待定` 是纯文字，
  `.phase-after` 下 `.phase-dot` 计数 **0**。同屏 `.terminal-block` **0**、`.error-code` **0**。
  结论条「运行中 STREAMING」，四个数 `已推行数 3 / 当前批次序号 1 / 已用时 00:00 / 累计字节 96 B`。
  截图 `v1-live.png` 已缩图取回**人眼看过**：三点一线、终态处是文字不是第四个点。**符合。**

### V2 · 运行详情 · 成功
- **实际观察**：`load_date=2026-01-03`。终态渲染 `SWAPPED　目标表已切换`，计算样式
  `background-color: rgb(205,235,217)`（`--ok-fill`）、`color: rgb(11,102,55)`（`--ok-ink`）、
  `border: 1px solid rgb(196,236,214)`——**是块且实心有底色**。同屏 `.error-code` **0**、`.phase-item` **0**。
  结论条 `目标端：运行成功：已推送 100,000 行，暂存表已切换为目标表。`；四个数
  `100,000 / 20 / 00:09 / 4481.8 KiB`。**符合。**

### V3 · 运行详情 · 校验失败
- **实际观察**：`load_date=2026-01-04`（`VERIFY_FAILED`）。终态 `DISCARDED　目标表未被触碰`，
  `background-color: rgba(0,0,0,0)`（**完全无底色**）、`border: 1px solid rgb(220,222,226)`、
  `color: rgb(81,90,110)`——**描边块**。与 V2 的实心块并列时一个实心一个描边。
  运行历史同屏里两个 `DISCARDED`（`VERIFY_FAILED` 与 `INTERNAL_PRECHECK_ESCAPE`）逐数相同。**符合。**

### V4 · 运行详情 · 映射预检失败（含 5xx 一例）
- **实际观察**：`load_date=2026-01-05` 屏，`.error-summary` 子元素实测顺序与几何位置为
  `[0] .error-code.is-rejected「PRECHECK_FAILED HTTP 422」x=229` →
  `[1] 纯 span「[类型映射] 目标端：映射预检未通过：一次发现 3 项问题，未创建暂存表」x=419`,
  **码在前、中文人话结论在其右**。分档两例（实测计算样式）：
  - `PRECHECK_FAILED 422` / `VERIFY_FAILED 409` → `border-style: dashed`、
    边 `rgb(251,210,199)` / 底 `rgb(254,243,240)` / 字 `rgb(237,64,20)`（`--crit` 系）；
  - `INTERNAL_PRECHECK_ESCAPE 500` → `border-style: solid`、
    边 `rgb(255,225,184)` / 底 `rgb(255,248,236)` / 字 `rgb(255,153,0)`（`--warn` 系）。
  **4xx 虚边、5xx 实边，符合。**

### V5 · 灰度打印 / 关掉颜色 —— 可量判据，实测数字如下
- **实际观察**：运行历史列表整页 `filter: grayscale(1)` 后**整页截图**，取块左内边距内
  6×14 像素片的中位亮度：**`SWAPPED` 块 227（min = max = 227，窗口完全落在填充上）、
  `DISCARDED` 块 255，差 28/255 = 11.0%**（判据 ≥ 25/255）。同列纯文字格
  （`未建暂存表`）中位 232。**符合。**
- **与 2026-08-16 那轮逐数相同**（227 / 255 / 28 / 11.0%）——`--ok-fill` 与 `--ok-ink` 自 #89 之后没动过，
  这一轮是第一版界面全部落地之后的复核。
- **量测口径踩了一次坑，值得照抄**：第一次量出来是 **21/255 = 8.2%**（`DISCARDED` 块中位 248）。
  原因不是设计系统退化，是**鼠标指针停在那一行上**——`.data-grid tbody tr:hover td` 会把行底
  从纸白换成 `--mute-bg`（248,248,249），而 `DISCARDED` 块 `background` 是透明的，量到的正是透出来的行底。
  探针加一句 `page.mouse.move(0, 0)` 之后回到 255。**判据本身没问题，是观察手段把悬停态当成了常态。**

## 二、轴二只在墓碑真存在时出现

### V6 · 运行详情 · 形状预检失败 —— **N/A，判据已退役**
- **对象不存在**：[ADR-0036](../../../adr/0036-task-spec-structured.md) §5 整段取消了 SQL 形状预检，
  `web/src/run.test.ts:131` 有一条「no longer has a shape-precheck branch at all」守着它不回来。
  这一屏造不出来，**不是「没跑」，是没有可跑的对象**。
- **它想守的东西仍然由 V7 / V8 / V15 守着**：不出终态块的三个态里，剩下的两个（映射预检失败、
  结局不明）都实测过，`run_id` 未发起那一格由 V15 实测。**照实记 N/A，不记「通过」。**

### V7 · 运行详情 · 映射预检失败
- **实际观察**：`load_date=2026-01-05` 屏 `.terminal-block` 计数 **0**（`.error-code` 计数 1、
  `.phase-item` 计数 0）。结论条里没有终态块，「未创建暂存表」是人话不是块。**符合。**

### V8 · 运行详情 · 进程消失
- **实际观察**：`load_date=2026-01-06`（`unknown_reason = PROCESS_DISAPPEARED`）。整屏
  `.terminal-block` **0**、`.error-code` **0**、`.phase-item` **0**；`.unknown-conclusion.is-process_disappeared`
  三行文字 `结局不明 | 进程消失，无终态日志 | 没有错误码，也没有目标端终态块。`，无底色无边框。**符合。**

### V9 · 运行历史列表
- **实际观察**：「结局」列 7 行**四种形态混排、宽高参差**——
  - 块：`SWAPPED 目标表已切换` w=149.4 h=26.6、`DISCARDED 目标表未被触碰` w=175.8 h=26.6（两行）；
  - 中性灰字 `.neutral-outcome`：`未建暂存表` w=60 h=17、`未发起` w=36 h=17，均 `rgb(128,134,149)`、透明底；
  - `.unknown-summary.is-process_disappeared`：`结局不明 / 进程消失，无终态日志` w=175.8 **h=36.6**（比别人都高）；
  - `.live-summary`：`进行中 STREAMING` w=122.7 h=18.6。
  **四种形态、五种宽度、四种高度，齐不了。符合。**
  截图 `v9-v14-history.png` 已缩图取回**人眼看过**：绿实心块、描边块、两种灰字、双行结局不明、
  带蓝点的进行中，一列里六种长相。

## 三、两段预检分开是结构不是文案

### V10 · 运行详情 · 形状预检失败 · 上下两张卡 —— **N/A，判据已退役**
- **对象不存在**：同 V6。第二张「灰色未执行占位卡」的类名 `.is-skipped` 已由
  [#132](https://github.com/liumingjian/db-qbs/issues/132) 从 `app.css` 里撤掉——只剩一段之后它没有对象。
- **实测佐证**：映射预检失败屏 `.precheck-reports .is-skipped` 计数 **0**，
  `.precheck-reports > section` 计数 **1**、类名 `is-failed`。

### V11 · 运行详情 · 映射预检失败 —— **前半 N/A，后半符合**
- **前半（第一张卡形状预检六条逐条列出）N/A**：对象随 ADR-0036 §5 一起没了。
- **后半（第二张卡逐列摆 + 末尾总计）实际观察**：`.precheck-reports` 下**恰好 1 个 section**
  （`class="is-failed"`），卡头 `映射预检 / sink`，表头五列 `列 / 源端 / 目标端 / 规则 / 建议`，
  三行逐列一条：
  - `C_NAME | VARCHAR2(200) | VARCHAR(80) | 目标列过窄 | 把目标列放宽到 VARCHAR(200)`
  - `LOAD_DATE | DATE | VARCHAR(20) | 类型不兼容 | 把目标列改成 DATETIME(0)`
  - `ROW_NO | （未映射） | int(11) NOT NULL | 未映射且不允许留空 | 目标表的 ROW_NO 列未被映射且不允许留空，请映射它或给它默认值`
  末行 `总计 3 项问题`。**「一次报全部」看得见，第三行还证到了 ADR-0038 §5 第 3 分支的
  「（未映射）」源列写法。符合。**

### V12 · 运行详情 · 形状预检失败屏没有错误码标签 —— **N/A，判据已退役**
- **对象不存在**：同 V6。这条守的是「形状预检是 source 本地成文、不属 ADR-0010 闭集」，
  而形状预检本身没了。**闭集不增不减这件事仍然成立**，只是不再由这一条守。

## 四、业务值的呈现

### V13 · 运行详情 · 哨兵逃逸
- **实际观察**：`load_date=2026-01-07`（`INTERNAL_PRECHECK_ESCAPE` + `column/value`）。
  `.sensitive-value` 是**独立框**，框内文字
  `源库真实业务值 | 显示即把源库真实值送进这台浏览器 | 显示 | column C_NAME | value 张三丰·测试客户名·2026`。
  默认态 `dl` 带 `is-masked`，`dd` 的计算样式 **`filter: blur(5px)`**、**`user-select: none`**，
  按钮文字「显示」。点「显示」之后 `dl` 类名清空、`filter: none`、`user-select: auto`、按钮变「隐藏」。
  **默认打码、需要显式动作才展开，符合。**

## 五、两个 id 与并发提示

### V14 · 运行历史列表
- **实际观察**：表头十列，`RUN_RECORD_ID` **第一列**，格内是 `button.history-link`、
  `color: rgb(44,138,240)`（品牌蓝、可点）；`RUN_ID` **紧随其后**，`class="mono run-id-cell"`、
  `color: rgb(81,90,110)`（等宽、灰一号）。`run_id` 缺失那行是 `.missing-run-id`、
  `color: rgb(128,134,149)`，文字「未发起，目标端不知道这次运行」。**符合。**

### V15 · 运行详情 · run 未发起 —— **符合（造态换了一个，判据原样）**
- **口径说明**：清单写的情形是「形状预检失败」，那一态已退役（见 V6）。`run_id` 为空这件事
  **仍然会发生**——源端在向 sink 发请求之前失败即是（本轮用 `SOURCE_CONNECT` 造）。
  **判据一个字没改，只换了造这一态的手段。**
- **实际观察**：`load_date=2026-01-08` 屏，`.run-identity` 五格实测
  `run_record_id rec-not-started | run_id 未发起，目标端不知道这次运行 | task_id task-holding |
  run_params load_date=2026-01-08 | staging_table —`。**不是空白、不是横杠**（横杠是
  `staging_table` 那一格，两个栏位谁也不替代谁）。同屏 `.terminal-block` 0、`.error-code` 0，
  结论条 `[Oracle 连接] 源端：连接 Oracle 失败：ORA-12541: TNS:no listener，未向 sink 发出请求。`**符合。**

### V16 · 发起对话框 · 有进行中 run
- **实际观察**：`rec-live` 停在 STREAMING，对同一任务同一业务日期再开发起对话框，`.stale-run-hint`
  三行：`该任务以同一组运行参数可能已有一个 run 进行中。` / `rec-live · 已跑 不到 1 分钟` /
  `这条提示可能滞后、不是门禁；真正的并发判断由后端在发起时完成。`。
  提示条底 `rgb(255,248,236)`、左边框 `3px solid rgb(255,153,0)`（`--warn` 系；清单写的是 info，
  按 #77 关闭时的决定不改）。提交按钮文字「发起」，实测 `disabled === false`、`cursor: pointer`。
  **只提示不拦，符合。**

### V17 · 运行详情 · 进行中
- **实际观察（STREAMING 态）**：「取消运行」按钮实测 `disabled = false`、`cursor: pointer`、
  `pointer-events: auto`——**常亮**。点下去 `.run-notice` 当场出「已发送 SIGTERM，等待子进程退出」。
- **实际观察（已受理、子进程还没报到）**：结论条「已受理，正在拉起」，三个 `.phase-item` 类名
  全是裸 `phase-item`（没有 `is-done` 也没有 `is-current`，三点全灰）；同一个按钮仍写「取消运行」、
  仍 `disabled = false`，点下去回的是 `.run-notice`「**run 尚未进入可取消阶段**」。
  **这正是「禁用状态本身会说谎」要的行为：不禁用，当场如实拒绝。符合。**

## 六、构建器与建表 SQL

### V18 · 建任务 · 手改 SQL —— **N/A，判据已退役**
- **对象不存在**：[ADR-0036](../../../adr/0036-task-spec-structured.md) §2 之后源端 SQL
  **由结构化规格现算**，界面上不再有手改入口。实测建任务对话框内 `<textarea>` 计数 **0**、
  「已被手改」命中 **0**、「重走向导」命中 **0**；`.generated-sql` 卡头逐字是
  **`源端 SQL | 由规格现算，只读`**。
- 这条守的是「整段替换 + 不恢复勾选状态」这句话不许悄悄消失，而**能被覆盖的手改内容本身没有了**。

### V19 · 建任务 · `target_table` 未填
- **实际观察**：目标表输入框清空后点「拿建表 SQL」，取列面板进 `fetch-ready`，**DDL 照给**，
  整段实测为：
  ```
  -- db-qbs 生成的目标表建表 SQL，请自行执行；产品不会替你建表。
  -- 下面那条主键不是可选项：写入走 upsert，目标表没有它时重跑会静默出重复行。
  CREATE TABLE <目标表名> (
    `ID` DECIMAL(19,0) NOT NULL,
    `C_NAME` VARCHAR(200) NULL,
    `LOAD_DATE` DATETIME(0) NULL,
    PRIMARY KEY (`ID`)
  ) DEFAULT CHARSET=utf8mb4;
  ```
  表名处渲染为 `.ddl-placeholder`「`<目标表名>`」，计算样式 `border: 1px dashed rgb(255,225,184)`、
  `background: rgb(255,248,236)`、`color: rgb(255,153,0)`（`--warn` 系虚边 + 底色）。
  同屏三轴元素计数为 0，**占位符不进三轴。不拦着人先看，符合。**

### V20 · 建任务 · 取列成功
- **实际观察**：结果卡内明写「这份取列结果**刷新即丢**：不进任务定义、不进存储，只是这一次的查看。」；
  三列 describe 结果 `ID NUMBER (19,0) / C_NAME VARCHAR2 (200) / LOAD_DATE DATE -`；
  DDL 下方 `.row-size-warning` 两段：「执行时若报 ERROR 1118 Row size too large」+
  「列宽合计超出 MySQL 单行上限，需缩窄字符列或拆表；**这是静态提示，产品不预先判定行长**。」
  DDL 头两行注释明写「产品不会替你建表」与「那条主键不是可选项」。**符合。**

### V21 · 建任务 · 目标端两字段 —— **N/A，判据方向已反**
- **判据被明文推翻**：清单要的是「**没有**目标表下拉、**没有**目标列列表，且屏上明写
  『是不画，不是没画完』」。[ADR-0038](../../../adr/0038-column-mapping-and-target-metadata-face.md) §3
  开出目标端元数据面，[ADR-0039](../../../adr/0039-v1-ui-increments.md) §5 明文要求把它们画出来，
  #131 已落地。**这条不是「没做到」，是被后来的裁定判废了。**
- **实测现状**：建任务对话框内 `<datalist>` **1 个**、`input[list]` **1 个**；
  「是不画」命中 **0**（那段文案已由 #131 按 ADR-0038 §3 删掉）；`.target-side-note` 现在承载的是
  ADR-0039 §7 的单位说明，逐字为「长度栏的单位是字符，而映射预检按字节判（ADR-0033）：
  utf8mb4 下 10 个汉字是 30 字节。第一版不统一两套口径，撞上时以预检结论为准。」
- **目标表下拉与目标列参考表的形态判据没有落空**，它们归 X5 / X7，已在
  [`v1-visual-walkthrough-20260819T110601Z.md`](v1-visual-walkthrough-20260819T110601Z.md) 实测过。

### V22 · 运行详情 · 映射预检失败
- **实际观察**：`.precheck-exit` 文案「目标表和这段 SQL 对不上。建表 SQL 在取列那一步现取，这屏不重给——
  免得你拿着旧的去撞 `ERROR 1050`。」，出口按钮**只有一个**「回到取列拿建表 SQL」；
  全屏正则搜 `CREATE TABLE` **命中 0 次**。**符合。**

### V23 · 全局
- **实际观察**：任务屏正则搜「重试」命中 **0**、「重新发起」命中 **0**（该按钮只在运行详情屏）；
  运行详情屏（终态）搜「重试」命中 **0**、「重新发起」命中 **1**（右上角按钮文字）。**符合。**
- **一处不在渲染面上的命中记在这里**：`web/src/errors.ts:2` 的兜底文案「请求失败，请稍后重试」
  说的是**一次 HTTP 请求**重试，不是重跑一个 run，且只在网络层出错时才渲染，本轮没有造出它。
  与前三轮的处置一致，不改。

## 七、导航与排版

### V24 · 全局导航
- **实际观察**：侧栏实测六项——`a.nav-item.is-active`「任务」（`rgb(44,138,240)`）、
  `a.nav-item`「运行历史」、`a.nav-item`「**数据源**」（两项均 `rgb(81,90,110)`）、
  分节标题 `p.nav-section`「非 V1 范围」，其下 `span.nav-item.is-disabled`「定时调度 M3+」
  「告警 M3+」，均 `rgb(128,134,149)`（灰），角标是 `span.nav-badge`「M3+」两个。
  导航文本里搜「构建器」**命中 0**。**构建器不是独立导航项、调度只是占位置灰标 M3+，符合。**
- **本轮的增量**：第三项「数据源」是 #130 加的（ADR-0039 §1），它与前两项**逐字同一套元素**
  （`<a class="nav-item">`），**没为它新造类名**，所以 V24 的判据不因此改。这也正是本票要给
  README §7 预留位置补的那一条。

### V25 · 任意屏
- **实际观察**：`body` 字体栈 `"PingFang SC", "HarmonyOS Sans SC", "Source Han Sans SC",
  "Noto Sans CJK SC", "Microsoft YaHei", "Hiragino Sans GB", sans-serif`，`color-scheme: light`，
  底 `rgb(240,242,245)`；`.mono` 字体栈 `ui-monospace, "SF Mono", "JetBrains Mono",
  "Cascadia Mono", Menlo, Consolas, "Liberation Mono", monospace` 且
  `font-variant-numeric: tabular-nums`；全页 `font-weight` 去重后只有 **`400` 与 `600`**（无 700）；
  `matchMedia('(prefers-color-scheme: dark)')` 为 false，样式表 **288 条**顶层规则里
  含 `prefers-color-scheme` 的条件规则计数 **0**。**符合。**
- 288 条对比 2026-08-16 那轮的 258 条：第一版新增的界面规则（数据源屏、映射两栏、目标列参考）
  都落在 `web/src/app.css`，`docs/design-system/tokens.css` 一个字节没动。

## 八、清单外的观察

1. **V5 的量测口径**：见 V5 那条的「踩了一次坑」。判据是可量的，**观察手段也得可量**——
   鼠标停在哪一行会改一个 28/255 的结论。探针里那句 `page.mouse.move(0, 0)` 是这一轮买下的。
2. **`.run-identity` 在 1440 下五格一行摆得开**（本轮截图实测）。#132 那轮记的
   「1024 视口下 `.run-identity` 末尾有一个空格子」是 1024 下的事，本轮没在 1024 复看——
   V 系列没有点名视口，那笔账仍挂在 `m3-visual-walkthrough-20260819T103910Z.md`。
3. **`.data-grid tbody tr:hover td` 用的是 `--mute-bg`，与 `.history-grid tr.is-expanded > td`
   用的 `--brand-dim` 是两套底色**，实测没有互相盖住。顺手记一笔，无待办。

## 九、门槛

- `npm run typecheck` 干净；`npm run build` `✓ built in 231ms`（CSS 30.08 kB、JS 268.76 kB）；
  `npm test` **6 files / 78 tests**。
- `cargo test --workspace`：见 [#133](https://github.com/liumingjian/db-qbs/issues/133) 的关闭评论。
- 全部在 mac 上真跑（`rexec`）。
- **台架未跑**：M1/M2/M3 三份仍是退役调用面，复跑并进 #134；C 系列入口归 #135 还不存在。
  这是 #126 Testing Decisions 的裁定，不是豁免。
- **X1–X8 未跑**：本票没碰数据源屏、映射两栏、目标端下拉的渲染结构（只改文档文字）→
  触发条件不成立。落点定死在 #130 / #131 / #136。
- **W1–W6 未跑**：本票没碰 `.precheck-reports` 布局与 `DiagnosticTable` 列结构 → 触发条件不成立。
