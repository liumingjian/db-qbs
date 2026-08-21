# M2 渲染面走查实录 · V1–V25（2026-08-21，P2 作业中心换皮后）

- **触发**：`CLAUDE.md` 视觉门禁表第 1 行——`docs/design-system/tokens.css` 与
  `docs/design-system/README.md` 本轮都改了（[ADR-0043](../../../adr/0043-p2-job-center.md) §3 §10），
  字面命中「**任何**改动」这一条。
- **判据版本**：`m2-visual-walkthrough.md`（含 2026-08-21 的 V9 / V14 / V24 / V25 四条改判）。
- **怎么跑的**：`docs/spikes/fixtures/local-rig/walkthrough/run-v-walkthrough.sh`
  （`v-mock.py` 桩后端 + `v-probe.py` 机器观察，喂真实的 `web/dist` 构建产物）。
  真跑在用户 mac 上（`rexec --mac lmj-macbook`），服务器只留静态操作。
- **台架改动**：`v-mock.py` 由**一个任务七条历史**改成**七个任务各一条**。
  原因写在文件里：ADR-0043 §2 之后一行 = 一个任务 + 它最近一次运行，
  「同屏同时看到 SWAPPED 与 DISCARDED」不能再靠一个任务的多条历史。
  `v-probe.py` 的 `history_walk` 随之改名 `drawer_walk`，轴二 / 轴三改在详情抽屉里看。

原始输出：`/tmp/v-report.json`（16.5 KB，逐条 JSON）。下面是实际观察，不是「通过」。

---

## 一、三条形状轴

**V1（进行中 · 阶段串三个点）**：`.phase-item` 命中 **3 个**，逐字
`PREPARING 准备中` / `STREAMING 传输中` / `COMMITTING 提交中`，
点的几何全是 `8x8 / border-radius 50%`；已完成绿 `rgb(82,196,26)`、当前蓝 `rgb(60,126,255)`、
未到灰 `rgba(0,0,0,.45)`。尾部跟一句 `→ 终态待定`，**它自己不带点**（`phase_after_dot_count = 0`）。
同屏 `.terminal-block` 0 个、`.error-code` 0 个——**进行中不出终态块**。

**V2（成功 · 终态实心块）**：`.terminal-block.is-swapped` 1 个，文字 `SWAPPED　目标表已切换`，
底 `rgb(205,235,217)`（`--ok-fill`）、字 `rgb(11,102,55)`（`--ok-ink`）、`1px solid rgb(183,235,143)`。
结论条：`目标端：运行成功：已推送 100,000 行，暂存表已切换为目标表。`

**V3（校验失败 · 终态描边块）**：`.terminal-block.is-discarded` 1 个，
文字 `DISCARDED　目标表未被触碰`，**底透明** `rgba(0,0,0,0)`、`1px solid rgb(217,217,217)`。
同屏一个 `.error-code`：`VERIFY_FAILED HTTP 409`。

**V4（错误码标签 · 4xx 虚线 / 5xx 实线）**：
- 4xx：`PRECHECK_FAILED HTTP 422`，`border-style: dashed`、`rgb(255,241,240)` 底、`rgb(245,34,45)` 字。
- 5xx：`INTERNAL_PRECHECK_ESCAPE HTTP 500`，`border-style: solid`、`rgb(255,251,230)` 底、`rgb(250,173,20)` 字。
两者与人话结论条**并排在 `.error-summary` 里**，标签在左（x=288）、人话在右（x=478）。

**V5（灰度可分 · 必须贴实测数字）**：整页 `filter: grayscale(1)` 后取块内 6x14 片的中位亮度——
- `SWAPPED` 实心块：**227**
- `DISCARDED` 描边块：**255**（就是纸白，因为它无底色）
- 差 **28 / 255 = 11.0%**，**过 ≥25 的门槛**。
**取样口径本轮变了一处**：两块**各自开一个抽屉**取样，不再同屏。
判据量的是两个块各自的块内中位亮度差，同屏是旧形态（历史列表）的副产物，不是判据本身。

**V7（映射预检失败 · 不出终态块）**：`.terminal-block` **0 个**、`.error-code` 1 个。
未创建暂存表的那一次没有「目标表效果」可言，块不出是对的。

