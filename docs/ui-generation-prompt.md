# UI generation prompt — db-qbs

> Hand this whole file to a UI-generating agent. It is self-contained: it states the product,
> the screens, the visual system, and the rules that must not be broken.

---

## 1. What you are designing

**db-qbs** is an internal, on-premise **offline database import tool**. An operator uses it to
pull a batch of rows out of an **Oracle 11g** database on the *source* network and bulk-load them
into a **MySQL 8.0** table on the *target* network. The two networks are isolated; the only link
is one outbound HTTPS hop from a `source` service to a `sink` service (the "target agent") that
sits next to MySQL and is the *only* way the target database is ever touched.

You are designing the **web UI served by `source`**. There is no mobile app, no marketing site,
no multi-tenant account system. Design a **desktop-first admin console**, ~1280–2560px wide,
usable at 1366px, in **Simplified Chinese** (all visible copy is Chinese; identifiers, SQL,
state codes and error codes stay in English/uppercase).

**Who uses it:** one or two data engineers or DBAs at a customer site. They are technical: they
read SQL, know what a primary key and an upsert are, and will be blamed if the month-end import
silently writes wrong numbers. They are *not* browsing — they come to the screen to do one job
and want to see, at a glance, whether it worked.

**The single most important product value: never let a value be silently corrupted.** The UI's job
is to make every gate, every check, and every ambiguous outcome *visible and legible*, never to
smooth them away with a green checkmark. When something is uncertain, the screen says it is
uncertain rather than guessing.

---

## 2. Domain vocabulary (use these exact concepts; do not invent others)

- **Target Agent** — the `sink` process on the target host. First-class object with a name, an
  address (`http://host:port`), a stable `agent_id`, a version, and a liveness state. Three
  distinct states, never collapsed into two: **在线 (online)**, **不在线 (offline)**, and
  **身份不符 (identity mismatch — a *different* agent answered at this address)**. Probed every
  15s in the background and again immediately before each use.
- **Datasource** — a saved connection. Two kinds: **Oracle（源端）** (connect string, user,
  password) and **MySQL（目标端）** (host, port, database, user, password, **plus a required
  target agent**). Passwords are never returned by the API: the edit form shows
  「已设置 · 留空 = 不改」. Each row shows whether it is 被引用 / 未被引用 by a task.
- **Task (导入任务) / Task Definition** — the structured spec: source table (or hand-written SQL),
  selected columns, column mapping, primary key, `WHERE` clause, target table. **The SQL is
  generated from the spec and is always read-only** — never present it as an editable statement.
- **Custom SQL (自定义 SQL)** — an optional hand-written `SELECT` used *instead of* picking a
  table. It is wrapped as an opaque subquery; it is an input field, not an authority. Cannot be
  combined with a `WHERE` clause or a DBLINK.
- **Run (运行)** — one execution. Starting one takes **no input at all**: click 发起, it runs. No
  dialog, no parameters. One task may not have two runs in flight.
- **Run Stage** — exactly five wire values, always shown in their uppercase spelling *next to*
  the Chinese label: `PREPARING 准备中` → `STREAMING 传输中` → `COMMITTING 提交中`, then the
  terminal `SUCCEEDED` / `FAILED`. The progress rail draws only the first three and ends with
  「→ 终态待定」. **Stop is only permitted in `PREPARING` and `STREAMING`** — the button is greyed
  in the other three rather than failing on click.
- **Mapping Precheck (映射预检)** — a hard per-column gate before a run: source column type vs
  target column type, reported **all columns at once** in a table of 列 / 源端 / 目标端 / 规则 / 建议.
- **Target DDL (建表语句)** — a `CREATE TABLE` the product *generates for a human to run
  themselves*. The product never creates the target table. Copyable, with `<目标表名>` and
  `DECIMAL(<p>,<s>)` rendered as visually distinct placeholders.
- **Staging table (暂存表)** — `<target>__stg_<run_id>`. Rows land there first and are swapped into
  the target inside one transaction after verification.
- **Two identifiers, always displayed together, never substituted**: `run_record_id` (the
  submission the source accepted) and `run_id` (the run the target agent knows about, shaped
  `20260813091530_a3f19c`). `run_id` may be **null** when the precheck rejected the submission.
- **The write is an upsert, and an upsert never deletes.** Wherever a run's outcome is shown, a
  permanent, non-dismissible line of prose states this:
  「按主键 upsert：新增和变更已写入；源端删除的行仍保留在目标表。」 It is styled as neutral
  semantics, **not** as a warning or an error.
