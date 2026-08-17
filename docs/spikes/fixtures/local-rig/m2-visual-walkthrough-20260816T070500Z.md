# M2 渲染面人工走查记录 —— 2026-08-16T07:05Z

**清单来源**：[`m2-visual-walkthrough.md`](m2-visual-walkthrough.md)（规格挂 ADR-0028 §6）。
**触发条件**：命中第 2 条——本轮改了 `docs/design-system/README.md` §61（V5 按 A 方案落地）。
**配套自动验收**：[`m2-acceptance-20260816T065710Z.md`](m2-acceptance-20260816T065710Z.md)，A1–A14 全 PASS；
M1 入口同一提交下 [`m1-acceptance-20260816T070036Z.md`](m1-acceptance-20260816T070036Z.md) 9/9 PASS。
**被走查的提交**：`4200ebf86639f1d7db7bdcbb9f4da8cb1c939aef`。
**台架**：`M2_KEEP_RIG=1 ./scripts/run-m2-acceptance.sh` 留下的实例，web UI `http://127.0.0.1:18088`。

## 关于观察手段

同上一轮，用 **Playwright CLI 代替人眼**：取渲染文本、元素**计算样式**与**几何位置**，
灰度那条用 `filter: grayscale(1)` 后**截图取真实像素亮度**。

**没有违反 ADR-0028 §1**：没有向 `run-m2-acceptance.sh` 或任何测试文件添加一行 DOM 断言，
浏览器只作一次性观察工具，产物就是这份文字记录。

**台架为走查做的编排**（不改产品代码）：child 模式在 `real` / `hang-streaming` / `fail-escape`
之间切换，各态从 UI 现场发起。走查结束时台架仍在（模式 `real`）。

## 结论速览

| 判定 | 条目 | 数 |
|---|---|---|
| **符合** | V1 V2 V3 V4 V6 V7 V8 V9 V10 V11 V12 V13 V14 V15 V16 V17 V18 V19 V20 V21 V22 V23 V24 V25 | 24 |
| **部分符合**（方向落地，强度不足） | V5 | 1 |

上一轮判「有偏差」的 12 条（V4 V6 V10 V11 V14 V16 V18 V19 V20 V21 V22 V25）本轮全部转为符合。
**另有两条清单外的观察**记在第八节，其中一条已在本轮修掉，另一条留票。

## 一、三条形状轴不得互换

### V1 · 运行详情 · 进行中
- **实际观察**：`hang-streaming` 下从 UI 发起 A5，阶段串渲染为
  `PREPARING 准备中 → STREAMING 传输中 → COMMITTING 提交中`，`.phase-item` **恰好 3 个**；
  三个 `.phase-dot` 计算样式均为 `width/height: 8px`、`border-radius: 50%`（真圆点），
  颜色依次 `rgb(25,190,107)`（`--ok`，已过）、`rgb(44,138,240)`（`--info`，当前）、
  `rgb(128,134,149)`（`--mute`，未到）。串尾 `→ 终态待定` 为纯文字，`.phase-after` 下**无圆点**。
  同屏 `.terminal-block` 计数 0、`.error-code` 计数 0。**符合。**

### V2 · 运行详情 · 成功
- **实际观察**：`real` 模式跑完 A5（已推行数 `100,000`、批次数 `20`、累计字节 `4481.7 KiB`），
  `outcome SUCCEEDED`，终态渲染为 `SWAPPED　目标表已切换`，计算样式
  `background-color: rgb(240,250,244)`（`--ok-bg`）、`border: 1px solid rgb(196,236,214)`（`--ok-bd`）、
  `border-radius: 3px`、`padding: 3px 9px`——是**块**且**有底色**。同屏无错误码标签、无阶段串。**符合。**

### V3 · 运行详情 · 校验失败（本轮用哨兵逃逸这一态）
- **实际观察**：`fail-escape` 下跑 A9，终态渲染为 `DISCARDED　目标表未被触碰`，计算样式
  `background-color: rgba(0,0,0,0)`（**完全无底色**）、`border: 1px solid rgb(220,222,226)`（`--line-strong`）。
  与 V2 的 `SWAPPED` 并列时，一个有底色一个没有——**这正是本轮 V5 A 方案改的地方**，
  改之前两者都有底色。**符合。**