**V8（结局不明 · 只出人话，不出错误码）**：`.terminal-block` 0、`.error-code` 0、`.phase-item` 0。
`.unknown-conclusion.is-process_disappeared` 一块，文字
`结局不明 | 进程消失，无终态日志 | 无法确认目标表是否被修改，请到目标库核对。`

**V9**：**N/A（判据已随 ADR-0043 §4 退役，且方向反转）**。运行历史列表整屏取消；
作业中心的「运行状态」列是一维索引，五个词都是同一种实心方角标签，**齐是对的**，
形态判据改由 X17 守（实录见同日 `v1-visual-walkthrough-20260821T051427Z.md`）。

---

## 二、详情抽屉（原运行历史列表）

**V14**：**N/A（判据已随 ADR-0043 §2 退役）**。「两个 id 谁也不替代谁」由 V15 兼守，实测在下面。

**V15（`run_id` 不是空白也不是横杠 + 两个 id 并存）**：
- 运行详情页身份网格：`运行记录 | rec-not-started`、
  `目标端运行号 | 未发起，目标端不知道这次运行`、`暂存表 | —`。
  同屏 `.terminal-block` 0、`.error-code` 0（源端就失败了，没向 sink 发过请求）。
- 抽屉里（`rec-verify`）：`run_record_id` **在标题旁**——`运行详情 · 校验失败那条` + `rec-verify`，
  样式 `rgba(0,0,0,.45)` / `ui-monospace` / `12px`；`run_id` 在「运行参数与标识」里，
  栏名「目标端运行号」、值 `20260819121000_cccccc`。**两个栏位同时在场，各是各的。**

**V2 / V3 / V4 在抽屉里的形状**：与运行详情页**逐字同一套值**（见上），
分区顺序 `结论 · 行数核对 · 分段耗时 · 任务定义 · 运行参数与标识 · 当次执行的源端 SQL`。

**V25（排版 · 改判为字重 500）**：
- `body` 字体栈 `-apple-system, system-ui, "Segoe UI", Roboto, "Helvetica Neue", Arial,
  "PingFang SC", "HarmonyOS Sans SC", "Source Han Sans SC", "Noto Sans CJK SC",
  "Microsoft YaHei", "Noto Sans", sans-serif`；`color-scheme: light`；底 `rgb(244,247,249)`。
- 数字走独立等宽栈 `ui-monospace, "SF Mono", …`，`font-variant-numeric: tabular-nums`。
- **强调字重实测 500**：表头 `500`、卡内标题块 `500`；整页出现过的字重集合只有 `["400","500"]`——
  **没有 600、没有 700**（原判「600 不是 700」按 ADR-0043 §3 改判为 500，取值来自对 x2doris 的实测）。
- **没有暗色主题**：`prefers-color-scheme` 条件规则 **0 条**（全表 311 条规则），
  `matchMedia('(prefers-color-scheme: dark)')` 为 false。
- **深色侧栏不是暗色主题**：侧栏 `rgb(0,21,41)`、卡片 `rgb(255,255,255)`、内容区透明见外层浅灰。
  三者同屏，是参照物**浅色布局**的一部分（ADR-0043 §8）。

---

## 三、作业中心与构建器

**V24（改判 · 导航三项 + 深色侧栏 + 折叠态图标居中）**：
- 导航实测三项，全是 `A.nav-item`、**没有新造的样式**：
  `作业中心`（`is-active`，字 `rgb(255,255,255)`）、`数据源`、`系统设置`（后两者 `rgba(255,255,255,.75)`）。
- 「构建器」在导航里命中 **0 次**——它仍不是独立导航项（原判这一半照跑）。
- `.nav-badge` **0 个**：「调度占位灰标 `M3+`」的对象在 P0 已被撤掉，本条补记这次静默退役。
- 侧栏底 `rgb(0,21,41)`。折叠态：侧栏 48px、菜单块宽 40px、
  **图标水平中心偏移 0.0px（居中）**、`.nav-text` `display: none`。
  参照物那道「蓝块被切成一条竖边」的渲染瑕疵**没有照抄**（ADR-0043 文末自决 2）。

