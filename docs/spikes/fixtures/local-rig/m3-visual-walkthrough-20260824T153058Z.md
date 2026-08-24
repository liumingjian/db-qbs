# M3 走查实录 · W1–W6（2026-08-24，一轮 QA 修复）

- **触发：本来不该触发，我还是跑了。** [ADR-0046](../../../adr/0046-qa-round-editor-nav-and-dead-column.md)
  §走查触发 判的是「`.precheck-reports` 与 `DiagnosticTable` 一个字未改，W 系列不触发」——
  那句话是对的。但本票往 `web/src/app.css` 那条**关连字的合并规则**里加了一个选择器
  （`.sql-highlight`），而那条规则的选择器清单里恰好列着 `.precheck-reports code`。
  「这算不算改了 `.precheck-reports`」正是 `CLAUDE.md` 规则 1 说的、**裁定比跑一趟贵**的那种问题，
  所以直接跑。
- **跑完发现的东西比判据本身重要**，见下面第一节。
- **怎么跑的**：`walkthrough/run-w-walkthrough.sh`（`m3-mock.py` + `m3-probe.py`），
  真跑在用户 mac 上（`rexec`，`lmj-mac-mini-269d`），喂本次提交现构的 `web/dist`。

原始输出：mac 上的 `/tmp/w-out.json`。下面是实际观察，不是「通过」。

---

## 〇、先说事故：W1–W6 已经静默失效三天

第一次跑，30 秒超时：

```
playwright._impl._errors.TimeoutError: Page.fill: Timeout 30000ms exceeded.
  - waiting for locator("input[list=\"target-table-options\"]")
```

`f371935`（"Refine task builder table selection"，2026-08-21，另一位作者）把构建器的目标表
从 `<input list="target-table-options">` + `<datalist>` 换成了目标端那棵树上的搜索框。
从那一刻起 `m3-probe.py` 第 74 行选不中任何东西，**整份 W1–W6 一跑就以一个超时收场**。
那一票没有触发 W 系列，中间几票也没有，所以三天里没人发现。

**这与 `47a2fed` 摘掉建表 SQL 区块是同一种事故**（探针自己的注释里记着那一次）：
判据还在，跑它的手断了。`CLAUDE.md` 规则 4 要的正是这个——工具进仓库，
但进了仓库也只保证「下一台机器找得到它」，不保证「它还能跑」。

修了两处，都在 `m3-probe.py`：

1. 目标表改认 `.target-tree-shell .tree-search input`，填完 `blur()`
   （`loadTargetColumns` 挂在 `onBlur` 上，与 X 走查 X6 同一处坑）。
2. `.run-parameter-list` 与 `.builder-key-note` 改成缺席不抛的 `text_or_absent()`。
   原来是 `page.query_selector(...).inner_text()`，缺席即 `AttributeError`，
   **一格没取到就把后面所有判据一起带走**——那是最贵的失败方式。

## 一、W1 / W2 / W6：预检报告（1440 与 1024 两个视口）

两个视口的行内容逐字相同，只有排版尺寸不同：

```
             1440                          1024
sections     1                             1
section_box  289,305  1118 x 367.39        289,371  702 x 455.39
reports_box  288,304  1120 x 369.39        288,370  704 x 457.39
columns      列 · 源端 · 目标端 · 规则 · 建议   同
row_count    6                             6
total_line   总计 6 项问题                   同
table_overflow_x  0                        0
body_overflow_x   0                        0
empty_suggestion_cells  []                 []
```

六行逐条：

| 列 | 源端 | 目标端 | 规则 | 建议 |
|---|---|---|---|---|
| PAYLOAD | CLOB | `<missing>` | 目标表缺列 | 在目标表加列，或把该列从源 SQL 里去掉 |
| V_TEXT | VARCHAR2(200) | VARCHAR(80) | 目标列过窄 | 把目标列放宽到 VARCHAR(200) |
| D_WRONG | DATE | VARCHAR(20) | 类型不兼容 | 把目标列改成 DATETIME(0) |
| N_TOO_WIDE | NUMBER(38,-30) | DECIMAL(65,30) | 超出 MySQL DECIMAL(65,30) | 改源 SQL 或 CAST 收窄值域 |
| N_MISSING | NUMBER | DECIMAL(10,2) | 裸 NUMBER 未声明精度 | 在取列面为该列配 (p,s) |
| N_BARE | NUMBER | DECIMAL(10,2) | 值域校核：3 行超出目标 DECIMAL(10,2) | 调整任务定义和目标 DECIMAL 的 (p,s)，或改源 SQL / CAST 收窄值域 |

