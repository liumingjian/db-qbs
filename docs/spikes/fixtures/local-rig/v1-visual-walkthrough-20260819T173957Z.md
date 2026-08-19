# 第一版渲染面走查记录（X1–X9）· 2026-08-19T17:39:57Z

- 清单：[`v1-visual-walkthrough.md`](v1-visual-walkthrough.md)（规格 ADR-0040 §6.3，本轮新增 X9）
- 触发条件：**第 2 条**——[#150](https://github.com/liumingjian/db-qbs/issues/150) 在运行历史里加了
  「重跑」入口、并让发起对话框接受预填。改动落在 `web/src/rerun.ts`（新增）、
  `web/src/HistoryScreen.tsx`、`web/src/StartRunDialog.tsx`、`web/src/App.tsx`、`web/src/ui.tsx`；
  **`web/src/app.css` 一行未动**（重跑位复用既有的 `.row-actions` + `.icon-button`，
  禁用态复用既有的 `.button:disabled, .icon-button:disabled`）。
- Git commit：`87672c0` + 工作区里 #150 的改动（本轮跑的是带改动的构建，`npm run build` 于 17:47Z 重跑过）。
- 观察时间：第一遍桩与探针跑于 2026-08-19T17:28Z；**代码评审改了三处之后于 17:47Z 重跑一遍**，
  本记录贴的是**重跑那一遍**的观察。改的三处：并跑提示改问 `historyPresentation`（结局不明的行
  `outcome` 也是 `null`，原判定会让重跑它时提示条指着这条已死记录）、禁用态的原因改挂外层
  `.row-actions` 的 `title` 与按钮 `aria-label`（浏览器不给 `disabled` 控件派发指针事件）、
  任务清单读不到时的措辞不再自称「还在读」。

## 造态源：**桩，不是活台架**——本轮据实回落，理由在此

ADR-0040 §6.3 增补（所有者 2026-08-19 裁定 Q5）判的是「X 系列优先对活台架取观察，桩是回落手段」。
**本轮八条 + 新增的第九条全部跑在桩上**（`walkthrough/v1-mock.py`），不粉饰。两条理由：

1. **X9 要的五种行，活台架造不出来。** 判据要一屏里同时有 FAILED / 结局不明 /
   进行中 / SUCCEEDED / **任务已删除** 五种历史行。`unknown_reason` 只在进程消失或服务重启后
   由 source 补写，「任务已删除但历史还在」要先删任务——两者都不是 `run-v1-acceptance.sh`
   的 C1–C6 能编排出来的态，硬造等于往真库里塞行（那正是 `x-rig-seed.sh` 拒绝做的事）。
2. **活台架此刻不在。** 上一轮（15:48Z）跑完留下的 Oracle / MySQL 容器已停，mac 上
   `docker ps` 只剩无关的 pg 容器。为 X1–X8 重建它要一次 Oracle XE 冷启 + release 构建 +
   10 万行 fixture 重灌，而 #150 **没有碰** X1–X8 的任何一处渲染代码
   （数据源屏、构建器、任务屏三处文件本轮零改动），重跑它们是回归复核，不是新主张。

因此本记录的效力口径是：**X9 是本轮的新主张，在桩上取全五态；X1–X8 是回归复核，
同样在桩上跑，用来证明「重跑列插进历史表之后，别处没塌」**。
X1–X8 的**活台架**实录以 [`v1-visual-walkthrough-20260819T155125Z.md`](v1-visual-walkthrough-20260819T155125Z.md)
（15:51Z，八条一条未回落）为准，本轮不推翻它。

- 编排：`walkthrough/run-x-walkthrough.sh`（不带 `X_RIG`）→ `walkthrough/v1-mock.py`（:18098）
  → `walkthrough/v1-probe.py`，喂的是真 `web/dist` 构建产物。
- 桩本轮新增：`/api/runs` 的五条历史行、`/api/runs/<id>`、`POST /api/runs`；
  任务「财务凭证」多一条「运行时填」的 `region`，好让预填三规则在一次观察里全部现形。
- 截图落在 mac 的 `/tmp/v1-visual/`：本轮新增 `x9-history-rerun.png`、
  `x9-prefilled-dialog.png`、`x9-concurrent-hint.png`。

## 逐条实录

**X1（导航第三项）**：`nav[aria-label="主导航"]` 下依次 `任务` / `运行历史` / `数据源`，
三者同为 `<a>`，class 实测 `nav-item is-active` / `nav-item ` / `nav-item `，第三项与第二项逐字一致。
其后是 `<p class="nav-section">非 V1 范围` 与两个 `nav-item is-disabled` 的 `<span>`。没有新造类。

**X2（列表七列、无搜索无筛选无连接状态）**：表头七列——
`名称` / `类型` / `连接` / `用户` / `口令` / `被引用` / `操作`；`#datasources` 下
`.search-field` 0 个、`<select>` 0 个、`.toolbar` 0 个，**没有连接状态列**。5 行里
Oracle 两条给 `//oracle-core:1521/ORCLPDB`、`//oracle-fa:1521/FAPDB`，
MySQL 三条给 `10.0.0.12:3306 / dw_stage`、`10.0.0.13:3307 / dw_mart`、`10.0.0.14:3306 / dw_spare`；
被引用列实测 `1 个任务` / `1 个任务` / `2 个任务` / `未被引用` / `未被引用`；
没设口令那条如实显示 `未设置`。

**X3（错凭据当场拦住）**：新建对话框 6 个字段，**打开时保存按钮就是禁用的**。
填错口令点「测试连接」→ `.form-error` 出
「Access denied for user 'sink'@'10.0.0.9' (using password: YES) / 连不上就存不进来：先确认地址、账号与网络放行，再回来测一次。」，
此刻 `submit_disabled: true`，`.error-code-tag` / `.inline-result` 各 0 个。改对口令再测：
`.inline-result` 出「连接成功 · 186 ms · dw_new」，computed style
`color rgb(81,90,110)` / `background rgba(0,0,0,0)` / `border 0px`——一行纯文字，不着成功色；
保存按钮转为放行。（本条在桩上跑，报错文本是桩编的；真 Oracle 报错的实录见 15:51Z 那份。）

**X4（改名免测连、删除被拒点名任务）**：挑「被引用」列非空的第一行（`生产核心库`，`1 个任务`）。
编辑态打开即 `submit_disabled: false`（免测连），口令栏空值 + 徽标
「已设置 · 留空 = 不改」，class `field-badge is-neutral`、`color rgb(128,134,149)` /
`background rgb(248,248,249)`——中性，不是成功色；类型字段 readonly。改完名仍放行。
删除被拒，报文「数据源仍被 1 个任务引用；请先改这些任务的数据源」，**点名列表**里是
`持仓日明细` 一条（#139 之后任务名只出现在列表里，不在红底那句话里重复），对话框保持打开。

**X5（目标表可过滤下拉）**：`input[list="target-table-options"]`，`readonly` 属性不存在（能直接键入），
`<datalist>` 五个 option：`HOLDING` / `HOLDING_DAILY` / `CUSTOMER` / `ORDER_ITEM` / `AUDIT_LOG`；
当前值 `HOLDING`。目标列参考表七列（`目标表列` / `类型` / `长度（字符）` / `可空` / `默认值` /
`约束` / `映射自`），未映射的 `CREATE_TIME` / `ROW_NO` 整行带 `is-unmapped`（探针以它为等待条件）。

**X6（映射两栏，1440 与 1024 各一次）**：表头 `选择` / `列名` / `字典类型` /
`精度 / 长度（字符）` / `可空` / `目标字段` / `主键`；四行的目标字段都是**常驻输入框**、
默认预填源名（`ID` / `C_NAME` / `LOAD_DATE` / `N_AMT`），主键勾选在其右侧，
`N_AMT` 未选中。两个视口下 `input_height` 均为 26、`row_height` 均为 44，
`table_overflow_x` 均为 0（不撑破行）；1024 下输入框右缘 895.23 < 单元格右缘 907.23。
`prefers-color-scheme` 媒体查询规则数 **0**——第一版没有暗色主题。
改目标名 `ID` → `CUST_ID_RENAMED` 后主键仍勾着（改回原值收尾）。

**X7（长度栏单位标注）**：表头是「精度 / 长度（**字符**）」，脚下 `.target-side-note`
「长度栏的单位是字符，而映射预检按字节判（ADR-0033）：utf8mb4 下 10 个汉字是 30 字节。
第一版不统一两套口径，撞上时以预检结论为准。」，style `color rgb(128,134,149)` /
`background rgba(0,0,0,0)` / `border 0px`——纯文字，不着色、不长成告警。

**X8（任务屏「源 → 目标」列）**：表头 `任务` / `源 → 目标` / `源表` / `目标表` / `主键` /
`条件` / `操作`；两行取值 `生产核心库 → 数仓 MySQL`、`财务库 → 数仓 MySQL`——**是名字不是 id**；
单元格 `children: 1`、`color rgb(23,35,61)`、背景透明，没引入新组件、不着色。

**X9（重跑入口三态 + 预填）**——本轮的新主张，五种行一屏内同时在场：

| run_record_id | 结局列实测 | 「操作」列实测 |
|---|---|---|
| `rec-failed-fa` | `DISCARDED 目标表未被触碰`（FAILED） | 可点的重跑，`aria-label` 为「重跑」 |
| `rec-live-fa` | `进行中 STREAMING` | **`—`**，没有按钮 |
| `rec-unknown` | `结局不明 进程消失，无终态日志` | 可点的重跑，`aria-label` 为「重跑」 |
| `rec-succeeded` | `SWAPPED 目标表已切换` | **`—`**，没有按钮 |
| `rec-task-gone` | `DISCARDED 目标表未被触碰`（FAILED，任务已删） | **禁用**的重跑；外层 `.row-actions` 的 `title` 与按钮 `aria-label` 都是「重跑（不可用）：任务已删除。重跑按任务当前的规格现算 SQL，规格没了就无从跑起。」 |

进行中与 SUCCEEDED 两行给的是破折号占位、不是空白单元格；任务已删除那行按钮**在原地**、按不动、
悬停有原因——三条判据逐条对上。

点 `rec-failed-fa` 的重跑：开的是**既有对话框**——标题 `发起运行 · 财务凭证`，
上下文行 `task-fa · 重跑自 rec-failed-fa`，确认键文字仍是 `发起`。
该行的运行参数是 `legacy_region=SH · load_date=2026-08-18`，而「财务凭证」当前规格的
「运行时填」是 `load_date` 与 `region`，预填实测：

- `load_date` = `2026-08-18`（**行里有的取行值**），`editable: true`
- `region` = 空串（**行里没有的留空**），`editable: true`
- `legacy_region` **没有出现在表单里**（**行里多出的丢弃**）

把 `region` 手敲成 `HZ`（证明预填能改），两格取值变为 `["2026-08-18", "HZ"]`，
随即 `.stale-run-hint` 自己冒出来：「该任务以同一组运行参数可能已有一个 run 进行中。/
rec-live-fa · 已跑 15 小时 20 分钟 / 这条提示可能滞后、不是门禁；真正的并发判断由后端在发起时完成。」
——既有并跑提示原样生效，重跑没有绕开它。

点「发起」：没有 `.form-error`，回运行详情；回到运行历史看清单，
**发起前** `rec-failed-fa` / `rec-live-fa` / `rec-unknown` / `rec-succeeded` / `rec-task-gone` 五条，
**发起后**变六条，多出来的是 `rec-new-6`，而 `rec-failed-fa` **原样还在**、位置与取值都没变。
新记录走的是 `POST /api/runs`（既有端点，零协议改动），SQL 由后端按任务当前规格现算——
历史行上钉的 SQL 快照没有被重放（web 侧压根没读它，见 `web/src/rerun.ts` 只取 `run_params`）。

**结局不明那条单独再点一次**（评审所指的那个坑）：`rec-unknown` 的重跑打开后
两格取值 `["2026-08-17"]`、填满后等 1.5 秒，`.stale-run-hint` **不存在**（实测 `null`）。
它的 `outcome` 同样是 `null`，若并跑提示按 `outcome` 认「进行中」，此刻会指着这条
早已死掉的记录说它还在跑。改后不会。

表头本轮实测十一列：`RUN_RECORD_ID` / `RUN_ID` / `任务` / `运行参数` / `结局` / `错误码` /
`行数` / `耗时` / `发起于` / **`操作`** / `详情`——新增的只有「操作」一列，`app.css` 零改动。

## 没跑的与为什么

- **V1–V25（M2 走查）**：未跑。触发条件是 `docs/design-system/README.md` 或
  `tokens.css` 变更，本轮两者零改动；封存点为 ADR-0040 §6.1（#133），自封存以来这两份文件未变。
- **W1–W6（M3 走查）**：未跑。触发条件是 `app.css` 里 `.precheck-reports` 布局或
  `DiagnosticTable` 列结构变更，本轮 `app.css` 一行未动、`DiagnosticTable` 未碰；
  最近一次实录 [`m3-visual-walkthrough-20260819T151821Z.md`](m3-visual-walkthrough-20260819T151821Z.md)。