**V23（不出「重试」）**：作业中心 `重试` 命中 0、`重新发起` 命中 0；
运行详情页 `重试` 0、`重新发起` 1。**一次也没有把「重跑」写成「重试」。**

**V11（映射预检卡）**：`.precheck-reports section` **1 个**（`is-failed`），
表头 `映射预检 | 目标端`，五列 `列 · 源端 · 目标端 · 规则 · 建议`，3 行逐条给建议，
收尾 `总计 3 项问题`；**灰色「未执行」占位卡 0 个**。

**V22（预检出口 · 给「编辑任务」不给「建表」）**：
出口条文字「目标表结构与本次取数的列对不上。请在目标库中调整目标表，或回到任务编辑修改字段映射。」
按钮只有 `编辑任务` 一个，`建表` 类字样命中 **0 次**。

**V13（业务值告警框 · 默认打码）**：默认 `dl.is-masked`、`filter: blur(5px)`、`user-select: none`，
按钮「显示」；点开后 `filter: none`、`user-select: auto`、按钮变「隐藏」，
框头那句「显示即把源库真实值送进这台浏览器」始终在。

**V16（并跑提示）**：`.stale-run-hint` 一块，左 3px `rgb(250,173,20)` 竖条 + `rgb(255,251,230)` 底，
文字「该任务以同一组运行参数可能已有一次运行正在进行。/ rec-live · 已跑 41 小时 13 分钟 /
状态可能有延迟，以发起结果为准。」**确认键仍是「发起」且可点**（`disabled: false`，`cursor: pointer`）——
提示是提示，不是拦截。

**V17（取消按钮 · 禁用态会说谎）**：`STREAMING` 时按钮「取消运行」可点，点后回执
「已发送 SIGTERM，等待子进程退出」；`已受理` 态三个圆点全灰、按钮**仍可点**，
点后如实回「run 尚未进入可取消阶段」。**两态都没有把按钮画成禁用。**

**V21**：判据方向已反（ADR-0038 §3 / ADR-0039 §5 明文开出目标表下拉）。
实测 `datalist` 1 个、`input[list]` 1 个、单位说明「长度栏的单位是字符。MySQL 使用 utf8mb4 时，
1 个汉字通常按 3 字节计算。」、「是不画」字样 0 次。

**V18**：源端 SQL 由规格现算、只读——`textarea` **0 个**，区块头 `源端 SQL | 只读预览`，
「已被手改」0 次、「重走向导」0 次。

---

## 四、V19 / V20：**对象已经不存在了（不是本轮删的）**

| # | 观察 |
|---|---|
| **V19** | `.modal .column-fetch-section` **0 个**，「拿建表 SQL」按钮不存在，`.fetch-ready` / `.ddl-placeholder` 无从谈起 |
| **V20** | 同上，取列卡的「取列范围说明」与「行宽告警」一并没有对象 |

**根因**：`47a2fed`（*Prepare x2doris P1 frontend handoff*，2026-08-21）把构建器里整段
「目标表建表 SQL / 拿建表 SQL / `.fetch-ready`」摘掉了。`git show 47a2fed -- web/src/App.tsx`
逐行可查；它的父提交 `33e9ec5` 里 `column-fetch-title` 还在（命中 2 次），
`85805b1`（v1 整体验收）里也还在——**v1 验收那次 V19/V20 是有对象的**。

**这不是本轮 P2 改动造成的回归**，但它是一次**没被门禁接住**的回退：
摘掉这段的那一票没有跑 V1–V25，`CLAUDE.md` 规则 1 挡的正是这种「改了界面不跑走查」。
本轮如实记下，**不代所有者决定要不要把它加回来**——见交接件的收尾问题。

`api.ts` 里的 `fetchColumns()`（`POST /api/columns`）还在，但界面上**没有任何一处调用它**。

---

## 五、结论

V1–V25 中：
- **判据成立、实测已贴**：V1 / V2 / V3 / V4 / V5 / V7 / V8 / V11 / V13 / V15 / V16 / V17 /
  V21 / V22 / V23 / V24 / V25（含 V24 / V25 两条改判）。
- **N/A（判据已退役）**：V6 / V9 / V10 / V12 / V14 / V18。
- **对象不存在（`47a2fed` 引入的回退，非本轮）**：V19 / V20。