### V4 · 运行详情 · 映射预检失败
- **实际观察**：A7 映射预检失败屏，`.error-summary` 的子元素顺序实测为
  `[0] .error-code.is-rejected "PRECHECK_FAILED HTTP 422"` → `[1] 纯 span「目标端：映射预检未通过：一次发现 1 项问题，未创建暂存表」`；
  几何位置 **标签 x=229、结论 x=419**，即**码在前、中文结论在其右**。上一轮这两者是反的。
  分档三例（全部实测计算样式）：
  - `PRECHECK_FAILED HTTP 422` → `border-style: dashed`、`rgb(251,210,199)` / `rgb(254,243,240)` / `rgb(237,64,20)`（`--crit` 系）
  - `VERIFY_FAILED 409` → `dashed`，同 `--crit` 系
  - `INTERNAL_PRECHECK_ESCAPE HTTP 500` → `border-style: solid`、`rgb(255,225,184)` / `rgb(255,248,236)` / `rgb(255,153,0)`（`--warn` 系）

  **符合。**

### V5 · 灰度打印 / 关掉颜色
- **实际观察（这条给数）**：`filter: grayscale(1)` 后对运行历史里的终态块**逐个截图取像素中位亮度**：

  | 块 | 内部底色 | 上边框 |
  |---|---:|---:|
  | `DISCARDED` | **255**（纯白，无底色） | 222 |
  | `SWAPPED` | **247** | 226 |

  错误码标签的分档在灰度下靠 `border-style`（`dashed` vs `solid`）承载，**是几何不是颜色**，
  灰度、黑白复印下都不丢。

- **判定：部分符合。** A 方案落地了——`DISCARDED` 现在真的无底色，"实心 vs 描边"在结构上成立，
  §61 与 §148 不再互相打架。**但强度要如实说**：两者在灰度下的差别只剩底色 **247 vs 255，约 3% 亮度**；
  边框亮度 222 vs 226 基本不可分。屏幕上勉强看得出，**复印件上大概率糊掉**。
  §148「黑白打印/复印下三轴仍可分」对轴三（虚边/实边）成立，对轴二**只是勉强成立**。
  再往前一步要动 `--ok-bg` 这类 token（影响面远超本票），已另开票，不在本轮做。

## 二、轴二只在墓碑真存在时出现

### V6 · 运行详情 · 形状预检失败
- **实际观察**：`real` 模式跑 A6（`SELECT * FROM t_m1_narrow`）。`.terminal-block` 计数 **0**、
  `.error-code` 计数 **0**；结论条 `.plain-conclusion` 文字为
  「**源端：SQL 形状预检未通过：一次发现 3 项问题，未向 sink 发出请求，未创建暂存表，目标表未被触碰。**」
  ——中文人话，且「目标表未被触碰」在结论条里、不是块。上一轮这里是照抄的英文
  `source-local SQL shape precheck found 3 problem(s)`。**符合。**

### V7 · 运行详情 · 映射预检失败
- **实际观察**：A7 屏 `outcome FAILED`，`.terminal-block` 计数 **0**；结论走错误码标签 + 中文结论。**符合。**

### V8 · 运行详情 · 进程消失 / 服务重启
- **实际观察**：`hang-streaming` 下从 UI 发起 A14 停在详情屏，随后重起 source。该屏轮询后转为
  `.unknown-conclusion.is-process_disappeared`，文字「结局不明 / 进程消失，无终态日志 /
  没有错误码，也没有目标端终态块。」；同屏 `.terminal-block` **0**、`.error-code` **0**、`.phase-line` **0**；
  左边框 `3px solid rgb(128,134,149)`（`--mute`），底色白。**符合。**

### V9 · 运行历史列表
- **实际观察**：10 行的「结局」列实测取值混排——`结局不明␣…`（`.unknown-summary`）×4、
  `SWAPPED␣目标表已切换`（块）×2、`DISCARDED␣目标表未被触碰`（块）×2、
  `未建暂存表` / `未发起`（`.neutral-outcome` 中性灰字）各 1。**块与中性灰字并存、语义上不齐**，
  没有被抹平。**符合。**

## 三、两段预检分开是结构不是文案

### V10 · 运行详情 · 形状预检失败
- **实际观察**：`.precheck-reports` 下**两张卡**，class 分别 `is-failed` / `is-skipped`；
  第二张卡标题 `映射预检 | sink`，正文为
  「**未执行——没跑到这一段，`sink` 不知道存在这个运行。**」上一轮这里只有「未执行」三个字。
  第一张卡末尾仍有「六条规则一次报告；本次未向 sink 发出请求。」**符合。**

