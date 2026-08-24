# 第一版渲染面走查实录 · X1–X20（2026-08-24，ADR-0045 补记 + code review 整改）

- **触发**：`CLAUDE.md` 视觉门禁表第 3 行，外加
  [ADR-0045](../../adr/0045-custom-sql-as-wrapped-subquery.md) §走查触发 新立的第 6 条。
  本轮改了**运行详情抽屉「任务定义」的源表一格**、**构建器的保存禁用条件**、
  **同名接线的触发时机**、**外层投影的引号规则**，以及 `web/src/app.css` 里连字与徽标两组规则。
- **另外两份**：`m2-visual-walkthrough.md`（V1–V25）**不触发**——`docs/design-system/README.md`
  与 `tokens.css` 一个字未改，屏幕清单也没多一条；
  `m3-visual-walkthrough.md`（W1–W6）**不触发**——`.precheck-reports` 的布局与
  `DiagnosticTable` 的列结构都没动。
- **判据版本**：`v1-visual-walkthrough.md`（含 2026-08-24 的 **X18 再改判**与**新增 X20**）。
- **怎么跑的**：`walkthrough/run-x-walkthrough.sh` 两趟。真跑在用户 mac 上
  （`rexec`，`lmj-mac-mini-269d`），喂当次 `npm run build` 的 `web/dist`。
- **退出码**：两趟都 **0**。原始输出 `/tmp/x-report.json`（两段 JSON）、截图 `/tmp/v1-visual/*.png`。

下面是实际观察，不是「通过」。

---

## 〇、先认一笔账：上一份实录里有一句**没有观察支撑**

`20260824T113748Z` 第 92 行写「自定义 SQL 模式下这张卡不再整个消失」。
查下来，入库的 `v1-probe.py` **从头到尾没有点过那个 tab**——全文搜「自定义 SQL」
只有一处，还是作业中心那一列的注释。那句话当时是拿一个**没入库的临时脚本**看截图得出的，
按 `CLAUDE.md` 规则 2（记真实观察）与规则 4（门禁工具要在仓库里）**两条都不合格**。

本轮的 `observe_custom_sql_mode`（X20）是它的接手人，随本次提交入库。
**`window.confirm` 要特别对付**：Playwright 默认自动 dismiss 对话框，什么都不做的话
切模式会静悄悄变成空操作，而实录会显示「切过去了」——正是那种「跑了等于没跑」的门禁。
探针挂了 `page.on("dialog")`，既接受它、又把原话记下来。

---

## 一、X20（新增）：自定义 SQL 模式整半边

### 岔路口长在哪张卡上

| | 实测 |
|---|---|
| `.builder-mode-switch` 的宿主 | `in_header: true`，卡 `class="builder-guide"`，**卡标题 `源表`** |
| 同排的兄弟按钮 | `按表选择` · `自定义 SQL` · `读取表` |

判据要的「长在它真正控制的那张卡的头上」成立——它在**源表**卡的 `<header>` 里，
不在数据源卡的页脚。

### 切换的确认：有东西可丢才弹

先读一次源列（`before_switch.source_rows = 3`），再点「自定义 SQL」：

```
confirm_on_switch = [{"type": "confirm",
  "message": "切换取数方式会清空当前的源表 / 结果列、字段映射、主键和过滤条件。确定切换？"}]
```

切完 `after_switch = {source_rows: 0, mapping_rows: 0, textarea_value: "", dblink_present: false}`
——七样清干净了，DBLINK 一栏在 SQL 模式下**不出现**（路径已经写在 SQL 里）。

**反向也量了**：把 textarea 清空再切回「按表选择」，`confirm_when_blank = []`——
**没有可丢的东西时不弹**。这一半上一轮没人量过。

### 一个概念一个名字

```
card_title = "源表"（切过去后变「自定义 SQL」）
tab_text = ["按表选择", "自定义 SQL"]
textarea_aria_label = "自定义 SQL"
legacy_wording_present = false   ← 「源 SQL」「源端查询方式」在整个 modal 的 innerHTML 里都不存在
```

`aria-label="源端查询方式"` 这个只有读屏听得到的第三个名字，本轮一并改成「取数方式」。

### 过滤条件卡：不消失，说清去向

```
present: true
subtitle: "由你写的 SQL 决定"
message: "自定义 SQL 模式：过滤与排序请直接写进上面的 SQL。"
add_button: false        ← 卡在，但加不了条件
```

### 构建 SQL 预览：**这一条是这次走查的核心**

写进 textarea 的是：

```sql
SELECT *
  FROM APP.T_CUSTOMER@POC_LINK_A
 WHERE N_AMT >= 100 AND STATUS != 9
```

读结果列拿到四列 `ID · C_NAME · LOAD_DATE · N_AMT`。桩的目标表只有前三列，
所以先勾掉 `N_AMT`，再让目标列失焦触发读取——**同名自动接线在这里终于有了对象**
（上一轮桩的规格本来就填满了，这条路径上没东西可填）：

```
mapping_after_autofill = [ID→ID, C_NAME→C_NAME, LOAD_DATE→LOAD_DATE]
mapping_subtitle = "3 列已映射，无需确认"
```

勾一个主键之后预览渲染出来：

