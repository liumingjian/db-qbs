# 第一版渲染面走查实录 · X1–X20（2026-08-24，一轮 QA 修复）

- **触发**：`v1-visual-walkthrough.md` 触发条件 7（[ADR-0046](../../../adr/0046-qa-round-editor-nav-and-dead-column.md)）——
  自定义 SQL 输入框加了高亮层与格式化按钮、导航项**次序**变了、数据源屏撤了一列。
- **改判三条、新增零条**：X1（导航次序）、X2（撤掉「口令」列）、X20（增补高亮与格式化）。
- **怎么跑的**：`walkthrough/run-x-walkthrough.sh` 两趟（常规 + `X_BULK=1`），
  真跑在用户 mac 上（`rexec`，`lmj-mac-mini-269d`），喂本次提交现构的 `web/dist`
  （`index-BUVQJYEq.css` 38.84 kB / `index-BcvNz0hq.js` 299.76 kB）。两趟都 `exit=0`。
- **桩不是真库**：态是 `v1-mock.py` 编出来的，只答「渲染出来没有」。

原始输出：mac 上的 `/tmp/x-out.json`（两份 JSON，中间隔一行分隔符）。下面是实际观察，不是「通过」。

---

## 一、X1（导航次序）· 三改判

```
order:        ["作业中心", "数据源", "目标端 Agent", "系统设置"]
tags:         ["A", "A", "A", "A"]          ← 四项同一套元素，没有新造样式
classes:      ["nav-item is-active", "nav-item ", "nav-item ", "nav-item "]
first_item:   作业中心
third_item:   目标端 Agent
active_item:  作业中心
sidebar_style:      background rgb(0, 21, 41)  width 256
active_item_style:  bg rgb(60, 126, 255)  color #fff  radius 10px
breadcrumb:   数据导入 / 作业中心
```

**`first_item === active_item` 是这次改判真正买到的东西。** 上一份实录
（`m2-visual-walkthrough-20260824T044212Z.md` 的 V24 一节）逐字记着当时的样子：

```
sidebar_items: 目标端 Agent · 作业中心(is-active) · 数据源 · 系统设置
```

高亮的是第二项，而落地页一直是作业中心——打开应用时，被点亮的那一项和展开的那一屏
**不是同一个**。这处错位在两份 ADR 之间活了一整天，没人报，因为它不报错。

侧栏底色 `rgb(0, 21, 41)` = `--sider-bg`，与上一轮逐字相同；四项仍是同一套 `<a class="nav-item">`。

## 二、X2（数据源列表）· 再改判

```
columns:      ["名称", "类型", "连接", "目标端 Agent", "用户", "被引用", "操作"]
column_count: 7
password_column_present: False
rows[0]:      ["生产核心库\nds-ora-core", "Oracle", "//oracle-core:1521/ORCLPDB", "", "app_reader", "5 个任务", ""]
search_fields: 0    selects_on_screen: 0    toolbars: 0
agent_cells:
  Oracle → ""                             state_tags []
  Oracle → ""                             state_tags []
  MySQL  → "目标端 A"                      state_tags []
  MySQL  → "目标端 B（灾备）不在线"          state_tags ["不在线"]
  MySQL  → "目标端 A"                      state_tags []
```

「口令」不在表头里，八列回到七列。**其余判据逐条仍在**：没有搜索框（0）、没有类型筛选（0）、
没有工具条（0）、没有业务库的连接状态列；「被引用」列在（`5 个任务`）；连接一列 Oracle 显示
`connect_string`；「目标端 Agent」一列 Oracle 行是空串、MySQL 行是名字、不在线那台跟一个
`.state` 标签。

**表单里那个徽标没被误伤**——X4 同一趟量到：

```
password_field_value: ""
password_badge: {text: "已设置 · 留空 = 不改", className: "field-badge is-neutral",
                 color: rgba(0,0,0,.45), background: rgb(250,250,250), border: rgb(217,217,217)}
```

中性色、文字一字未改。撤的是「这条记录有没有口令」，留的是「我这次留空会怎样」。

## 三、X20（自定义 SQL）· 增补高亮与格式化

### 高亮

```
token_classes: [sql-t-comment, sql-t-keyword, sql-t-number, sql-t-punct, sql-t-quoted, sql-t-string]
keyword  "SELECT"            rgb(124, 58, 237)
string   "'vip'"             rgb(15, 123, 79)
quoted   "\"grade\""         rgb(29, 111, 165)     ← 与 string 不同色
number   "100"               rgb(180, 83, 9)
comment  "-- 结果列由桩固定返回"  rgba(0, 0, 0, 0.45)   ← 落回 --mute
text_matches: true           ← 着色层拼回来的字逐字等于输入框里的，没吞、没改
textarea_color: rgba(0, 0, 0, 0)     caret_color: rgba(0, 0, 0, 0.85)
```

**两层同框，逐项相等**（左边 `<pre>`，右边 `textarea`）：