### V11 · 运行详情 · 两段预检卡内容
- **实际观察**：
  - A6 屏第一张卡表头 `规则 | 结果 | 说明`，六条逐条列出，**说明列已是中文**，例如
    `业务日期半开区间 | 未通过 | WHERE 必须按 source_date_col 写业务日期的半开区间：>= :biz_date 且 < :biz_date + 1。`
    `无相对时间函数 | 通过 | 不能用 SYSDATE 这类相对时间函数，否则同一业务日期重跑结果会变。`
  - A7 屏第一张卡 **6 行结果全是「通过」也照列**（「一次报全部」看得见）；
    第二张卡表头 `列 | 源端 | 目标端 | 规则`，一行 `V_TEXT | VARCHAR2(200) | <missing> | 目标表缺少同名列`，
    末尾 `总计 1 项问题`。
  - 中文措辞与新建任务对话框**同源**（`web/src/shape.ts`），不是抄两份。

  **符合。**（表头末列仍叫「规则」不叫「违反」，同上一轮，记实况。）

### V12 · 运行详情 · 形状预检失败无错误码
- **实际观察**：A6 屏 `.error-code` 计数 **0**。**符合。**

## 四、业务值的呈现

### V13 · 运行详情 · 哨兵逃逸
- **实际观察**：`.sensitive-value` 是独立框，`border: 1px solid rgb(255,225,184)`、
  底色 `rgb(255,248,236)`（`--warn` 系）；标题「源库真实业务值 / 显示即把源库真实值送进这台浏览器 / 显示」；
  值区 `dl.is-masked`，`dd` 计算样式 `filter: blur(5px)`——**默认打码**，要点「显示」才展开。**符合。**

## 五、两个 id 与并发提示

### V14 · 运行历史列表 · 两个 id
- **实际观察**：`RUN_RECORD_ID` 第一列，单元格内 `<button class="history-link">`，
  `color: rgb(44,138,240)`（`--brand`）、等宽、`cursor: pointer`；
  `RUN_ID` 紧随其后，计算色 **`rgb(81,90,110)`（`--dim`）**、等宽、`font-variant-numeric: tabular-nums`。
  上一轮 `RUN_ID` 是 `rgb(23,35,61)`（主文本色），和任务名一样深。**符合。**

### V15 · 运行详情 · run_id 栏位
- **实际观察**：A6 屏身份网格里 `run_id = 未发起，目标端不知道这次运行`，不是空白也不是横杠；
  `run_record_id` 另有一栏，两者并存。**符合。**

### V16 · 发起对话框 · 有进行中 run
- **实际观察**：A5 停在 STREAMING 时对同任务同业务日期再点「发起运行」，填入日期后提示条出现，三行：
  1. 「该任务该业务日期可能已有一个 run 进行中。」
  2. 「**3eca341cb3e6933ad9ecb84b5fd08f5f · 已跑 不到 1 分钟**」（等宽、`--dim`）
  3. 「这条提示可能滞后、不是门禁；真正的并发判断由后端在发起时完成。」

  发起按钮 `disabled: false`、`opacity: 1`、`cursor: pointer`、实心 `--brand` 底。
  上一轮缺的 `run_record_id` 与已跑时长都补上了。
  **配色仍是 `warn` 系**（左边框 `rgb(255,153,0)`、底 `rgb(255,248,236)`）而非清单写的 info：
  这是本轮明确的选择——它说的是「可能撞车」，属提醒不属中性信息，且文案本身写明不是门禁，
  不会被读成拦截。**符合。**

### V17 · 运行详情 · 进行中的取消按钮
- **实际观察**：STREAMING 屏「取消运行」按钮 `disabled: false`、`opacity: 1`、`cursor: pointer`，**常亮**。**符合。**

## 六、构建器与建表 SQL

### V18 · 建任务 · 手改 SQL
- **实际观察**：编辑对话框**刚打开时 `.field-badge` 计数 0**（这段 SQL 是加载进来的，不是这一屏改的）；
  手改 textarea 之后出现角标「**当前 SQL 已被手改**」，计算样式 `color: rgb(255,153,0)`、
  底 `rgb(255,248,236)`、边 `rgb(255,225,184)`、`font-size: 11px`；几何上**角标右缘 1189 = SQL 框右缘 1189**，
  且位于 SQL 框上方——即 SQL 框右上角。
  确认模态仍为：标题「重走向导会覆盖你手改的 SQL」，正文「构建器会用新生成的四字段整段替换当前内容，
  也不会恢复上一次的勾选状态。」，按钮「取消」/「覆盖并重走向导」。**符合。**

### V19 · 建任务 · `target_table` 未填
- **实际观察**：留空 `target_table` 点「拿建表 SQL」，DDL 正常返回、**没拦人**；
  `<pre class="ddl-output">` 内现在有一个子元素 `<span class="ddl-placeholder">`，文本 `<目标表名>`，
  计算样式 `border: 1px dashed rgb(255,225,184)`（`--warn-bd`）、底 `rgb(255,248,236)`（`--warn-bg`）、
  色 `rgb(255,153,0)`（`--warn`）。该 span **不带任何三轴 class**（不是 `.terminal-block` / `.error-code` / `.phase-item`）。
  上一轮这里 `querySelectorAll('*')` 返回空，整段是纯文本。**符合。**