- **结局不明 (unknown outcome)** — a first-class third result beside success and failure, for runs
  whose verdict was lost (e.g. the service restarted). It reads:「无法确认目标表是否被修改，请到
  目标库核对。」 Never render it as a failure and never guess.

---

## 3. Screens to design

### 3.0 Shell
Dark sidebar + white top bar + grey content area. Sidebar is `#001529`, 256px, collapsible to
48px (the toggle sits at the far left of the top bar; the collapsed state is remembered).
Product mark + name at the top of the sidebar. **Exactly three navigation items**, in this order:

1. **作业中心** (job centre) — the landing page
2. **数据源** (datasources)
3. **目标端 Agent** (target agents)

Top bar: breadcrumb on the left, and on the right an 「关于」 popover (read-only deployment facts:
listen address, data directory, Oracle client lib, retention days) plus a user menu button showing
the logged-in account with 修改口令 and 退出登录 (the sign-out item separated by a rule and
turning red on hover). There is **no settings screen** — it was demoted into that popover.

### 3.1 Login
The **only full-screen dark page** and the only screen outside the shell. A centred two-column
split on a deep navy field (`#101A5C`) with one large soft radial glow at the upper left: a
left-hand pitch column (large light headline, one paragraph of supporting copy, a small version
string) and a right-hand login card (~380px, thin translucent border, product wordmark, 账号 and
口令 fields with a show/hide eye toggle, a full-width primary submit reading 登 录 with wide
letter-spacing). The error line under the fields occupies a **fixed height even when empty** so the
button never jumps. Single account; no "register", no "forgot password", no "remember me".

### 3.2 作业中心 (Job Centre) — the landing screen
A full-width table of tasks, one row per task, carrying its **latest run** inline. Above it: a
search box (任务名 / 源表 / 目标表), a 源端 filter, a 目标端 filter, a 运行状态 filter, and a 刷新
button; a primary 新建导入 button at the right. Multi-select via checkboxes drives 批量发起
(and a 逐个发起 alternative for large selections).

Columns: 任务名 · 源端 · 源表(或「自定义 SQL」) · 目标端 · 目标表 · 运行状态 · 迁移进度 ·
启动时间 · 运行时长 · 操作.

- **运行状态 is one single-axis chip.** The list gets one dimension only — the multi-axis
  detail belongs on the run screen. Chips: 进行中 (blue), 成功 (green), 失败 (red),
  结局不明 (neutral/amber), 未运行 (grey). The chip links to the run detail.
