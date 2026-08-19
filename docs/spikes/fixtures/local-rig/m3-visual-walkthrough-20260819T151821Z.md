# M3 渲染面走查记录（W1–W6）· 2026-08-19T15:18:21Z

- 清单：[`m3-visual-walkthrough.md`](m3-visual-walkthrough.md)（规格 ADR-0032 §8，2026-08-19 订正见 #121）
- Git commit：`33e0fdd`
- 观察时间：探针跑于 2026-08-19T15:16Z（截图同批），记录写于 15:18Z。

## 触发判定：**本轮不是被触发的，是顺手跑的**

按 ADR-0040 §6.2 与所有者 2026-08-19 裁定（Q1 / Q6）：

- **W1–W6 在 #136 的触发判定是「已在 [#132](https://github.com/liumingjian/db-qbs/issues/132) 兑现」**，
  不因为本轮跑了 M3 台架而重新触发。依据是 ADR-0040 §6 正面排过第一版的走查计划，
  比 `CLAUDE.md` 表格「每次 M3 验收」的字面外推更近。
- **实查过的证据**：W 的封存点 `1348df1` 之后，`e63c492`、`aa510db` 两个提交确实动了前端，
  但逐行 diff 核过，**一行都没碰 `.precheck-reports` 布局，也没碰 `DiagnosticTable` 的列结构**
  ——清单那两条触发条件都不成立。
- **那为什么还是跑了**：裁定 Q6——`m3-probe.py` 的选择器随 v1 构建器漂移，本票要当票修；
  修完不跑一遍反而别扭。**因此下面这份观察是附带证据（corroboration），不是触发后的补跑。**
  V1–V25 仍然不跑，理由见本轮 #136 的报告。

## 造态源：桩后端（清单 #121 订正允许的偏差）