```
font        ui-monospace, "SF Mono", "JetBrains Mono", …   ==  同
size        12px                                          ==  同
lineHeight  18.6px                                        ==  同
padding     9px 10px                                      ==  同
borderWidth 1px                                           ==  同
whiteSpace  pre                                           ==  同
ligatures   none                                          ==  同   ← 两层都关
left/top    259 / 371                                     ==  同
width/height 922 / 170                                    ==  同
```

九项全等。这是这个实现最容易坏的地方——颜色对不对一眼看得出来，光标飘一像素看不出来，
所以它必须是量出来的。

### 格式化

```
button_title: "按子句换行重排，只动空白，不改任何一个字符"
enabled_before: true      disabled_after: true     changed: true
non_whitespace_identical: true
result_columns_after: ["ID", "C_NAME", "LOAD_DATE", "N_AMT"]
```

前：

```
SELECT ID, C_NAME, 'vip' AS "grade", N_AMT -- 结果列由桩固定返回
  FROM APP.T_CUSTOMER@POC_LINK_A
 WHERE N_AMT >= 100 AND STATUS != 9
```

后：

```
SELECT ID,
  C_NAME,
  'vip' AS "grade",
  N_AMT -- 结果列由桩固定返回
FROM APP.T_CUSTOMER@POC_LINK_A
WHERE N_AMT >= 100
  AND STATUS != 9
```

四件事同时成立，缺一条这个按钮就不该存在：

1. **`non_whitespace_identical: true`** —— 去掉全部空白后前后逐字相等。这就是
   `formatSql` 那条不变式的可观察形式（另有 `sql.test.ts` 里 7 组样本守着）。
2. **`>=` 与 `!=` 原样活着**，没被拆开、没被写成 `≥` `≠`；行注释 `-- …` 后面的
   `FROM` 另起一行，没被它注释掉。
3. **`disabled_after: true`** —— 排完就禁用，再点一次不会有「按了没反应」的疑心。
4. **`result_columns_after` 四列一列不少** —— 已读的结果列没被清掉。

### 该模式的其余判据（一字未改，回归）

```
confirm_on_switch:  [{type: confirm, message: "切换取数方式会清空当前的源表 / 结果列、
                      字段映射、主键和过滤条件。确定切换？"}]
confirm_when_blank: []                       ← 空白状态下切换不弹
after_switch:       source_rows 0, mapping_rows 0, textarea_value "", dblink_present false
conditions_card:    present true, subtitle "由你写的 SQL 决定",
                    message "自定义 SQL 模式：过滤与排序请直接写进上面的 SQL。", add_button false
unchecked_column:   N_AMT
```

外层投影（勾掉 `N_AMT` 之后）：

```
SELECT q.ID AS ID,
       q.C_NAME AS C_NAME,
       q.LOAD_DATE AS LOAD_DATE
  FROM (
         SELECT ID,
           C_NAME,
           'vip' AS "grade",
           N_AMT -- 结果列由桩固定返回
         FROM APP.T_CUSTOMER@POC_LINK_A
         WHERE N_AMT >= 100
           AND STATUS != 9
       ) q
```

两处顺带得到了证据：**子查询里是格式化之后的那段**（格式化确实写回了规格，不是只改了显示），
**`N_AMT` 不在外层投影里**（勾选真的连着最终语句）。把它勾回去，预览退回空态
`先读取结果列，再选目标表完成映射与主键。`——两个方向都动过了。
连字 `textarea: none` / `preview: none`。

## 四、其余各条：回归，无改判

| 用例 | 观察 |
|---|---|
| X3 / X4 | 新建对话框打开即禁用保存；删被 5 个任务引用的数据源 → `数据源仍被 5 个任务引用；请先改这些任务的数据源` + 点名五个任务，对话框不关 |
| X13 | 展开 256 / 折叠 48；折叠后 `nav_texts_visible: 0`、`nav_titles` 四项仍在、`icon_center_offset: 0`；`localStorage: "1"`，重载后仍是折叠态 |
| X19 | Agent 屏八列 `名称 · 地址 · 状态 · 身份 · 版本 · 最近可见 · 被引用 · 操作`，判据一字未动（**只是路径变了**：现在从导航第三项进） |
| X11 / X15 | 加量趟：共 31 个、每页 20；批量按钮空闲时禁用、勾两条后全部放行、全选当前页 20/20 |
| `#history` | 重定向到 `#jobs`，`history_section: false`，`active_nav: 作业中心` |

## 五、没有观察到的

- **真库上的行为一律没验**：桩不连 Oracle / MySQL。高亮与格式化都是纯前端，与这一点无关；
  但「这条 SQL 在 Oracle 上跑不跑得通」这一趟答不了，从来也不该由它答。
- **手写词法器在畸形输入上的表现**只由 `sql.test.ts` 的六组样本守着
  （未闭合引号、未闭合括号、空串、纯注释），走查这一趟没有另造畸形输入。
- **格式化后的横向滚动**没量：`wrap="off"` 之后长行会横向滚，两层的 `scrollLeft` 同步
  只在代码里对过，走查没造出一条长到要横滚的 SQL。