- **迁移进度** is an inline slim progress bar with a row count; only meaningful while running.
- **操作** holds icon actions: 发起运行, 运行详情, 编辑任务定义, 改名, 复制 cURL, 删除. Keep them
  in a fixed-width right column that stays reachable when the table scrolls horizontally (a soft
  left-pushing shadow on that column's edge).
- Empty state: a single illustrative icon plus 「还没有任务 / 新建第一个 Oracle → MySQL 导入任务。」
- Delete is confirmed in a modal that says 不可撤销.

### 3.3 新建导入 wizard — the centrepiece
Entered from the job centre. It is a **five-panel flow**, and the first panel is different from
the other four.

**Entry gate:** before the wizard opens, a small dialog checks the preconditions (at least one
Oracle datasource, at least one MySQL datasource) and lets the operator pick the source and target
datasource for this task. If a precondition is missing it says 暂时不能新建任务 and offers
前往数据源 / 前往目标端 Agent instead of opening a broken wizard.

**Panel 1 — 选择数据 (full width, no side rail).** Choose the retrieval method first
(取数方式: 按表选择 ⇄ 自定义 SQL, a segmented control), then two **equal, symmetric columns**:

- 源端: the source datasource (fixed label once bound), an optional DBLINK, then either a source
  table picker/tree or the SQL editor, then a 结果列 / 源表 status bar showing what has been
  identified (尚未识别结果列 → n 列).
- 目标端: the target datasource, the 目标表 input (「从下面挑一张，或直接输入表名」) with a table
  list and a 刷新目标表清单 action, then the target's own status bar (尚不存在 / 将新建 / 已通过).

**Geometry is a hard requirement here:** the two columns must stay aligned segment-for-segment,
their two cards' top and bottom edges landing on the same lines, drawn with **one** border layer —
not a border per card nested inside a border per column. Rows that appear only conditionally (the
DBLINK row, the filter row) must not break that alignment; reserve their slot.

**Panels 2–5 — the four wizard steps.** Once past panel 1, the left column collapses into a
**vertical step rail plus a compact summary** of the choices made in panel 1 (source, table/SQL,
target table), and the right side holds the active step:

1. **选列与字段映射** — a table of 同步 / 源列 / 目标列 / 主键 / (drop) with a checkbox per column, an
   editable target column name, a primary-key radio, and a per-row delete. Below it: 不搬的列 listed
   explicitly, and a read-only **构建 SQL** block with syntax highlighting.
2. **过滤与验证** — the `WHERE` clause box (「只写条件本身，不用写 WHERE 关键字」) and a
   **预览前 10 条** data preview table.
3. **目标表检查** — the mapping precheck report; if the table does not exist, the generated
   **建议建表语句** with a 复制建表语句 button and a 完整 CREATE TABLE expansion. This step is
   skipped silently when there is nothing to check.
4. **确认并运行** — a definition list of 任务名 / 取数方式 / 字段映射 / 主键 / 条件 / 不搬的列 /
   目标表检查 / 写入方式(按主键 upsert, with the forward-tense upsert note), and the final action
   pair: **只保存** and **开始导入**.

Footer on every panel: 上一步 / 下一步 (or 保存 / 开始导入 on the last), plus 取消. **When the next
step is blocked, the primary button stays visible and disabled and the reason is shown attached to
the control that is blocking it** (先挑一张源表 / 先写好 SQL / 先选择目标列 / 先选择或输入目标表 /
尚未检查目标表 …). Never a blocked button with no stated reason.

Leaving the wizard always raises a confirmation offering **保留草稿并离开** and
**丢弃草稿并离开** — even when nothing looks worth keeping. A change that clears a value the person
typed by hand also confirms; a change that only clears derived values does not.

The SQL editor needs: syntax highlighting, a 全屏编辑 mode (Esc exits), a soft-wrap toggle whose
tooltip states the trade-off (「打开软换行：长行折行显示，行号会关掉」), and a clause-reflow button
that「只动空白，不改任何一个字符」.

### 3.4 运行详情 (Run detail) — its own addressable screen
Reached from any status chip and from starting a run; it has a real URL (`#runs/<run_record_id>`)
so it survives reload and can be sent to a colleague.

Top: the task name, the two identifiers side by side (运行记录 `run_record_id` / 目标端运行号
`run_id`, the latter possibly 未钉住), and the **phase rail** (`PREPARING → STREAMING → COMMITTING
→ 终态待定`) with the current phase filled.

Live metrics grid while running: 已用时 · 总行数 · 已推行数 · 批次数 · 当前批次序号 · 累计字节 ·
累计批次耗时 · 暂存表 · 最后动静. A 停止 button, greyed with a reason outside the two abortable
stages. A quiet note when polling pauses in a background tab.

On a terminal outcome, show the **second axis** as a solid block — `SWAPPED 已按主键合并写入` or
`DISCARDED 目标表未被触碰` — followed by the permanent upsert note. On failure show
**当次运行证据**: the pinned connection snapshot (源数据源 / 目标数据源 / 目标端 Agent / MySQL 地址 /
Oracle 连接 / 暂存表 / 目标表 / 主键 / 字段映射) under the sentence 「以下连接与参数在发起时固定，
不随后续配置修改而变化。」, the error code tag, the failing 列 and 值 (business data — put it in a
bordered amber container so it reads as sensitive), the mapping-precheck report when that was the
gate, and 核对线索 telling the operator what to check in the target database. When the target may
hold this run's rows, offer 清理本次写入 with the blunt warning 「清理是删除，不是还原。」

Also design a **run drawer**: the same content in a right-hand slide-over for glancing at a run
without leaving the job centre, including 当次执行的源端 SQL (the pinned snapshot) versus
当前任务定义（可能已修改）, and 重跑：按这个任务当前的定义再跑一次.

### 3.5 数据源
A table (名称 · 类型 · 连接 · 用户 · 目标端 Agent · 被引用 · 操作) with 新建数据源 / 编辑 / 删除
modals. The MySQL form's agent selector shows each agent's liveness inline and marks unusable ones
（不可用）. 测试连接 is a button inside the form with an inline result. Empty state:
「先录一条 Oracle 源库与一条 MySQL 目标库，任务才有得选。」

### 3.6 目标端 Agent
A table (名称 · 地址 · 状态 · 版本 · 身份 · 最近可见 · 被引用 · 操作) with 注册目标端 Agent,
探测 (manual probe), 编辑, 删除. The status cell must render all three liveness states distinctly,
with 身份不符 clearly *not* a flavour of offline. 最近可见 shows a relative time or 从未.

---

## 4. Visual system — use these tokens exactly

Light theme only; no dark mode. Layering is **grey background + white cards**; cards have
**no border and no shadow**.

```
--bg #F0F2F5   --bg-outer #F4F7F9   --panel #FFFFFF
--line #F0F0F0   --line-strong #D9D9D9   --line-mid #E8E8E8   --line-top #EEEEEE
--text rgba(0,0,0,.85)   --dim rgba(0,0,0,.65)   --mute rgba(0,0,0,.45)
--brand #3C7EFF   --brand-dim #ECF2FF   --on-solid #FFFFFF
--sider-bg #001529   --sider-text rgba(255,255,255,.75)   --sider-text-active #FFFFFF
--sider-w 256px / 48px collapsed   --sider-item-h 57px   --sider-item-radius 10px
--ok #52C41A / bg #F6FFED / bd #B7EB8F      --ok-fill #CDEBD9 / ink #0B6637
--crit #F5222D / bg #FFF1F0 / bd #FFA39E
--warn #FAAD14 / bg #FFFBE6 / bd #FFE58F
--info #3C7EFF / bg #ECF2FF / bd #ADC6FF     --mute-bg #FAFAFA (table headers)
SQL highlight: keyword #7C3AED, string #0F7B4F, number #B45309, quoted ident #1D6FA5,
               comments and punctuation fall back to --mute / --dim
--scrim rgba(0,0,0,.45)   --shadow-pop 0 6px 24px rgba(0,0,0,.12)
--shadow-edge -6px 0 6px -6px rgba(0,0,0,.12)
Card radius 4px. Icon sizes: 14 inline, 16 table actions, 18 dialog/drawer close, 22 empty-state.
Inline table action target height 28px.
```

Typography: a system sans stack with **CJK fallbacks**, and a separate tabular/monospace stack for
numbers, identifiers, `run_id`s and SQL. Icons: **lucide**, outline, one weight.

Colour rules that are not negotiable:
- The dark sidebar has **its own foreground colours**. Never put `--text` / `--dim` (which are
  black-based) on `#001529`.
- `--brand` means "clickable". It carries **no status meaning**.
- The SQL highlight palette is separate from the semantic four. Never dye a string literal with the
  error red — highlighting says *what kind of token this is*, not whether something is wrong.
- The upsert note and the semantics prose are **not** warnings. Neutral styling.

Content area is **full-bleed on wide screens**, not centred in a max-width column. Long prose blocks
constrain their own measure instead.

---

## 5. Interaction rules that must survive into the design

1. **Keyboard-complete.** The wizard can be walked end to end with the keyboard: focus moves to the
   new step's heading on each transition and the step change is announced; Esc offers to leave
   (and never silently discards); dialogs and the full-screen editor trap focus.
2. **Every disabled control states its reason**, next to the thing that would unblock it.
3. **One failure shape.** Every error, everywhere, is one envelope: a message plus an optional
   attribution of *who the operator must go to next* — their own input, the source database
   (Oracle), the target agent / sink, or the generated DDL. Design one error presentation that
   carries that attribution as a tag, and reuse it on every screen.
4. **Unrecognised values are shown as they arrived, never swallowed** — an unknown stage spelling
   means the two ends are on different versions, which is exactly what you want on screen.
5. **Never invent certainty.** No "probably succeeded". Unknown is its own visual state.
6. Destructive actions (删除任务, 删除数据源, 删除 Agent, 清理本次写入) always confirm, and the
   confirmation says plainly what will be lost.

---

## 6. Deliverable

Produce **self-contained HTML mockups**, one file per screen (or one file with a screen switcher),
using the tokens above as CSS custom properties in a single shared block. Populate every screen
with **realistic Chinese sample data** — task names like 「核心客户主数据日增量」, Oracle tables like
`FIN.T_GL_BALANCE`, MySQL targets like `dw_stage.gl_balance`, run ids like `20260813091530_a3f19c`,
row counts in the hundreds of thousands. Show each screen in its interesting states, not just the
happy one: a running task, a failed task with evidence, an unknown-outcome run, an offline agent,
an identity-mismatched agent, a blocked wizard step, and every empty state.

Do not add features that are not described here — no dashboards, no charts, no scheduling, no
notifications, no user management, no dark mode toggle.
