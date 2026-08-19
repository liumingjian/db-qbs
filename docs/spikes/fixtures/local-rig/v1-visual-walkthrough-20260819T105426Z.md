# 第一版渲染面走查记录（X 系列）· 2026-08-19T10:54:26Z

**触发条件**：第 2 条 —— [#130](https://github.com/liumingjian/db-qbs/issues/130) 新建了整屏数据源屏，
并落了 [ADR-0039](../../../adr/0039-v1-ui-increments.md) §9 那四条新增 CSS 里的**第 2、3 条**
（`.field-badge.is-neutral`、`.inline-result`）。

**这是 X 系列的第一次走查**（清单 `v1-visual-walkthrough.md` 于 #122 开出后尚未跑过）。
按 [#126](https://github.com/liumingjian/db-qbs/issues/126) Testing Decisions 定死的落点，
X1–X8 在 #130（本次）与 #131 各跑一次，#136 整体验收再跑一次。

**造态手段与清单原文的偏差（必须先读这一段）**：清单写的是「复用 `run-v1-acceptance.sh`
的 C1–C5 造态」，而那个入口归 [#135](https://github.com/liumingjian/db-qbs/issues/135)，
此刻还不存在；M1/M2/M3 三份也仍是退役调用面（改造归 [#134](https://github.com/liumingjian/db-qbs/issues/134)）。
沿用 M3 走查的处置：**桩后端 + 真实 `web/dist` 构建产物**造态，取机器观察（只观察不断言，
照 ADR-0028 §1）。工具在工作区的 `.playwright/`（`v1-mock.py` / `v1-probe.py` /
`run-x-walkthrough.sh`），**不进版本库**。
**这只回答「渲染出来没有」，一个数据正确性问题都不回答**；C 系列自动验收未跑（还不存在）。

跑在 mac 上（`rexec`），产物 `web/dist/assets/index-*.css` 含本票改动。
截图落在 mac 的 `/tmp/v1-visual/`（`x2-datasource-list.png`、`x3-test-failed.png`、
`x3-test-passed.png`、`x4-rename.png`、`x4-delete-refused.png`、`x8-task-list.png`），
X2 与 X3 两张已缩图取回**人眼看过**。

## 逐条观察

**X1（导航）**：侧栏主导航六个条目，顺序实测为
`任务` / `运行历史` / **`数据源`** / `非 V1 范围`（`<p class="nav-section">`）/ `定时调度 M3+` / `告警 M3+`。
数据源**是第三项**，标签是 `<a>`、类名 `nav-item`（未激活时就是空修饰），
与前两项**逐字同一套**；非 V1 占位项仍是 `<span class="nav-item is-disabled">`，位置没动。
**没有为它新造任何类名**。移动导航同样补了第三个按钮。

**X2（数据源列表，录 5 条）**：表头实测七列
`名称 / 类型 / 连接 / 用户 / 口令 / 被引用 / 操作`，5 行。逐行实际内容：

| 名称（+ id 小字） | 类型 | 连接 | 用户 | 口令 | 被引用 |
|---|---|---|---|---|---|
| 生产核心库 `ds-ora-core` | Oracle | `//oracle-core:1521/ORCLPDB` | `app_reader` | 已设置 | 1 个任务 |
| 财务库 `ds-ora-fa` | Oracle | `//oracle-fa:1521/FAPDB` | `fa_reader` | 已设置 | 1 个任务 |
| 数仓 MySQL `ds-my-dw` | MySQL | `10.0.0.12:3306 / dw_stage` | `sink` | 已设置 | 2 个任务 |
| 集市 MySQL `ds-my-mart` | MySQL | `10.0.0.13:3307 / dw_mart` | `mart` | 已设置 | 未被引用 |
| 备用 MySQL `ds-my-spare` | MySQL | `10.0.0.14:3306 / dw_spare` | `spare` | 未设置 | 未被引用 |

「连接」一列**Oracle 给 `connect_string`、MySQL 给 `host:port / database`，各显各的**，
没有拼一个假的统一连接串。屏上 `.search-field` **0 个**、`<select>` **0 个**、
`.toolbar` **0 个**——**没有搜索框、没有类型筛选**；表头里**没有「连接状态」列**。
截图上「被引用」一栏是纯文字，没有绿点、没有着色。

**X3（新建对话框 · 先填错凭据）**：对话框打开时 `type="submit"` 的「新建」按钮**已经是禁用的**
（`submit_disabled: true`），表单 6 个输入框（名称 / 主机 / 端口 / 库名 / 用户名 / 口令）。
填 `10.0.0.99 / dw_new / u / wrong` 点「测试连接」——出 `.form-error`，正文实测两行：

```
Access denied for user 'sink'@'10.0.0.9' (using password: YES)
连不上就存不进来：先确认地址、账号与网络放行，再回来测一次。
```

**驱动报错原样在第一行、人话在第二行**；屏上错误码标签 **0 个**（`.error-code-tag` /
`.terminal-block` 都查不到），`.inline-result` **0 个**。**保存按钮保持禁用**。

口令改成对的再测一次：出一个 `<div class="inline-result">`，正文
**`连接成功 · 186 ms · dw_new`**——**一行纯文字**，computed style 是
`color: rgb(81, 90, 110)`（`--dim`）、`background: rgba(0, 0, 0, 0)`（透明）、`border-width: 0px`，
**不是标签、没套 `--ok-bg`**。此时 `.form-error` **0 个**，**保存按钮放行**（`submit_disabled: false`）。

**X4（编辑一条被引用的数据源 · 只改名称，然后删它）**：打开「生产核心库」的编辑对话框——

- 口令输入框的值是**空串**（界面永不回读口令，连密文都不回）；
- 口令栏徽标实测文字「**已设置 · 留空 = 不改**」，类名 `field-badge is-neutral`，
  computed style `color: rgb(128, 134, 149)` / `background: rgb(248, 248, 249)` /
  `border-color: rgb(220, 222, 226)`——**中性色，不是成功色也不是告警色**
  （既有 `.field-badge` 是 `--warn` 三件套，这里换成了 `--mute` 三件套）；
- 「类型」一栏在编辑态是 `readonly` 的 `<input>`（**编辑不可改**），不是下拉；
- **打开即可保存**（`submit_disabled_on_open: false`），改完名字仍然可保存
  （`submit_disabled_after_rename: false`）——**只改名称免测连**，连接字段一字未动。

删它：点「删除」后出 `.form-error`，正文
**`数据源仍被 1 个任务引用：持仓日明细；请先改这些任务的数据源`**，
`.delete-copy` 里另有一段 `<li>` 列表，实测内容 `["持仓日明细"]`——**点名列出**，
不是一句笼统的「无法删除」。对话框仍开着，数据源没被删掉。

**X5 · 未建成**：构建器目标端的 `<datalist>` 与目标列参考表归 [#131](https://github.com/liumingjian/db-qbs/issues/131)，
本票没有建。探针实测：任务定义对话框里 `<datalist>` **0 个**、`input[list]` **0 个**。
（口令徽标那半句 X5 也点到了——它属数据源对话框，已在 X4 里实测为 `.field-badge.is-neutral`。）

**X6 · 未建成**：映射两栏归 #131。探针实测 `.data-grid .cell-input` **0 个**、
`.data-grid tr.is-unmapped` **0 行**。1024 / 1440 两个视口的对照因此**没有跑**——
对象不存在时跑两个视口不产生任何信息。

**X7 · 未建成**：长度栏的单位标注归 #131。探针实测构建器里含「长度」的表头只有一个，
文字是「**精度 / 长度**」——**还没有写成「长度（字符）」**，脚下也还没有那句静态说明。

> X5–X7 **照实记「未建成」，不记「通过」、也不跳过**——#126 Testing Decisions 明写的处置。
> 它们的正主是 #131，那一票要再跑一次整份 X1–X8。

**X8（任务屏）**：表头实测七列 `任务 / **源 → 目标** / 源表 / 目标表 / 主键 / 条件 / 操作`，
新增的那一列在第二位。两行的实际内容是
`生产核心库 → 数仓 MySQL` 与 `财务库 → 数仓 MySQL`——**是数据源名字，不是 `datasource_id`**
（id 只在数据源屏那一列的小字里出现）。单元格 computed style
`color: rgb(23, 35, 61)`（正文色）、`background: rgba(0, 0, 0, 0)`，
子元素只有 1 个（那个 `aria-hidden` 的箭头 `<span>`）——**不引入新组件、不着色**。

## 清单之外顺手记的

- **数据源屏没有工具条**：ADR-0039 §2 只定了列表七列，任务屏那条 `.toolbar`（搜索 + 刷新）
  照抄过来就会带出一个 ADR 明确否掉的搜索框。这里干脆一条工具条都不给，
  增删改之后由对话框回调重读清单。**刷新按钮因此也没有**——记在这里，若现场要它，
  正解是单独给一个刷新按钮，不是把任务屏那条工具条整条搬过来。
- **空态**：一条数据源都没有时给的是 `.empty-state`（「还没有数据源 / 先录一条 Oracle 源库与
  一条 MySQL 目标库，任务才有得选。」），与任务屏空态同一套元素。
- **建任务时下拉为空的引导**（ADR-0039 §8）**已单独造态观察**（桩加了一个把清单清空的开关）：
  建任务对话框的数据源那一节出现 **2 个** `<a class="text-button" href="#datasources">`，
  文字都是「**去「数据源」建一个 →**」，分别落在两个下拉的正下方；两个下拉的占位项分别是
  「尚无 Oracle 数据源」与「尚无 MySQL 数据源」。**没有「就地弹出新建数据源」**——
  对话框套对话框会让「测通才让存」有两个入口（ADR-0039 §8）。
  这一条不属 X1–X8 任何一格，是本票自己的交付面，记在这里。
