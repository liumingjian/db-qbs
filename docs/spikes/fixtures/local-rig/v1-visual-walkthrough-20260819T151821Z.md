# 第一版渲染面走查记录（X1–X8）· 2026-08-19T15:18:21Z

- 清单：[`v1-visual-walkthrough.md`](v1-visual-walkthrough.md)（规格 ADR-0040 §6.3）
- 触发条件：**第 1 条**——第一版整体验收（[#136](https://github.com/liumingjian/db-qbs/issues/136)）必跑一次。
- Git commit：`33e0fdd`
- 观察时间：探针跑于 2026-08-19T15:08Z（截图同批），记录写于 15:18Z。

## 造态源：**活台架，不是桩**

所有者 2026-08-19 裁定（Q5）：X1–X8 对着 `scripts/run-v1-acceptance.sh` 跑完留下的**真服务**取观察，
桩降为回落手段。理由是第一版整体验收要答的是「这一版交付的东西真能用」，X3 / X4 用桩等于自己给自己发证。

- 后端：C1–C6 全绿那一轮留下的活台架，`http://127.0.0.1:18088`，二进制里嵌的是当前 `web/dist`
  （跑 C1–C6 时带了 `PATH="/opt/homebrew/bin:$PATH"`，前端确实重建过）。
- 真库：Oracle `//127.0.0.1:1521/XE`（spike）、MySQL `mysql:3306`（spike / 库 `qbs`）。
- 编排：`walkthrough/run-x-walkthrough.sh`（`X_RIG=1`）→ `walkthrough/x-rig-seed.sh` → `walkthrough/v1-probe.py`。
- **本轮八条一条都没有回落到桩。**

**一处造态补数**：台架自己只建 2 条数据源（C1/C2 要用的那两条），而 X2 的判据是「录满 5 条」。
差的 3 条由 `x-rig-seed.sh` 通过 `POST /api/datasources`（与人手同一个入口）补上，
**这 3 条不要求连得通**——POST 本身不测连（ADR-0039 §3 把「测通才让存」放在对话框上），
X2 只读列表的列与取值。要测连的是 X3，那条是真的。

截图落在 mac 的 `/tmp/v1-visual/`（`x2-datasource-list.png`、`x3-test-failed.png`、`x3-test-passed.png`、
`x4-rename.png`、`x4-delete-refused.png`、`x5-x7-builder-1440.png`、`x5-x7-builder-1024.png`、
`x8-task-list.png`），其中 x2 / x3-failed / x4-delete / x5-x7 两个视口 / x8 六张已缩图取回**人眼看过**。

## 逐条实录

**X1（导航第三项）**：`nav[aria-label="主导航"]` 下依次是 `任务` / `运行历史` / `数据源`，
三者同为 `<a>`、同一套 `nav-item` 类；第三项的 class 实测是 `"nav-item "`，与第二项完全一致
（第一项多一个 `is-active`）。其后是 `非 V1 范围` 分节（`<p class="nav-section">`）与两个
`nav-item is-disabled` 的 `<span>`（`定时调度 M3+` / `告警 M3+`）。**没有为数据源新造任何类**。

**X2（列表七列、无搜索无筛选无连接状态）**：表头实测七列——
`名称` / `类型` / `连接` / `用户` / `口令` / `被引用` / `操作`。
`#datasources` 下 `.search-field` **0 个**、`<select>` **0 个**、`.toolbar` **0 个**；
**没有「连接状态」列**。5 行如下（名称格下面挂 id，是既有的双行写法）：

| 名称 | 类型 | 连接 | 用户 | 口令 | 被引用 |
|---|---|---|---|---|---|
| V1 源库 | Oracle | `//127.0.0.1:1521/XE` | spike | 已设置 | 11 个任务 |
| V1 目标库 | MySQL | `mysql:3306 / qbs` | spike | 已设置 | 11 个任务 |
| 财务库（走查） | Oracle | `//oracle-fa:1521/FAPDB` | fa_reader | 已设置 | 未被引用 |
| 集市 MySQL（走查） | MySQL | `10.0.0.13:3307 / dw_mart` | mart | 已设置 | 未被引用 |
| 备用 MySQL（走查） | MySQL | `10.0.0.14:3306 / dw_spare` | spare | 未设置 | 未被引用 |

「连接」一列 **Oracle 给的是 `connect_string` 原文、MySQL 给的是 `host:port / database`**，
两种形态各显各的，没有拼一个假的统一连接串。「口令」一列在没设口令那条上如实显示 `未设置`。

**X3（错凭据当场拦住）**：打开「新建数据源」，对话框 4 个字段（名称 / 连接串 / 用户名 / 口令），
**打开时保存按钮就是禁用的**。填 Oracle `//127.0.0.1:1521/XE` + `spike` + 错口令 `definitely-wrong`，
点「测试连接」，回来的是**真的 Oracle 报错**：

```
源端：OCI Error: ORA-01017: invalid username/password; logon denied
连不上就存不进来：先确认地址、账号与网络放行，再回来测一次。
```

此刻保存按钮**仍然禁用**（`submit_disabled: true`），`.error-code-tag` / `.terminal-block` **0 个**，
`.inline-result` **0 个**——失败态只有一段 `.form-error`。
把口令改成对的 `spike123` 再测，出 `.inline-result`：`连接成功 · 326 ms · //127.0.0.1:1521/XE`，
computed style 实测 `color: rgb(81, 90, 110)` / `background: rgba(0, 0, 0, 0)` / `border-width: 0px`，
标签名 `DIV`——**一行纯文字，没有底色、没有边框，不是标签、不套 `--ok-bg`**。
保存按钮此时放行（`submit_disabled: false`），`.form-error` 归 0。

**X4（改名免测连、删除点名）**：探针**认「被引用」列、不认行号**（活台架上被引用的是哪一条取决于建的顺序）；
本轮挑中的是第一条命中的 `V1 源库`（被引用 = `11 个任务`）。

- 改名：编辑对话框**打开时保存按钮就是可点的**（`submit_disabled_on_open: false`），
  不必先测连。口令输入框值为空，旁边挂 `.field-badge.is-neutral`，文字 `已设置 · 留空 = 不改`，
  computed style `color: rgb(128, 134, 149)` / `background: rgb(248, 248, 249)` / `border-color: rgb(220, 222, 226)`
  ——**中性色，不是成功 / 失败色**（这条同时兑现 X5 后半句）。「类型」栏 `readonly`。
  改成 `V1 源库（改名）` 后保存按钮依然可点。改完取消退出，没有真存。
- 删除：点删除、确认，被**拒绝**，对话框**不关**（`still_open: true`）。报文逐个点名：

  > 数据源仍被 11 个任务引用：C1 引用检查（删除会被 409 拒）、C3 常量条件（GRP=A）、C3 运行时条件（GRP 每次填）、C4 主键 upsert 幂等、C4 目标表无唯一约束、C5① 主键列可空、C5② 非主键列可空放行、C5③ 未映射的非空无默认列、C6 基线（0 行）、C6 一万行、C6 十万行；请先改这些任务的数据源

  同样这 11 个任务名下面还有一份 `<li>` 列表，**逐条重复了一遍**（截图 `x4-delete-refused.png` 上看得很清楚：
  红底那段话里列了一遍，红框下面的项目符号列表又列了一遍）。判据只要求「点名列出、不是一句笼统的无法删除」，
  这一条**达成**；重复本身是观感问题，见文末尾账。

**X5（目标表可过滤下拉 + 允许直接键入）**：目标表栏实测是原生 `<input list="target-table-options">`
配 `<datalist id="target-table-options">`，`readonly` **没有**（`input_readonly: false`）——**可以直接键入**，
本轮就是靠键入把它从 `V1_C2` 改成 `V1_C4` 的。datalist 里 25 个 option，是活台架 MySQL 里的真表：
`M1_NARROW` / `M1_WIDE` / `M2_BAD` / `M3_B1`…`M3_B6` / `V1_C2` / `V1_C3` / `V1_C4` / `V1_C4_NOPK` /
`V1_C5_NULLABLE_PK` / `V1_C5_PASS` / `V1_C5_REQUIRED` / `V1_WIDE` / `ns_*` 五张 / `t_bulk_probe` /
`t_char_pad_probe` / `t_types_probe`。**「键入片段能筛」是原生 `<datalist>` 的浏览器行为，
本轮没有单独去验证下拉面板的筛选结果**——观察到的是选择器形态与可键入性，如实记在这里。

填入 `V1_C4` 失焦后，「目标表列参考」区块拉回真表的列：

| 目标表列 | 类型 | 长度（字符） | 可空 | 默认值 | 约束 | 映射自 |
|---|---|---|---|---|---|---|
| ROW_ID | decimal(8,0) | — | 否 | — | PRIMARY | ROW_ID |
| V_TEXT | varchar(80) | 80 | 是 | — | — | （未映射） |
| LOAD_DATE | datetime | — | 是 | — | — | LOAD_DATE |

未映射那行 `class="is-unmapped"`，`td` 的 computed color 是 `rgb(128, 134, 149)`，
另两行是 `rgb(23, 35, 61)`——**整行压暗，肉眼在 `x5-x7-builder-1440.png` 上也是明显更浅的一行**。
脚注：`这份结果刷新即丢：它不进任务定义、不进 SQLite（ADR-0038 §8）。长度栏同为字符；映射预检按字节判。`

**X6（映射两栏 · 1440 与 1024 各一次）**：本轮打开的是任务列表第一条 `C1 引用检查（删除会被 409 拒）`
（源表 `SPIKE.T_V1_C2`）。表头七列：`选择` / `列名` / `字典类型` / `精度 / 长度（字符）` / `可空` / `目标字段` / `主键`。
四行的目标字段**都是常驻输入框**（不是点开才出现的编辑态），取值 `ROW_ID` / `DEST_NAME` /
`DEST_AMOUNT` / `LOAD_DATE`——**预填的是任务里存的目标名，不同名映射如实显示**；
每行两个复选框，第一个是「选择」（四行全勾），最后一个是「主键」，**位置在目标字段右侧**，
只有 `ROW_ID` 勾着。

| 视口 | 输入框高 | 行高 | 输入框右缘 | 单元格右缘 | 表格横滚 | 暗色主题媒体查询 |
|---|---|---|---|---|---|---|
| 1440 | 26 px | 44 px | 1104.66 | 1116.66 | 0 | 0 条 |
| 1024 | 26 px | 44 px | 894.95 | 906.95 | 0 | 0 条 |

两个视口下**输入框都比行矮（26 < 44）、右缘都在单元格内 12 px、表格 `scrollWidth - clientWidth` 都是 0**
——不撑破行、不横滚。整份样式表里 `prefers-color-scheme` 的规则数**两次都是 0**，
**第一版确实没有暗色主题**。

顺带复核 ADR-0039 增补 1 的渲染面：把勾了主键那行的目标字段从 `ROW_ID` 改成 `CUST_ID_RENAMED`，
主键勾选**仍然勾着**（`key_still_checked: true`），改完已还原。

**X7（长度单位标注）**：映射栏表头写的是 `精度 / 长度（字符）`，目标表列参考那栏写的是 `长度（字符）`，
两处都带「字符」二字。脚下那句静态说明实测为：

> 长度栏的单位是字符，而映射预检按字节判（ADR-0033）：utf8mb4 下 10 个汉字是 30 字节。 第一版不统一两套口径，撞上时以预检结论为准。

computed style `color: rgb(128, 134, 149)` / `background: rgba(0, 0, 0, 0)` / `border-width: 0px`
——**纯文字、不着色、不长成告警块**，与 W3 的态度一致。

**X8（任务屏「源 → 目标」列）**：任务屏表头 `任务` / `源 → 目标` / `源表` / `目标表` / `主键` / `条件` / `操作`，
第二列 11 行取值**全是 `V1 源库 → V1 目标库`**——**给的是数据源名字，不是 id**。
该单元格 computed style `color: rgb(23, 35, 61)`（与正文同色）、背景透明、
`children: 1`——**没有引入新组件、没有着色**。

## 本轮撞出的东西（照所有者裁定 Q2 归口）

1. **删除被拒的报文重复了一遍任务名**（X4）：`.form-error` 那段话里已经列全 11 个任务，
   下面的 `<li>` 列表又列了一遍。**不碰客户五条需求**（搬得动 / 搬得对 / 幂等 / 预检 / 内存），
   按 Q2 归**尾账票**，不在 #136 当票修。
2. 除此之外，X1–X8 逐条都与清单判据吻合，没有需要当票修的缺陷。

## 探针这一侧的修正（本轮）

`walkthrough/v1-probe.py` 新增 `X_RIG=1` 活台架模式并做了三处去位置化：X3 改用真 Oracle 凭据、
X4 认「被引用」列而不是行号、对话框填值一律**按标签文字**取输入框（不认 `:nth-of-type`）。
起因是 v1 的构建器与数据源屏比 M3 那套多出一批字段，按位置写的选择器会随 Oracle / MySQL
两套字段集变形。这次真跑一遍确认三处改动都成立。