```sql
SELECT q.ID AS ID,
       q.C_NAME AS C_NAME,
       q.LOAD_DATE AS LOAD_DATE
  FROM (
         SELECT *
           FROM APP.T_CUSTOMER@POC_LINK_A
          WHERE N_AMT >= 100 AND STATUS != 9
       ) q
```

三件事一次看全：

1. **用户的 SQL 不原样执行**——外层套着投影，内层原文一个字节没动。
   副标题写明这件事：`只读预览——实际执行的是这一段：你写的 SQL 外面套了一层只取勾选列的投影`。
2. **勾掉的 `N_AMT` 真的不在投影里**。这就是 P0 第 3 条「表头全选是空点击」的判据本体。
3. **把 `N_AMT` 勾回去**，预览退回空态 `先读取结果列，再选目标表完成映射与主键。`——
   它又变成「选中但没映射的列」，`specComplete` 掉回 false。两个方向都动，
   才说明这颗勾选框真的连着最终语句。

### 连字

```
textarea: none     preview: none
preview_text 里 `>=` 与 `!=` 按字面显示
```

**这次是在预览真渲染出来之后量的。** 上一版在空态下量，`.ddl-output` 不在场，
取到的是 `null`——那不是「没关连字」，是「压根没量到」。探针里补了注释钉住这个顺序。

---

## 二、X18（再改判）：抽屉「任务定义」的源表一格

上一轮这条判据扫过抽屉的六个面板，却没有量那一格——因为它取的是**失败**任务，
而失败的那条是按表选择的。这一轮桩给自定义 SQL 的任务补了一次运行（`rec-sqlmode`），
判据才有对象。两种形态各开一次抽屉：

| 任务 | 「源表」文本 | `title` |
|---|---|---|
| 结算对账（按表选择） | `APP.T_HOLDING` | **无**（文本已是全文，不挂重复的 tooltip） |
| 客户订单增量（自定义 SQL） | `SELECT * FROM APP.T_HOLDING@POC_LINK_A WHERE STA…` | `SELECT *\n  FROM APP.T_HOLDING@POC_LINK_A\n WHERE STATUS = 1` |

**裸点没了。** 这一格此前对自定义 SQL 的任务直接拼 `owner.table`，两个都是空串，
打出来就是一个孤零零的 `.`——上一轮只修了作业中心，漏了这里。

同一次还量到抽屉的「当次执行的源端 SQL」面板：

```sql
SELECT q.ID AS ID,
       q.C_NAME AS C_NAME,
       q.LOAD_DATE AS LOAD_DATE
  FROM ( SELECT * FROM APP.T_HOLDING@POC_LINK_A WHERE STATUS = 1 ) q
```

历史里钉的是**包裹之后**的完整语句（ADR-0036 §2 + ADR-0045 §5），与规格里现算的那份同构。

---

## 三、加了一条运行之后，取样对象有没有被搅动

桩给 `task-sqlmode` 补运行会多出一个成功态，逐条核过：

| 用例 | 实测 | 判断 |
|---|---|---|
| **X17** | 五个标签仍齐：`成功 rgb(82,196,26)` · `进行中 rgb(60,126,255)` · `失败 rgb(245,34,45)` · `结局不明 rgba(0,0,0,.45)` · `尚未运行`，`radius 0px`、高 22 | 不受影响。`task-never` 仍是唯一的「尚未运行」 |
| **X16** | 三种进度态同屏：`100%`（绿）/ `35%`（蓝）/ `99%`（红）/ `75%`（结局不明）/ `—`（计数失败） | 不受影响 |
| **X8** | 七格仍是**两个子元素**（表名 / 数据源名），`ligatures` 全 `none`，自定义 SQL 那格是徽标 + 截断行 | 不受影响 |

---

## 四、其余用例

| 用例 | 实测 |
|---|---|
| **X1** | 四项 `目标端 Agent · 作业中心 · 数据源 · 系统设置`，侧栏 `rgb(0,21,41)` / 256px |
| **X11** | 加量趟：`共 31 个`、每页 20 行、`‹` 第 1 页禁用；退役的历史屏 `history_nav_items = 0` |
| **X2 / X3 / X4 / X5 / X6 / X7 / X9 / X10 / X12 / X13 / X14 / X15 / X19** | 两趟都跑到，退出码 0，观察与 `20260824T113748Z` 那份逐项一致——本轮没有动这些屏 |

---

## 五、这一轮**没有**观察到的，如实记下

1. **外层投影的引号规则（ADR-0045 §3）在走查里没有对象。** 桩的结果列名全是大写，
   走的是「不加引号」那一支；带引号的小写别名（`AS "id"`）只由 Rust 单测守
   （`a_lowercase_result_column_is_referenced_with_quotes`，已验证摘掉修复即挂）。
   要入渲染面门禁得让桩回一个小写列名，但那会连带改 X6 / X7 的取样对象，本轮不擅自加。
2. **`submitDisabled` 不再被瞬时 SQL 失败锁死这一条，走查没有直接量。**
   造这一态要让 `/api/builder/sql` 间歇性 500，桩目前只按规格内容决定回什么。
   这条由代码本身守着（判据从 `!specComplete || sqlError !== null` 收回 `!specComplete`）。
3. **内层 `ORDER BY` 对外层不绑定**（ADR-0045 §6）——这是 Oracle 的语义，
   渲染面看不出来，也不该由走查来守。
