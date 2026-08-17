# M2 渲染面走查记录 —— 2026-08-16T17:20Z

**清单来源**：[`m2-visual-walkthrough.md`](m2-visual-walkthrough.md)（规格挂 ADR-0028 §6）。
**触发条件**：命中第 2 条——本轮改了 `docs/design-system/tokens.css`（新增 `--ok-fill` / `--ok-ink`）
与 `docs/design-system/README.md`（§3 轴二、§8 承诺），按 `CLAUDE.md` 的 M2 视觉门禁必须重跑整份 V1–V25。
**配套自动验收**：[`m2-acceptance-20260816T164629Z.md`](m2-acceptance-20260816T164629Z.md)，A1–A14 全 PASS。
**被走查的改动**：[#89](https://github.com/liumingjian/db-qbs/issues/89)（轴二实心块的填充与墨色）
与 [#92](https://github.com/liumingjian/db-qbs/issues/92)（形状预检卡副标题不再回显英文原文），
走查时两者尚未提交，工作区即被走查对象。
**台架**：`M2_KEEP_RIG=1 run-m2-acceptance.sh` 留下的实例，web UI `http://127.0.0.1:18088`，
work root `tmp.JaJi3gJMw4`。

## 关于观察手段

同前三轮，用 **Playwright 代替人眼**：取渲染文本、元素**计算样式**、几何位置与真实像素亮度。
脚本在未跟踪的 `.playwright/` 下（`wt-runs.py` / `walkthrough-history.py` /
`walkthrough-tasks.py` / `wt-extra.py`），产物就是这份文字记录。

**没有违反 ADR-0028 §1**：没有向 `run-m2-acceptance.sh` 或任何测试文件加一行 DOM 断言。
台架编排只切 child mode（`hang-streaming` / `real`），不改产品代码——
`M2_KEEP_RIG` 交出来的台架固定停在 `hang-streaming`，V2/V6/V7/V10~V12/V15/V22 要的是 `real`，
用 `.playwright/switch-child-mode.sh` 按 `start_source` 同一套参数重起进程换模式。

## 结论速览

| 判定 | 条目 | 数 |
|---|---|---|
| **符合** | V1–V25 全部 | 25 |

**V5 本轮首次达标**：实测差 **28/255 = 11.0%**（判据 ≥ 25/255），前三轮都是 3.1% 的「部分符合」。
另有三条清单外观察记在第八节。

## 一、三条形状轴不得互换

### V1 · 运行详情 · 进行中
- **实际观察**：`hang-streaming` 下从 UI 发起「A10 并发」，run `2798257d…`。阶段串 `.phase-item`
  **恰好 3 个**：`PREPARING 准备中`（`is-done`，圆点 `rgb(25,190,107)`）、
  `STREAMING 传输中`（`is-current`，圆点 `rgb(44,138,240)`）、`COMMITTING 提交中`（未到，`rgb(128,134,149)`），
  三点均 `8px × 8px`、`border-radius: 50%`。串尾 `→ 终态待定` 是纯文字，
  `.phase-after` 下 `.phase-dot` 计数 **0**。同屏 `.terminal-block` 计数 0、`.error-code` 计数 0。
  结论条「运行中 STREAMING」，四个数 `已推行数 3 / 当前批次序号 1 / 已用时 00:00 / 累计字节 96 B`。**符合。**

### V2 · 运行详情 · 成功
- **实际观察**：`real` 模式跑完「A5 正常 10 万行」，终态渲染 `SWAPPED　目标表已切换`，计算样式
  **`background-color: rgb(205,235,217)`（`--ok-fill` `#CDEBD9`）**、
  **`color: rgb(11,102,55)`（`--ok-ink` `#0B6637`）**、`border: 1px solid rgb(196,236,214)`、
  `border-radius: 3px`、`padding: 3px 9px`——是块且有底色，底色比上一轮的 `rgb(240,250,244)` 明显深。
  同屏 `.error-code` 计数 0、`.phase-item` 计数 0。四个数
  `已推行数 100,000 / 批次数 20 / 累计批次耗时 00:00 / 累计字节 4481.7 KiB`；
  结论条 `目标端：运行成功：已推送 100,000 行，暂存表已切换为目标表。`**符合。**

### V3 · 运行详情 · 终态 DISCARDED
- **实际观察**：运行历史里 A8 校验失败（`VERIFY_FAILED 409`）与 A9 哨兵逃逸
  （`INTERNAL_PRECHECK_ESCAPE 500`）两行的终态均为 `DISCARDED　目标表未被触碰`，计算样式
  `background-color: rgba(0,0,0,0)`（**完全无底色**）、`border: 1px solid rgb(220,222,226)`、
  `color: rgb(81,90,110)`。与 `SWAPPED` 并列时一个实心一个描边。**符合。**

### V4 · 运行详情 · 映射预检失败（含 5xx 一例）
- **实际观察**：A7 映射预检失败屏，`.error-summary` 子元素实测顺序与几何位置为
  `[0] .error-code.is-rejected「PRECHECK_FAILED HTTP 422」x=229` →
  `[1] 纯 span「[类型映射] 目标端：映射预检未通过：一次发现 1 项问题，未创建暂存表」x=419`，
  **码在前、中文结论在其右**。分档两例（历史列表实测计算样式）：
  - `PRECHECK_FAILED 422` / `VERIFY_FAILED 409` → `border-style: dashed`、
    边 `rgb(251,210,199)` / 底 `rgb(254,243,240)` / 字 `rgb(237,64,20)`（`--crit` 系）
  - `INTERNAL_PRECHECK_ESCAPE 500` → `border-style: solid`、
    边 `rgb(255,225,184)` / 底 `rgb(255,248,236)` / 字 `rgb(255,153,0)`（`--warn` 系）
  **符合。**

### V5 · 灰度打印 / 关掉颜色 —— 本轮的主角
- **实际观察**：运行历史列表整页 `filter: grayscale(1)` 后**整页截图**，
  取块左内边距内 6×14 像素片的中位亮度：
  **`SWAPPED` 块 227，`DISCARDED` 块 255，差 28/255 = 11.0%**（判据 ≥ 25/255）。
  取样片内 `min == max`，即窗口完全落在填充上、没切到字形。
  同列纯文字格（`未建暂存表`）中位 250.5。
- **判定：符合。** 前三轮记的 3.1%（247 vs 255）由 [#89](https://github.com/liumingjian/db-qbs/issues/89)
  修掉，本轮是判据升格为可量之后的第一次实测。
- **口径记账**：预估值与实测值差 3。ADR-0025 增补三与 tokens.css 注释原写 224（离线按 ITU-601
  `0.299R+0.587G+0.114B` 折算 `#CDEBD9`），屏幕上量到 **227**——浏览器的 `filter: grayscale(1)`
  按 **Rec.709**（`0.2126R+0.7152G+0.0722B`）折算，同一个色算出来就是高 3。
  差 3 不动结论（28 ≥ 25），三处文档已改成以实测为准。
- **取样窗口也修了一处**：上一版脚本取块左侧 10×14、且只截视口。10 宽会切进第一个字母
  （块 `padding-left` 只有 9px），只截视口则会把落在 1000px 以下的 `DISCARDED` 行裁成黑边
  （量出 0 而不是纸白）。本轮改成整页截图 + 6 宽窗口，`min == max` 即窗口纯净的自证。

## 二、轴二只在墓碑真存在时出现

### V6 · 运行详情 · 形状预检失败
- **实际观察**：`real` 模式跑「A6 形状失败」，run `58e12e4d…`，`.terminal-block` 计数 **0**；
  结论条 `源端：SQL 形状预检未通过：一次发现 3 项问题，未向 sink 发出请求，未创建暂存表，目标表未被触碰。`
  ——「目标表未被触碰」在人话里，不是块。**符合。**

### V7 · 运行详情 · 映射预检失败
- **实际观察**：A7 屏 `.terminal-block` 计数 **0**（`.error-code` 计数 1、`.phase-item` 计数 0）。**符合。**

### V8 · 运行详情 · 进程消失
- **实际观察**：V17 点「取消运行」后 run `9e7ae31b…` 落成
  `outcome FAILED | 结局不明 | 进程消失，无终态日志 | 没有错误码，也没有目标端终态块。`，
  实测该屏 `.terminal-block` 计数 0、`.error-code` 计数 0。历史列表里
  `服务重启，结局未知`（A13）与多行 `进程消失，无终态日志` 同样如此。**符合。**

### V9 · 运行历史列表
- **实际观察**：「结局」列 23 行**三种形态混排、宽度参差**——
  块：`SWAPPED 目标表已切换` w=149、`DISCARDED 目标表未被触碰` w=176；
  中性灰字 `.neutral-outcome`：`未发起` w=36、`未建暂存表` w=60，均 `rgb(128,134,149)`；
  「结局不明」族用的是第三种 `.unknown-summary`（`display: flex`、无底色、`rgb(81,90,110)`），
  两行文字 w=176 / **h=37**，比其他行都高。**齐不了，符合。**

## 三、两段预检分开是结构不是文案

### V10 · 运行详情 · 形状预检失败
- **实际观察**：`.precheck-reports` 下**恰好两张卡**。第一张 `is-failed`（白底 `rgb(255,255,255)`），
  卡头「SQL 形状预检 / source 本地」；第二张 `is-skipped`，底色 `rgb(248,248,249)`（灰），
  正文「未执行——没跑到这一段，sink 不知道存在这个运行。」，无表格、无 `small`。**符合。**

### V11 · 运行详情 · 映射预检失败
- **实际观察**：第一张卡表头 `规则 / 结果 / 说明`，**六条全部列出**，A6 屏是「3 未通过 + 3 通过」
  逐条并列（业务日期半开区间 未通过、WHERE 无额外谓词 通过、每列显式命名 未通过、
  列精度可确定 未通过、无相对时间函数 通过、源/目标日期列同名 通过），
  A7 屏六条**全部标「通过」也照列**；第二张卡表头 `列 / 源端 / 目标端 / 规则`，
  一行 `V_TEXT · VARCHAR2(200) · <missing> · 目标表缺少同名列`，末尾 `总计 1 项问题`。**符合。**

### V12 · 运行详情 · 形状预检失败
- **实际观察**：该屏 `.error-code` 计数 **0**；`.precheck-exit` 计数也是 0（那个出口只属映射失败屏）。**符合。**

## 四、业务值的呈现

### V13 · 运行详情 · 哨兵逃逸
- **实际观察**：历史列表展开 `INTERNAL_PRECHECK_ESCAPE` 那一行，`column` / `value` 在独立的
  `.sensitive-value` 框里，框头「源库真实业务值 · 显示即把源库真实值送进这台浏览器 · [显示]」。
  **默认态**实测 `dl.is-masked`、`dd` 计算样式 `filter: blur(5px)`、`user-select: none`；
  点「显示」后 `filter: none`、`user-select: auto`，按钮文字变「隐藏」，
  值 `column V_TEXT / value 真实业务值-1265` 才可读。**符合。**

## 五、两个 id 与并发提示

### V14 · 运行历史列表
- **实际观察**：表头顺序 `RUN_RECORD_ID / RUN_ID / 任务 / 业务日期 / 结局 / 错误码 / 行数 / 耗时 / 发起于 / 详情`；
  第一列是 `.history-link` 可点按钮、等宽 + `tabular-nums`；`run_id` 紧随其后，
  有值时 `td.mono.run-id-cell`（`--dim`），无值时 `td.missing-run-id`。**符合。**

### V15 · 运行详情 · 形状预检失败
- **实际观察**：`run_identity` 区实测为
  `run_record_id | 58e12e4d… / run_id | 未发起，目标端不知道这次运行 / task_id | 8db6d011… /
  biz_date | 2026-08-14 / staging_table | —`——`run_id` 栏是那句话，不是空白也不是横杠。**符合。**

### V16 · 发起对话框 · 有进行中 run
- **实际观察**：`hang-streaming` 下先发起一个 run 停在 STREAMING，再对同一任务同一日期打开发起对话框，
  `.stale-run-hint` 显示「该任务该业务日期可能已有一个 run 进行中。」+
  `2798257d… · 已跑 不到 1 分钟` +「这条提示可能滞后、不是门禁；真正的并发判断由后端在发起时完成。」；
  提交按钮文字「发起」，实测 `disabled === false`、`cursor: pointer`。**符合。**
  （提示条底 `rgb(255,248,236)`、左边框 `3px solid rgb(255,153,0)`，即 `--warn` 系；
  清单里写的是 info，此处按 #77 关闭时的决定不改。）

### V17 · 运行详情 · 进行中
- **实际观察**：「取消运行」按钮实测 `disabled=false`、`opacity: 1`、`cursor: pointer`、
  `pointer-events: auto`——**常亮**。run 进 STREAMING 后点下去，`.run-notice` 当场出「已发送 SIGTERM」，
  该 run 随即落成「结局不明 / 进程消失，无终态日志」。**符合。**
- **顺带证到清单外的一条**：run 刚发起、子进程还没报到（结论条「已受理，正在拉起」、
  三个阶段圆点全灰）时点同一个常亮按钮，回的是 `.run-notice`「**run 尚未进入可取消阶段**」——
  **按钮不因此禁用，而是当场如实拒绝**。这正是 V17 那句「禁用状态本身会说谎」要的行为，
  只是清单只写了 STREAMING 一态。（本轮第一次跑走查脚本就撞上了它，不是刻意构造。）

## 六、构建器与建表 SQL

### V18 · 建任务 · 手改 SQL
- **实际观察**：在「A2 取列」的编辑对话框里手改 `source_sql` 后，字段右上角出现常驻角标
  `.field-badge`「当前 SQL 已被手改」，计算样式底 `rgb(255,248,236)`、字 `rgb(255,153,0)`、
  `1px solid rgb(255,225,184)`（`--warn` 系）。点「重走向导」弹出确认框：
  「重走向导会覆盖你手改的 SQL」+「构建器会用新生成的四字段整段替换当前内容，也不会恢复上一次的勾选状态。」
  + 按钮「取消 / 覆盖并重走向导」。**符合。**

### V19 · 建任务 · `target_table` 未填
- **实际观察**：`target_table` 清空后直接点「拿建表 SQL」，取列面板进 `fetch-ready`，**DDL 照给**：
  `CREATE TABLE <目标表名> ( ROW_ID DECIMAL(8,0) NULL, V_TEXT VARCHAR(200) NULL,
  D_BIZ DATETIME(0) NULL, KEY idx_d_biz (D_BIZ) ) DEFAULT CHARSET=utf8mb4;`，
  表名处渲染为 `.ddl-placeholder`「`<目标表名>`」，计算样式
  `border: 1px dashed rgb(255,225,184)`、`background: rgb(255,248,236)`、`color: rgb(255,153,0)`
  （`--warn` 系虚边 + 底色，不进三轴）。**符合。**

### V20 · 建任务 · 取列成功
- **实际观察**：结果卡内明写「这份取列结果刷新即丢：不进任务定义、不进存储，只是这一次的查看。」；
  三列 describe 结果 `ROW_ID NUMBER (8,0) / V_TEXT VARCHAR2 (200) / D_BIZ DATE -`；
  DDL 下方「执行时若报 ERROR 1118 Row size too large / 列宽合计超出 MySQL 单行上限，
  需缩窄字符列或拆表；**这是静态提示，产品不预先判定行长**。」DDL 头两行注释明写
  「产品不会替你建表」与「这条索引不是可选项」。**符合。**

### V21 · 建任务 · 目标端两字段
- **实际观察**：对话框内 `select` 计数 **0**、`datalist` 计数 **0**，字段只有
  `source_sql / source_date_col / target_table / target_date_col` 四个文本框；`.target-side-note` 写
  「目标端只给这两个文本框：不给目标表下拉、不给目标列列表，**是不画，不是没画完**。目标表由你自己用下面的建表 SQL 建。」**符合。**

### V22 · 运行详情 · 映射预检失败
- **实际观察**：`.precheck-exit` 文案「目标表和这段 SQL 对不上。建表 SQL 在取列那一步现取，这屏不重给——
  免得你拿着旧的去撞 ERROR 1050。」，出口按钮**只有一个**「回到取列拿建表 SQL」；
  全屏正则搜 `CREATE TABLE` **命中 0 次**。**符合。**

### V23 · 全局
- **实际观察**：任务屏正则搜「重试」命中 **0**、「重新发起」命中 0（该按钮只在运行详情屏）；
  运行详情屏搜「重试」命中 **0**、「重新发起」命中 **1**（终态屏右上角按钮文字）。**符合。**

## 七、导航与排版

### V24 · 全局导航
- **实际观察**：主导航实测五项——`a.nav-item.is-active`「任务」（`rgb(44,138,240)`）、
  `a.nav-item`「运行历史」（`rgb(81,90,110)`）、分节标题 `p.nav-section`「非 V1 范围」，
  其下 `span.nav-item.is-disabled`「定时调度 M3+」「告警 M3+」，均 `rgb(128,134,149)`（灰），
  角标是 `span.nav-badge`「M3+」。导航文本里搜「构建器」**命中 0**。**符合。**

### V25 · 任意屏
- **实际观察**：`body` 字体栈 `"PingFang SC", "HarmonyOS Sans SC", "Source Han Sans SC",
  "Noto Sans CJK SC", "Microsoft YaHei", "Hiragino Sans GB", sans-serif`，
  `color-scheme: light`，底 `rgb(240,242,245)`；`.mono` 字体栈
  `ui-monospace, "SF Mono", "JetBrains Mono", "Cascadia Mono", Menlo, Consolas, "Liberation Mono", monospace`
  且 `font-variant-numeric: tabular-nums`；全页 `font-weight` 去重后只有 **`400` 与 `600`**（无 700）；
  `matchMedia('(prefers-color-scheme: dark)')` 为 false，样式表 258 条顶层规则里
  含 `prefers-color-scheme` 的条件规则计数 **0**。**符合。**

## 八、清单外的观察

1. **上一轮第 1 条已修掉（#92）。** 形状预检卡的副标题不再是英文原文
   `source-local SQL shape precheck found 3 problem(s)`，本轮实测为
   **「六条形状规则中 3 条未通过。」**，与通过态那句「六条形状规则已通过。」同句式。
   `detail.message` 仍原样从 API 回（A1–A14 断言面不受影响），只是不再进 DOM。
2. **`--ok-bd` 在实心块上确实看不见了。** `SWAPPED` 的边 `rgb(196,236,214)` 与底
   `rgb(205,235,217)` 灰度几乎同亮。**这是 #89 认下的代价、已写进设计系统 §3 轴二**，
   不当缺陷报——实心块本来就不该靠边框立住。`DISCARDED` 的 `rgb(220,222,226)` 边照旧清晰。
3. **走查脚本的取样窗口曾经在骗人。** 见 V5 末尾两条：10px 宽窗口会切进字形、
   只截视口会把视口外的块裁成黑边量出 0。两处都已修，`min == max` 现在是窗口纯净的自证。
   前三轮 247 vs 255 那组数落在视口内、且切到字形的部分不足以改变中位数，结论仍然成立。