`m3-visual-walkthrough.md` 的 2026-08-19 订正已写明：B1–B6 那套编排是退役形态，
在 [#122](https://github.com/liumingjian/db-qbs/issues/122) 就绪前，走查用桩后端喂 `web/dist` 造态，
并在记录里明写这处偏差。本轮照此办理：

- 编排 `walkthrough/run-w-walkthrough.sh`（端口 18099）→ `walkthrough/m3-mock.py` 起桩 →
  `walkthrough/m3-probe.py` 取观察。桩喂的是仓库里当前的 `web/dist`。
- **桩是 M3 时期那份，缺 v1 新增的两个端点**：本轮实测页面对 `POST /api/target/tables` 与
  `POST /api/target/columns` 各拿到一个 **404**（浏览器控制台两条 error）。受影响的只有
  目标表下拉的候选与「目标表列参考」区块——**W1–W6 六条判据一条都不落在这两处**，
  目标表名本轮是直接键入的。这两处的真态由 X5 在活台架上单独看过。

截图落在 mac 的 `/tmp/m3-visual/`（`w1-w6-1440.png`、`w2-1024.png`、`w3-w4-column-fetch.png`、
`w5-rejected.png`、`builder.png`），前四张已缩图取回**人眼看过**。

## 逐条实录

**W1（预检表五列、建议列不留空、一次报全）**：运行详情页在 `PRECHECK_FAILED HTTP 422`
横幅下只有**一段** `.precheck-reports`（`sections: 1`），表头五列 `列` / `源端` / `目标端` / `规则` / `建议`，
**6 行**，`empty_suggestion_cells` **为空**——第五列没有一个空格子。六行原文：

| 列 | 源端 | 目标端 | 规则 | 建议 |
|---|---|---|---|---|
| PAYLOAD | CLOB | `<missing>` | 目标表缺列 | 在目标表加列，或把该列从源 SQL 里去掉 |
| V_TEXT | VARCHAR2(200) | VARCHAR(80) | 目标列过窄 | 把目标列放宽到 VARCHAR(200) |
| D_WRONG | DATE | VARCHAR(20) | 类型不兼容 | 把目标列改成 DATETIME(0) |
| N_TOO_WIDE | NUMBER(38,-30) | DECIMAL(65,30) | 超出 MySQL DECIMAL(65,30) | 改源 SQL 或 CAST 收窄值域 |
| N_MISSING | NUMBER | DECIMAL(10,2) | 裸 NUMBER 未声明精度 | 在取列面为该列配 (p,s) |
| N_BARE | NUMBER | DECIMAL(10,2) | 值域校核：3 行超出目标 DECIMAL(10,2) | 调整任务定义和目标 DECIMAL 的 (p,s)，或改源 SQL / CAST 收窄值域 |

表尾一行 `总计 6 项问题`。**不分组、不折叠、不截断**——六行一次报全，肉眼在
`w1-w6-1440.png` 上确认过没有「展开更多」之类的控件。

**W2（1024 与 1440 各一次，整宽 + 第五列不横滚）**：

| 视口 | `.precheck-reports` 段数 | section 盒子 | reports 盒子 | 表格横滚 | 页面横滚 |
|---|---|---|---|---|---|
| 1440 | 1 | x=230, y=297.4, w=**1168**, h=323.1 | x=229, w=**1170** | 0 | 0 |
| 1024 | 1 | x=230, y=356.6, w=**752**, h=357.2 | x=229, w=**754** | 0 | 0 |

两个视口下 section 都**占满 `.precheck-reports` 的整宽**（1168/1170、752/754，差的 2px 是边框），
`.diagnostic-table-wrap` 的 `scrollWidth - clientWidth` **都是 0**，页面级横滚也是 0
——**第五列「建议」全在框内，不需要横滚**。1024 下 `N_TOO_WIDE` 与 `N_BARE` 两行的建议文字
换行成两行、行高自己撑开，没有溢出（`w2-1024.png` 上看得到）。
清单里那句「对照 `shape-failed` 仍是两栏并置」按 #121 订正**已失去对象**（ADR-0036 §5 取消了形状预检），
本轮只看整宽这半句。

**W3（三档标记纯文字不着色）**：取列卡 describe 类型那一列实际渲染为
`NUMBER` / `NUMBER [待配精度]` / `VARCHAR2` / `TIMESTAMP` / `DATE`。
带标记的只有一格，`className` 是 `mono`（与其余四格同一个类），computed style
`color: rgb(23, 35, 61)`（正文色）、`background: rgba(0, 0, 0, 0)`；五格的标签名**都是 `TD`**
——**没有长成标签、没有套 `--warn` / `--crit`**。`w3-w4-column-fetch.png` 上肉眼确认
`[待配精度]` 与前面的 `NUMBER` 是同一串等宽文字，没有色块。

**W4（占位符 + 整份 DDL 照给）**：`.ddl-placeholder` 命中 **1 个**，内容 `DECIMAL(<p>,<s>)`，
落在 `N_BARE` 那行。整份 DDL **11 行**，首行 `-- db-qbs 生成的目标表建表 SQL，请自行执行；产品不会替你建表。`，
末行 `) DEFAULT CHARSET=utf8mb4;`——**整份照给，没有因为有占位符就整份失败**。
截图上 `CREATE TABLE` 区块完整可见，占位符是既有的虚线警示底纹样式，其余行都是普通等宽文本。

**W5（第四态：列表照给、只有 DDL 区块换成整份不给）**：目标表填 `REJECTED`（桩以此切态）后取列，
列清单**照给**四行 `ROW_ID` / `PAYLOAD` / `BF` / `LOAD_DATE`，其中 `PAYLOAD` 显示
`CLOB [不支持]`、`BF` 显示 `BINARY_FLOAT [不支持]`（同样是纯文字）。
取列卡内的 `.ddl-output` **不存在**（`ddl_block_present: false`），位置换成 `.row-size-warning.is-crit`：

> 这些列无法生成目标表定义，整份建表 SQL 不给。
> PAYLOAD（CLOB）：unsupported source type; narrow it in the source SQL or CAST it
> BF（BINARY_FLOAT）：unsupported source type; CAST it to NUMBER(p,s)
> 请按逐列原因修正后重新取列。

**列清单没有跟着一起消失**——`w5-rejected.png` 上四行表格与红框提示同屏。

**W6（值域校核混在同一张五列表里）**：`N_BARE` 那条规则写的是
`值域校核：3 行超出目标 DECIMAL(10,2)`，它**就在 W1 那张五列表的第 6 行**，
与其余五条逐列规则同表、同列、同形态；`.precheck-reports` 下 section 数是 **1**，
**没有另起区块、没有另起标题**。

## 探针这一侧的修正（本轮，都属裁定 Q6 的当票修）

1. **目标表选择器**：v1 把目标表从普通输入框换成 `<input list>` + `<datalist>`，旧写法认的是
   React 不反射的 `value` **属性**，永远选不中。改认静态的 `list` 属性。
2. **取列区块选择器**：`.column-fetch-section` 在 v1 有两处（「目标表建表 SQL」与新增的
   「目标表列参考」共用），裸选歧义。改认 `aria-labelledby="column-fetch-title"`。
3. **「拿建表 SQL」按钮点不到**（本轮新发现）：该按钮在 1440×1200 下落在模态框滚动区的
   **视口之外**（y≈1568）。`page.click()` 会自己滚过去再点，但填完目标表名触发的
   `/api/builder/sql` 会在滚动途中重渲染这一段，**点击落空——一次 `/api/columns` 都不发**，
   `.fetch-ready` 永远等不到（首跑就是卡在这里 30s 超时）。改成先等 SQL 回来、
   把按钮滚进视野，再走 DOM 的 `click()`：不依赖坐标，就不怕重渲染。
4. **`.ddl-output` 在 v1 也有两处**：构建器上方的「生成的 SQL」也用这个类。裸选命中的是它
   （9 行源 SQL），于是 W4 一度读出「DDL 只有 9 行、不以 `utf8mb4;` 收尾」、
   W5 一度读出「DDL 区块还在」——**两条都是探针作用域错，不是界面回退**。
   作用域收进 `.fetch-ready` 后，两条与 2026-08-19T10:39Z 那次记录完全一致。
5. **截图补一步滚动**：W3/W4 与 W5 的截图原先停在模态框的当前滚动位置，DDL 区块落在折叠线以下，
   图上无从看起。改成截图前把 DDL / 告警区块滚进视野。

第 4 条值得单独记一句：**探针读错和界面变坏，在报告里长得一模一样**。本轮之所以没误判成回退，
是因为拿上一份记录（`m3-visual-walkthrough-20260819T103910Z.md`）逐项对了一遍，
再回 `web/src/App.tsx` 核了渲染分支。这套复核方法已写进 ADR-0040 的 #136 增补。
