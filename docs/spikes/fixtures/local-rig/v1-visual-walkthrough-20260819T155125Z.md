# 第一版渲染面走查记录（X1–X8）· 2026-08-19T15:51:25Z

- 清单：[`v1-visual-walkthrough.md`](v1-visual-walkthrough.md)（规格 ADR-0040 §6.3）
- 触发条件：**第 2 条**——[#139](https://github.com/liumingjian/db-qbs/issues/139) 改了数据源屏
  删除对话框的渲染结构（红底那段话不再重复任务名）。改动落在 `web/src/DatasourceScreen.tsx`
  与 `web/src/datasource.ts`，**没有碰** `web/src/app.css`。
- Git commit：`af01b11` + 工作区里 #139 的三处改动（本轮跑的是带改动的构建）。
- 观察时间：台架跑于 2026-08-19T15:48Z，探针跑于 15:50Z，记录写于 15:51Z。

## 造态源：**活台架，不是桩**

照 ADR-0040 §6.3 增补（所有者 2026-08-19 裁定 Q5）：X1–X8 对着 `scripts/run-v1-acceptance.sh`
跑完留下的真服务取观察，桩只是回落手段。**本轮八条一条都没有回落到桩。**

- 后端：本轮重跑的 C1–C6（**6/6 PASS**，报告在 mac 的 `/tmp/v1-acceptance-139.md`，
  未入库——它是为造态跑的，不是本票主张的验收）留下的活台架，`http://127.0.0.1:18088`。
- **二进制里嵌的是带 #139 改动的 `web/dist`**：跑台架时带了 `PATH="/opt/homebrew/bin:$PATH"`，
  build.rs 真重建了前端（这一点是本轮观察成立的前提——不重建就是拿旧 UI 自证）。
- 真库：Oracle `//127.0.0.1:1521/XE`（spike）、MySQL `mysql:3306`（spike / 库 `qbs`）。
- 编排：`walkthrough/run-x-walkthrough.sh`（`X_RIG=1`）→ `walkthrough/x-rig-seed.sh`
  → `walkthrough/v1-probe.py`。
- 造态补数同上一轮：台架自建 2 条数据源，X2 判据要「录满 5 条」，差的 3 条由 `x-rig-seed.sh`
  走 `POST /api/datasources` 补上，这 3 条不要求连得通（POST 本身不测连）。

截图落在 mac 的 `/tmp/v1-visual/`；本轮把 `x4-delete-refused.png` 缩图取回**人眼看过**
（这一条正是本票改的那处），其余七条按机器观察逐条记录。

## 逐条实录

**X1（导航第三项）**：`nav[aria-label="主导航"]` 下依次是 `任务` / `运行历史` / `数据源`，
三者同为 `<a>`；class 实测 `nav-item is-active` / `nav-item ` / `nav-item `，
第三项与第二项**逐字一致**。其后是 `<p class="nav-section">非 V1 范围` 与两个
`nav-item is-disabled` 的 `<span>`（`定时调度 M3+` / `告警 M3+`）。没有为数据源新造类。

**X2（列表七列、无搜索无筛选无连接状态）**：表头实测七列——
`名称` / `类型` / `连接` / `用户` / `口令` / `被引用` / `操作`。
`#datasources` 下 `.search-field` **0 个**、`<select>` **0 个**、`.toolbar` **0 个**；
**没有「连接状态」列**。5 行：

| 名称 | 类型 | 连接 | 用户 | 口令 | 被引用 |
|---|---|---|---|---|---|
| V1 源库 | Oracle | `//127.0.0.1:1521/XE` | spike | 已设置 | 11 个任务 |
| V1 目标库 | MySQL | `mysql:3306 / qbs` | spike | 已设置 | 11 个任务 |
| 财务库（走查） | Oracle | `//oracle-fa:1521/FAPDB` | fa_reader | 已设置 | 未被引用 |
| 集市 MySQL（走查） | MySQL | `10.0.0.13:3307 / dw_mart` | mart | 已设置 | 未被引用 |
| 备用 MySQL（走查） | MySQL | `10.0.0.14:3306 / dw_spare` | spare | 未设置 | 未被引用 |

Oracle 给 `connect_string` 原文、MySQL 给 `host:port / database`，各显各的；没设口令那条如实显示 `未设置`。

**X3（错凭据当场拦住）**：新建对话框 4 个字段，**打开时保存按钮就是禁用的**。
填 `//127.0.0.1:1521/XE` + `spike` + 错口令 `definitely-wrong` 点「测试连接」，回来的是真 Oracle 报错：

```
源端：OCI Error: ORA-01017: invalid username/password; logon denied
连不上就存不进来：先确认地址、账号与网络放行，再回来测一次。
```

此刻 `submit_disabled: true`，`.error-code-tag` / `.inline-result` **各 0 个**。
换成对口令再测：`.inline-result` 出「连接成功 · 306 ms · //127.0.0.1:1521/XE」，
computed style 是 `color rgb(81,90,110)` / `background rgba(0,0,0,0)` / `border 0px`——
**一行纯文字，不着底色不成标签**；保存按钮放行（`submit_disabled: false`），`.form-error` 0 个。

**X4（只改名免测连；删除被拒点名任务）· 本票改的就是这一条**：
被引用的行取「被引用」列不等于 `未被引用` 的第一条，实测是 `V1 源库`（11 个任务）。
编辑它：打开时保存按钮**就是可用的**（`submit_disabled_on_open: false`），口令栏空、
徽标 `field-badge is-neutral`「已设置 · 留空 = 不改」（`color rgb(128,134,149)` /
`background rgb(248,248,249)`，中性色），类型栏 readonly；只改名称后仍可提交——**免测连成立**。

删除它，服务端回 409，对话框里实测：

- `.form-error` 全文一行：**`数据源仍被 11 个任务引用；请先改这些任务的数据源`**
  ——**不再带任何任务名**（改前是 11 个名字用顿号连成的一大串）。
- `<li>` 列表 11 条，逐条点名：`C1 引用检查（删除会被 409 拒）` / `C3 常量条件（GRP=A）` /
  `C3 运行时条件（GRP 每次填）` / `C4 主键 upsert 幂等` / `C4 目标表无唯一约束` /
  `C5① 主键列可空` / `C5② 非主键列可空放行` / `C5③ 未映射的非空无默认列` /
  `C6 基线（0 行）` / `C6 一万行` / `C6 十万行`。
- 对话框保持打开（`still_open: true`），没有误删。

**人眼看 `x4-delete-refused.png`（缩到 900px 取回）**：红框里现在只有一行短句，紧接着是
项目符号列表一份，**红框与列表不再各列一遍同样的 11 个名字**；对话框整体高度明显收住，
底部「取消 / 删除」两个按钮与列表之间留白正常，没有被顶出可视区。
判据（ADR-0039 §4 / ADR-0037 §7「点名列出，不是一句笼统的『无法删除』」）**仍然达成**——
点名的那份由 `<li>` 列表买单。

**X5（目标表可过滤下拉 · 1440 与 1024 两个视口同）**：目标表输入框 `list="target-table-options"`、
`readonly` 为否（**允许直接键入**），`<datalist>` 实测 25 个选项（`M1_NARROW` … `t_types_probe`），
当前值 `V1_C2`。目标端列面表头七列：`目标表列` / `类型` / `长度（字符）` / `可空` / `默认值` /
`约束` / `映射自`；脚注「这份结果刷新即丢：它不进任务定义、不进 SQLite（ADR-0038 §8）。
长度栏同为字符；映射预检按字节判。」口令栏中性徽标见 X4 那段（同一个 `.field-badge.is-neutral`）。

**X6（映射两栏 · 1440 / 1024 各看一次）**：表头七列
`选择` / `列名` / `字典类型` / `精度 / 长度（字符）` / `可空` / `目标字段` / `主键`。
四行（`ROW_ID` / `SRC_NAME` / `SRC_AMOUNT` / `LOAD_DATE`）的目标字段都是**常驻输入框**、
不 disabled，默认预填源名（不同名那条实测预填的是已存的 `DEST_NAME`）；每行 2 个勾选框，
**主键勾在目标字段右侧**（`ROW_ID` 勾中）。
两个视口下 `input_height: 26` / `row_height: 44`（**等高、不撑破行**），
`table_overflow_x: 0`（横向不溢出）；`dark_theme_media: 0`——**没有暗色主题**（ADR-0025 复核）。
改名跟随主键：`ROW_ID` 改成 `CUST_ID_RENAMED` 后主键仍勾着。
未映射行压暗在 X5 的目标端列面上实测：`V_TEXT` 行 class `is-unmapped`、
`color rgb(128,134,149)`，其余两行 `rgb(23,35,61)`。

**X7（长度栏单位标注）**：表头写 `精度 / 长度（字符）`，脚下静态说明一句：
「长度栏的单位是字符，而映射预检按字节判（ADR-0033）：utf8mb4 下 10 个汉字是 30 字节。
第一版不统一两套口径，撞上时以预检结论为准。」
computed style `color rgb(128,134,149)` / `background rgba(0,0,0,0)` / `border 0px`——
**纯文字、不着色、不长成告警**。

**X8（任务屏「源 → 目标」列）**：表头七列 `任务` / `源 → 目标` / `源表` / `目标表` /
`主键` / `条件` / `操作`；11 行的该列都是 `V1 源库 → V1 目标库`——**显示的是名字不是 id**。
单元格 `color rgb(23,35,61)`（正文色）、`background rgba(0,0,0,0)`、子元素 1 个，
**不引入新组件、不着色**。

## 结论

八条全部对着活台架取到观察，**没有一条回落到桩**。#139 改的那处（X4 的 `.form-error`）
按判据仍然点名列出引用任务——名字由 `<li>` 列表出，红底那段话只留数量与动作；其余七条与
上一轮（`v1-visual-walkthrough-20260819T151821Z.md`）逐项对得上，没有别处漂移。