**`empty_suggestion_cells: []`**——六条建议一条不空；
**两个视口 `table_overflow_x` / `body_overflow_x` 都是 0**——1024 下没有横向溢出，
表是靠列宽压缩塞进去的，不是靠滚动条。**只有一段报告**（`sections: 1`），
形状预检那一段随 ADR-0036 §5 取消之后没有回来。

连字那条合并规则改完之后，`.precheck-reports` 这一片的渲染**逐字与判据相同**——
这正是我想验的那件事。

## 二、W3 / W4 / W5：对象仍然不存在（判据已判废）

```
W3_W4  object_missing: 构建器里没有「目标表建表 SQL」卡（column-fetch-title）——
       整段在 47a2fed 被摘掉；所有者 2026-08-21 裁定判废（ADR-0043），W3 / W4 已写 N/A
       column_fetch_sections_on_screen: []   fetch_ready_present: false
W5     object_missing: 同 W3 / W4：取列卡不存在，W5 的第四态无从制造；判据已判废
```

与 `20260821T051427Z` 那次的处置逐字一致。

## 三、顺带量到的构建器表面 —— **两处新发现，都不是本票改坏的**

```
datasource_selects: [ds-oracle 「源库（走查）」, ds-mysql 「目标库（走查）」]
condition_rows:     2
sql_is_readonly:    true
key_note:           "主键用于去重，必须至少选一列。"
primary_key_boxes:  0                      ← ①
run_parameters:     {object_missing: ".run-parameter-list"}   ← ②
```

**① `primary_key_boxes: 0` 是探针的陈旧选择器**：主键勾选框早已从 `.builder-columns`
搬到了 `.field-mapping-section`（ADR-0038 §2 之后主键存的是**目标**字段名）。
这一格不属于 W1–W6 任何一条判据，本次**没有跟着修**——修它要顺带确认它该量哪一侧，
不该夹在一轮 QA 修复里顺手做掉。记在这里。

**② `.run-parameter-list` 缺席，这一条是真问题，而且比它看起来严重。** 顺着查下去：

```
桩里存着的任务规格：
  {column: LOAD_DATE, operator: eq, value_type: date,
   parameter: load_date, value_source: "runtime", constant: ""}

界面打开这个任务后，POST /api/builder/sql 送出去的：
  {column: LOAD_DATE, operator: eq, value_type: date,
   parameter: load_date, value_source: "constant", constant: ""}   ← 被改写了

桩对**存着的那份**规格的回话：  run_parameters: [{parameter: load_date, …}]
桩对**界面送来的那份**的回话：  run_parameters: []
```

`web/src/App.tsx` 里两处把它写死：`normalizeTaskSpecForEditor()` 在**打开任务时**
把每一条条件的 `value_source` 强制改成 `"constant"`，`updateCondition()` 在**每次改动时**
再强制一次。也就是说：**一个带运行参数的任务，只要在构建器里打开一次并保存，
那个运行参数就变成了一个空字符串常量**——`WHERE LOAD_DATE = ''`。

- 它来自 `71e09b0`（"feat: refine task query configuration"）——
  **正是那个把自定义 SQL 悄悄塞进来、后来不得不由 ADR-0045 补记的同一个提交**。
- 它与 ADR-0035 §3（发起时逐条列出运行参数并取值）、ADR-0036 §6（运行参数是规格的派生面）、
  ADR-0043 §重跑（按上次运行参数预填）**同时冲突**，而 ADR 里一个字都没记。
- 界面上也确实**没有任何控件**能把一条条件标成「运行时传入」：条件行只有
  `字段 / 比较 / 值 / 值类型` 四栏。

**本票不修它**：它不在这轮 QA 的三条里，改它要动产品形态（得先决定运行参数在条件行里
长什么样），而且它和 ADR-0045 是同一种账——该按同样的规格补记，不该夹在别的票里顺手改掉。
写在这里，是为了它不再是「没人报，因为它不报错」。

## 结论

**W1 / W2 / W6 无回归**，六行报告与两个视口的布局逐条与判据相同。
**W3 / W4 / W5 对象仍不存在**，处置照旧。

真正的产出是两件账：**W 的探针已经断了三天**（已修，跑通），
以及**构建器会把运行参数改写成空常量**（未修，已定位到行）。