### V20 · 建任务 · 取列成功
- **实际观察**：结果卡上方新增一句「**这份取列结果刷新即丢：不进任务定义、不进存储，只是这一次的查看。**」
  `ERROR 1118` 提示保持原样：标题「执行时若报 ERROR 1118 Row size too large」，
  正文「列宽合计超出 MySQL 单行上限，需缩窄字符列或拆表；**这是静态提示，产品不预先判定行长。**」，
  DDL 首行注释仍是「请自行执行；产品不会替你建表。」**符合。**

### V21 · 建任务 · 目标端两字段
- **实际观察**：目标端仍只有 `target_table` / `target_date_col` 两个纯文本框，无下拉、无列列表；
  两字段下方新增一句「目标端只给这两个文本框：不给目标表下拉、不给目标列列表，
  **是不画，不是没画完**。目标表由你自己用下面的建表 SQL 建。」**符合。**

### V22 · 运行详情 · 映射预检失败的出口
- **实际观察**：A7 屏 `document.body.innerText.includes("CREATE TABLE")` 为 **false**——**不重给建表 SQL**；
  新增 `.precheck-exit` 面板，文案「目标表和这段 SQL 对不上。建表 SQL 在取列那一步现取，这屏不重给——
  免得你拿着旧的去撞 `ERROR 1050`。」+ 单个按钮「**回到取列拿建表 SQL**」。
  实点该按钮：打开「编辑 · A7 映射失败」对话框，里面有构建器入口与「拿建表 SQL」按钮。
  上一轮这屏一个入口也没有。**符合。**

### V23 · 全局措辞
- **实际观察**：任务屏、运行历史屏、历史行展开态三处 `重试` 出现次数均为 **0**；
  运行详情终态屏的动作按钮是「重新发起」。**符合。**

## 七、导航与排版

### V24 · 全局导航
- **实际观察**：侧栏四项——`任务`、`运行历史`（可点），`定时调度 M3+`、`告警 M3+`
  （`.is-disabled`，各带 `.nav-badge` = `M3+`），上方有「非 V1 范围」小标题。
  **构建器不是独立导航项**（它在新建/编辑任务对话框里）。**符合。**

### V25 · 字体与主题
- **实际观察**：`body` 字体族 `"PingFang SC", "HarmonyOS Sans SC", "Source Han Sans SC", …`（中文系统栈）；
  数字/id 走 `ui-monospace, "SF Mono", …` 独立等宽栈；`th`/`h1`/`.task-name`/`strong` 的
  计算 `font-weight` 集合为 **`{600}`**（无 700）；无 `prefers-color-scheme`、无 `data-theme`。
  `tabular-nums` 本轮补齐：运行历史的 `RUN_ID`、行数、耗时、时间戳单元格计算值均为
  `font-variant-numeric: tabular-nums`（上一轮列表屏全是 `normal`），运行详情的 `.run-metrics dd` 保持。**符合。**

## 八、附带看到的（清单没问）

1. **取列结果卡的「describe 类型」列一直是空的——本轮已修。**
   `POST /api/columns` 回的字段叫 `type`，`POST /api/builder/columns` 回的叫 `data_type`，
   前端两处都按 `data_type` 读，于是那一列永远读到 `undefined`。
   实测修前三行是 `ROW_ID | (空) | (8,0)`，修后是 `ROW_ID | NUMBER | (8,0)` /
   `V_TEXT | VARCHAR2 | (200)` / `D_BIZ | DATE | -`。按前端侧修，未动 API 语义。
   **上一轮走查记录里也是空的，当时没认出来。**

2. **成功运行的结论条是英文。** A5 跑成功后 `.success-conclusion` 显示
   `run completed successfully`——直接来自 API 的 `message`。形状预检失败那条本轮已改成中文
   （见 V6），成功这条没改，同一屏两种语言。已开票，不在本轮做。

3. **`ERROR 1118` 那条提示的准确性本轮没有复验**（上一轮 G1 已真在 MySQL 上撞过）。

## 台架状态

走查结束时台架**仍在**：web UI `http://127.0.0.1:18088`，source 停在 `real` 模式，
运行历史里累积了本轮走查现场发起的若干 run（A5 成功、A6 形状失败、A7 映射失败、A9 哨兵逃逸、
A14 被重启打断的那条）。拆台架命令见 `run-m2-acceptance.sh` 结尾打印的那行。
