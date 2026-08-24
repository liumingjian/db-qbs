#!/usr/bin/env python3
"""第一版渲染面走查 X1–X19 的机器观察。

## 已跟进 ADR-0043（2026-08-21）

作业中心前端落地的同一票里把本文件一起改了（`CLAUDE.md` 规则 4：驱动走查的工具跟着走）。
改动逐条：

* `HISTORY_COLUMNS` → `JOB_COLUMNS`（☐ · 任务名 · 源表 · 目标表 · 迁移进度 · 运行状态 ·
  启动时间 · 运行时长 · 操作），位置索引仍只写这一处。
* `observe_nav`（X1）：三项、数据源第二项、深色侧栏。
* `observe_task_screen`（X8）：源表 / 目标表两列各两行。
* `observe_rerun`（X9）：入口从历史行「操作」列改成详情抽屉底部；「任务已删除」一态退役。
* `observe_task_filters`（X10）：「最近运行」列 → 运行状态 / 启动时间 / 运行时长三列。
* `observe_history_filters`（X11）：历史屏筛选与列序退役，只剩分页那一半，形态是
  页码按钮 + 每页条数。
* `observe_row_test`（X12）：行内测连改图标按钮，认 `title` / `aria-label`，不再认按钮文字。
* 新增 `observe_sider_collapse`（X13）、`observe_job_columns`（X14）、`observe_bulk`（X15）、
  `observe_progress`（X16）、`observe_state_tags`（X17）、`observe_drawer`（X18）。
* 2026-08-24（ADR-0044）：`observe_nav`（X1）改判成**四项、目标端 Agent 第一项**，
  `observe_list`（X2）多量一列「目标端 Agent」，新增 `observe_agents`（X19）。

屏的 id 从 `#tasks` 变成 `#jobs`，筛选条从 `.history-filters` 变成 `.filter-card`
（而且它是表格卡的**兄弟**、不在卡内），卡内计数从 `.card-subtitle` 变成 `.table-count`。

按 ADR-0028 §1 的先例：**只观察，不断言**；一行 DOM 断言都不进验收套件。
输出是给人抄进走查记录的实际观察，不是 pass/fail。

## 两种造态源

* **桩后端**（`v1-mock.py`，默认）：态是编出来的，只答「渲染出来没有」。
* **活台架**（`X_RIG=1`，指向 `run-v1-acceptance.sh` 跑完留下的真服务）：
  X3 的测连失败是 Oracle 真回的报错、X4 点名的是真建的那个任务、X5/X6/X7 的列面
  来自真表。**#136 判的是后者**（所有者 2026-08-19 裁定，Q5）——第一版整体验收要答的是
  「这一版交付的东西真能用」，用桩证 X3/X4 等于自己给自己发证。

两种源共用同一套观察代码，差异全收在下面这几个常量里，**选择器一律认标签文字或
`aria-*`，不认位置**——位置写法（`:nth-of-type(3)`）会随 Oracle / MySQL 两套字段集变形。
"""

import json
import os
import sys

from playwright.sync_api import sync_playwright

BASE = f"http://127.0.0.1:{sys.argv[1] if len(sys.argv) > 1 else 18098}"
SHOTS = os.environ.get("X_SHOTS", "/tmp/v1-visual")
RIG = os.environ.get("X_RIG") == "1"

# X3 要新建的那条数据源：桩里是一条连不通的 MySQL，活台架上是**真的 Oracle**——
# 错口令换来的是 ORA-01017，不是桩里那句编好的话。
NEW_DS = json.loads(os.environ["X_NEW_DS"]) if "X_NEW_DS" in os.environ else (
    {
        "name": "V1 走查新建（Oracle）",
        "kind": "oracle",
        "fields": {"连接串": "//127.0.0.1:1521/XE", "用户名": "spike"},
        "bad_password": "definitely-wrong",
        "good_password": "spike123",
    } if RIG else {
        "name": "新库",
        "kind": "mysql",
        "fields": {"主机": "10.0.0.99", "库名": "dw_new", "用户名": "u"},
        "bad_password": "wrong",
        "good_password": "right",
    }
)

# X5 的目标表列参考要一张**真存在、且当前任务没把它的列全映射掉**的表，
# 否则 `tr.is-unmapped` 这一态没有对象。
REFERENCE_TABLE = os.environ.get("X_REFERENCE_TABLE", "V1_C4" if RIG else "HOLDING")


# 作业中心的列序（ADR-0043 §4，裁定 3 逐字）。位置索引只在这一处写，别再散到各处。
JOB_COLUMNS = {
    "check": 0, "task": 1, "source_table": 2, "target_table": 3, "progress": 4,
    "state": 5, "started_at": 6, "elapsed": 7, "action": 8,
}

JOB = "#jobs"


def field_input(page, label):
    """按字段标签取输入框——不认位置。"""
    return page.query_selector(
        f'.modal label.form-field:has(span.field-label:text-is("{label}")) input'
    )


def open_agents(page, width=1440, height=1200):
    page.set_viewport_size({"width": width, "height": height})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector(f"{JOB} tbody tr")
    page.click('nav[aria-label="主导航"] a[href="#agents"]')
    page.wait_for_selector("#agents")


def open_datasources(page, width=1440, height=1200):
    page.set_viewport_size({"width": width, "height": height})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector(f"{JOB} tbody tr")
    page.click('nav[aria-label="主导航"] a[href="#datasources"]')
    page.wait_for_selector("#datasources")


def observe_nav(page):
    """X1（ADR-0044 再改判）：导航**四项**、目标端 Agent 第一项、数据源第三项、侧栏深色。

    2026-08-24 之前判的是「三项、数据源第二项」（ADR-0043），更早判的是「四项、数据源第三项」
    ——现在这四项与更早那次不是同一组。这里连侧栏底色一起量出来，
    「不再是白底左导航」那句话得有个数字兜着。
    """
    page.set_viewport_size({"width": 1440, "height": 1200})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector(f"{JOB} tbody tr")
    items = page.query_selector_all('aside.sidebar nav[aria-label="主导航"] > *')
    return {
        "order": [item.inner_text().strip().replace("\n", " ") for item in items],
        "tags": [item.evaluate("(el) => el.tagName") for item in items],
        "classes": [item.get_attribute("class") for item in items],
        "first_item": items[0].inner_text().strip() if items else None,
        "third_item": items[2].inner_text().strip() if len(items) > 2 else None,
        "sidebar_style": page.eval_on_selector(
            "aside.sidebar",
            "(el) => { const cs = getComputedStyle(el);"
            " return {background: cs.backgroundColor, width: el.getBoundingClientRect().width}; }",
        ),
        "active_item_style": page.eval_on_selector(
            "aside.sidebar .nav-item.is-active",
            "(el) => { const cs = getComputedStyle(el);"
            " return {background: cs.backgroundColor, color: cs.color, radius: cs.borderRadius}; }",
        ),
        "breadcrumb": page.query_selector(".topbar .breadcrumb").inner_text().replace("\n", " "),
    }


def observe_list(page):
    """X2（ADR-0044 改判）：列表八列——多一列「目标端 Agent」；
    仍然没有搜索框 / 类型筛选 / **业务库的**连接状态列。

    那一列 MySQL 行显示 agent 名字、绑的那台不在线时跟一个状态标签，Oracle 行是空的。
    """
    open_datasources(page)
    headers = [h.inner_text() for h in page.query_selector_all("#datasources thead th")]
    rows = [[c.inner_text() for c in r.query_selector_all("td")]
            for r in page.query_selector_all("#datasources tbody tr")]
    page.screenshot(path=f"{SHOTS}/x2-datasource-list.png", full_page=True)
    return {
        "columns": headers,
        "row_count": len(rows),
        "rows": rows,
        "search_fields": len(page.query_selector_all("#datasources .search-field")),
        "selects_on_screen": len(page.query_selector_all("#datasources select")),
        "toolbars": len(page.query_selector_all("#datasources .toolbar")),
        # 「目标端 Agent」那一列逐行取：Oracle 行必须是空串，MySQL 行是名字（不是 id），
        # 绑在不在线的 agent 上那一行还要多出一个状态标签。
        "agent_column_index": headers.index("目标端 Agent") if "目标端 Agent" in headers else None,
        "agent_cells": [
            {
                "kind": row.query_selector_all("td")[1].inner_text(),
                "text": row.query_selector_all("td")[3].inner_text().replace("\n", " "),
                "state_tags": [t.inner_text() for t in row.query_selector_all("td:nth-child(4) .state")],
            }
            for row in page.query_selector_all("#datasources tbody tr")
        ],
    }


def observe_new_dialog(page):
    """X3：填错凭据 → 测连失败 + 保存不放行；改对 → 一行纯文字 + 放行。"""
    open_datasources(page)
    page.click('#datasources .card-header button')
    page.wait_for_selector(".modal")
    field_input(page, "名称").fill(NEW_DS["name"])
    page.select_option(".modal select", NEW_DS["kind"])
    for label, value in NEW_DS["fields"].items():
        field_input(page, label).fill(value)
    password = page.query_selector('.modal input[type="password"]')
    password.fill(NEW_DS["bad_password"])
    fields = page.query_selector_all(".modal .form-field input")
    submit = page.query_selector('.modal button[type="submit"]')
    before = {"submit_disabled": submit.is_disabled(), "field_count": len(fields)}

    page.click('.modal .row-actions button')
    page.wait_for_selector(".modal .form-error", timeout=60000)
    failed_text = page.query_selector(".modal .form-error").inner_text()
    failed = {
        "form_error": failed_text,
        "submit_disabled": submit.is_disabled(),
        "error_code_tags": len(page.query_selector_all(".modal .error-code-tag, .modal .terminal-block")),
        "inline_results": len(page.query_selector_all(".modal .inline-result")),
    }
    page.screenshot(path=f"{SHOTS}/x3-test-failed.png", full_page=True)

    password.fill(NEW_DS["good_password"])
    page.click('.modal .row-actions button')
    page.wait_for_selector(".modal .inline-result", timeout=60000)
    result = page.query_selector(".modal .inline-result")
    style = result.evaluate(
        "(el) => { const cs = getComputedStyle(el);"
        " return {color: cs.color, background: cs.backgroundColor, border: cs.borderWidth,"
        " tag: el.tagName, className: el.className}; }"
    )
    passed = {
        "inline_result": result.inner_text(),
        "inline_result_style": style,
        "submit_disabled": submit.is_disabled(),
        "form_errors": len(page.query_selector_all(".modal .form-error")),
    }
    page.screenshot(path=f"{SHOTS}/x3-test-passed.png", full_page=True)
    page.click('.modal .modal-footer button.is-ghost')
    return {"source": "rig" if RIG else "mock", "datasource": NEW_DS["name"],
            "kind": NEW_DS["kind"], "before_test": before,
            "wrong_password": failed, "right_password": passed}


def observe_rename_and_delete(page):
    """X4：只改名称免测连即可保存；删除被拒且点名任务。

    **认「被引用」列，不认行号**：桩里被引用的恰好是第一行，活台架上未必——
    C1 建的两条里被 C2 的任务引用的是哪一条，取决于建的顺序。
    """
    open_datasources(page)
    rows = page.query_selector_all("#datasources tbody tr")
    referenced_index = None
    reference_cells = []
    for index, row in enumerate(rows):
        cells = [c.inner_text().strip() for c in row.query_selector_all("td")]
        reference_cells.append({"name": cells[0], "referenced": cells[5] if len(cells) > 5 else None})
        if referenced_index is None and len(cells) > 5 and cells[5] != "未被引用":
            referenced_index = index
    if referenced_index is None:
        return {"skipped": "没有一条数据源被任务引用——X4 的对象不存在",
                "rows": reference_cells}
    target_row = f"#datasources tbody tr:nth-child({referenced_index + 1})"
    picked = reference_cells[referenced_index]

    page.click(f'{target_row} button[aria-label="编辑数据源"]')
    page.wait_for_selector(".modal")
    submit = page.query_selector('.modal button[type="submit"]')
    password_badge = page.query_selector(".modal .field-badge")
    badge_style = password_badge.evaluate(
        "(el) => { const cs = getComputedStyle(el);"
        " return {text: el.innerText, color: cs.color, background: cs.backgroundColor,"
        " border: cs.borderColor, className: el.className}; }"
    )
    password_input = page.query_selector('.modal input[type="password"]')
    rename = {
        "picked_row": picked,
        "submit_disabled_on_open": submit.is_disabled(),
        "password_field_value": password_input.input_value(),
        "password_badge": badge_style,
        "kind_field_readonly": field_input(page, "类型") is not None
        and field_input(page, "类型").get_attribute("readonly") is not None,
    }
    field_input(page, "名称").fill(picked["name"] + "（改名）")
    rename["submit_disabled_after_rename"] = submit.is_disabled()
    page.screenshot(path=f"{SHOTS}/x4-rename.png", full_page=True)
    page.click('.modal .modal-footer button.is-ghost')

    page.click(f'{target_row} button[aria-label="删除数据源"]')
    page.wait_for_selector(".delete-copy")
    page.click(".modal-footer button.is-danger")
    page.wait_for_selector(".delete-copy .form-error", timeout=30000)
    delete = {
        "form_error": page.query_selector(".delete-copy .form-error").inner_text(),
        "named_tasks": [li.inner_text() for li in page.query_selector_all(".delete-copy li")],
        "still_open": page.query_selector(".modal") is not None,
    }
    page.screenshot(path=f"{SHOTS}/x4-delete-refused.png", full_page=True)
    page.click('.modal .modal-footer button.is-ghost')
    return {"source": "rig" if RIG else "mock", "rows": reference_cells,
            "rename": rename, "delete": delete}


def open_builder(page, width):
    page.set_viewport_size({"width": width, "height": 1400})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector(f"{JOB} tbody tr")
    page.click('button[aria-label="编辑任务定义"]')
    page.wait_for_selector(".modal .builder-guide")


def observe_builder(page, width):
    """X5 / X6 / X7：目标表下拉、映射两栏、单位标注。"""
    open_builder(page, width)
    # 先读一次源列，好让「没选中的列」这一态有对象（桩里 N_AMT 不在任务规格里）。
    page.click('.modal .builder-guide button:has-text("读取列")')
    page.wait_for_selector('.modal .builder-columns tbody tr:nth-child(4)')

    # X5：**这条的对象换过两次，`.modal input[list]` 已经不再指着目标表。**
    #
    # 原判说「目标表是原生 `<input list>` + `<datalist>`」。P2 把目标表改成了
    # 「树 + 可直接键入的搜索框」（`.target-tree-shell`），它身上根本没有 `list`；
    # 而本分支新加的**源端 DBLINK** 恰好是唯一一个 `input[list]`——旧写法于是
    # 静悄悄地把 DBLINK 当成目标表量了一遍，实录看上去正常、量的却是另一个控件。
    # 两个都摆出来，各按各的形态记。
    target_input = page.query_selector('.modal .target-tree-shell .tree-search input')
    dblink_input = page.query_selector('.modal input[list]')
    dblink_list = (
        page.query_selector(f'.modal datalist#{dblink_input.get_attribute("list")}')
        if dblink_input else None
    )
    x5 = {
        "target_control": {
            "tag": target_input.evaluate("(el) => el.tagName") if target_input else None,
            "has_list_attr": target_input.get_attribute("list") is not None
            if target_input else None,
            "readonly": target_input.get_attribute("readonly") is not None
            if target_input else None,
            "value": target_input.input_value() if target_input else None,
            "placeholder": target_input.get_attribute("placeholder") if target_input else None,
            "tree_nodes": len(page.query_selector_all(".modal .target-tree-shell .table-node")),
        },
        "dblink_combo": {
            "list_attr": dblink_input.get_attribute("list") if dblink_input else None,
            "readonly": dblink_input.get_attribute("readonly") is not None
            if dblink_input else None,
            "placeholder": dblink_input.get_attribute("placeholder") if dblink_input else None,
            "value": dblink_input.input_value() if dblink_input else None,
            "options": [o.get_attribute("value")
                        for o in dblink_list.query_selector_all("option")]
            if dblink_list else None,
            # 自动发现是新能力，可它长得和普通文本框一样。这枚 ▾ 是它唯一的可见线索。
            "chevron": page.query_selector(".modal .combo-input > svg") is not None,
            "badge": page.eval_on_selector(
                ".modal .field-badge.is-inline",
                "(el) => ({text: el.innerText, cls: el.className,"
                " margin_left: getComputedStyle(el).marginLeft,"
                " left: el.getBoundingClientRect().left})",
            ) if page.query_selector(".modal .field-badge.is-inline") else None,
        },
    }

    # 映射面要等**目标列读回来**才渲染（它得知道目标表有哪些列可选）。
    # 触发器是目标表输入框的 `blur`——不失焦就不发 `/api/target/columns`，
    # 于是 `.field-mapping-section` 一直不出现，看上去像「映射两栏没了」。
    # 得**先聚焦再失焦**：`el.blur()` 对一个从没获得过焦点的元素不发事件，
    # React 的 `onBlur` 自然也不触发，映射面就永远等不到。
    target_input.focus()
    target_input.evaluate("(el) => el.blur()")
    page.wait_for_selector(".modal .field-mapping-section tbody tr", timeout=15000)

    # X6：映射两栏。
    #
    # **认 `.field-mapping-section`，不是 `.builder-columns`**：后者是「勾哪几个源列」的
    # 取列表（选择 / 列名 / 字典类型 / 精度长度 / 可空），映射面在 2026-08-21 的
    # `47a2fed` 之后独立成了 `.field-mapping-section`（源列 / 目标字段 / 目标类型 /
    # 约束 / 主键）。旧写法在新 DOM 上取到的是取列表，`target_value` 全是 `None`，
    # 看上去像「目标字段没渲染」——那是选择器过时，不是界面回退。
    source_headers = [h.inner_text() for h in page.query_selector_all(".modal .builder-columns th")]
    mapping_scope = ".modal .field-mapping-section"
    headers = [h.inner_text() for h in page.query_selector_all(f"{mapping_scope} th")]
    rows = []
    for row in page.query_selector_all(f"{mapping_scope} tbody tr"):
        cells = row.query_selector_all("td")
        control = row.query_selector(".cell-input")
        boxes = row.query_selector_all('input[type="checkbox"]')
        control_box = control.bounding_box() if control else None
        row_box = row.bounding_box()
        rows.append({
            "source_column": cells[0].inner_text(),
            # 目标字段这一格现在是**常驻下拉**（原判说的是常驻输入框）：形态变了，
            # 「常驻、不是点开才出现的编辑态」这半边照旧成立。
            "target_control_tag": control.evaluate("(el) => el.tagName") if control else None,
            "target_value": control.input_value() if control else None,
            "target_options": (
                [o.inner_text() for o in control.query_selector_all("option")]
                if control and control.evaluate("(el) => el.tagName") == "SELECT" else None
            ),
            "row_class": row.get_attribute("class"),
            "key_checked": boxes[-1].is_checked() if boxes else None,
            "key_disabled": boxes[-1].is_disabled() if boxes else None,
            "control_height": control_box["height"] if control_box else None,
            "row_height": row_box["height"] if row_box else None,
            "control_fits_row": (
                control_box["height"] <= row_box["height"]
                if control_box and row_box else None
            ),
        })
    x6 = {
        "viewport": width,
        "source_picker_headers": source_headers,
        "headers": headers,
        "rows": rows,
        "table_overflow_x": page.eval_on_selector(
            f"{mapping_scope} .table-wrap",
            "(el) => el.scrollWidth - el.clientWidth",
        ) if page.query_selector(f"{mapping_scope} .table-wrap") else None,
        "dark_theme_media": page.evaluate(
            "() => [...document.styleSheets].flatMap(s => { try { return [...s.cssRules] }"
            " catch { return [] } }).filter(r => (r.conditionText || '').includes('prefers-color-scheme')).length"
        ),
    }

    # 改一个目标字段，看主键跟不跟着走（ADR-0039 增补 1 的渲染面）。
    # 目标字段现在是下拉，改法从 `fill` 换成 `select_option`；判据本身没变。
    rename = None
    for index, row in enumerate(page.query_selector_all(f"{mapping_scope} tbody tr")):
        control = row.query_selector(".cell-input")
        boxes = row.query_selector_all('input[type="checkbox"]')
        if control is None or not boxes or not boxes[-1].is_checked():
            continue
        before = control.input_value()
        others = [o.get_attribute("value")
                  for o in control.query_selector_all("option")
                  if o.get_attribute("value") not in ("", before)]
        if not others:
            break
        control.select_option(others[0])
        page.wait_for_timeout(150)
        after_row = page.query_selector_all(f"{mapping_scope} tbody tr")[index]
        after_boxes = after_row.query_selector_all('input[type="checkbox"]')
        rename = {
            "before": before,
            "after": after_row.query_selector(".cell-input").input_value(),
            "key_still_checked": after_boxes[-1].is_checked() if after_boxes else None,
        }
        after_row.query_selector(".cell-input").select_option(before)
        page.wait_for_timeout(150)
        break
    x6["rename_follows_key"] = rename

    # 映射卡的头：同名列在读取目标列时**自动接上**，这一行说的是「机器做到了多少、
    # 还剩几个要你决定」；两颗操作键（同名填充 / 清空映射）也一并记下。
    x6["mapping_header"] = page.eval_on_selector(
        f"{mapping_scope} > header",
        "(el) => ({sub: el.querySelector('span')?.innerText,"
        " buttons: [...el.querySelectorAll('button')].map(b => ({text: b.innerText,"
        " disabled: b.disabled}))})",
    )
    # 三张表描述同一批列、且嵌了三层（目标表 > 目标表列参考 > 字段映射）的结构已经拆平：
    # 「字段映射」与「目标表结构」现在是**兄弟**，后者默认收起。
    x6["target_structure"] = page.eval_on_selector(
        ".modal .target-structure",
        "(el) => ({open: el.classList.contains('is-open'),"
        " expanded: el.querySelector('.structure-toggle')?.getAttribute('aria-expanded'),"
        " title: el.querySelector('.structure-toggle strong')?.innerText,"
        " sub: el.querySelector('.structure-toggle span')?.innerText,"
        " rows_rendered: el.querySelectorAll('tbody tr').length})",
    ) if page.query_selector(".modal .target-structure") else None
    x6["mapping_nested_in_structure"] = page.evaluate(
        "() => !!document.querySelector('.modal .target-structure .field-mapping-section')"
    )

    # X7：单位标注与静态说明。
    #
    # 「长度」这一栏**不在映射表上**——它在取列表（`.builder-columns`）和目标表结构表上。
    # 原来这里拿的是 `headers`（映射表的五个表头），于是 `length_headers` 恒为空，
    # 这条判据一直落在空集上、看不出真假。改成从真正有这一栏的两张表上取。
    x7 = {
        "length_headers": [h for h in source_headers if "长度" in h] + [
            h.inner_text()
            for h in page.query_selector_all(".modal .target-structure th")
            if "长度" in h.inner_text()
        ],
        "static_note": page.query_selector(".modal .target-side-note").inner_text()
        if page.query_selector(".modal .target-side-note") else None,
        "note_style": page.eval_on_selector(
            ".modal .target-side-note",
            "(el) => { const cs = getComputedStyle(el);"
            " return {color: cs.color, background: cs.backgroundColor, border: cs.borderWidth}; }",
        ) if page.query_selector(".modal .target-side-note") else None,
    }

    # 目标列参考表：填一张真表再失焦。
    target_input.fill(REFERENCE_TABLE)
    target_input.evaluate("(el) => el.blur()")
    # 这张只读表现在**默认收起**（它是查证用的参考，不是每次建任务都要读完的东西），
    # 所以得先展开才有对象。「未映射整行压暗」这条判据本身没变，只是多一次点击。
    page.wait_for_selector(".modal .structure-toggle", timeout=15000)
    if page.query_selector(".modal .target-structure.is-open") is None:
        page.click(".modal .structure-toggle")
    page.wait_for_selector(".modal table.data-grid tr.is-unmapped", timeout=5000)
    reference_rows = []
    for row in page.query_selector_all(
        ".modal section[aria-labelledby='target-structure-title'] tbody tr"
    ):
        cells = [c.inner_text() for c in row.query_selector_all("td")]
        reference_rows.append({
            "cells": cells,
            "class": row.get_attribute("class"),
            "color": row.eval_on_selector("td", "(el) => getComputedStyle(el).color"),
        })
    x5["reference_headers"] = [
        h.inner_text()
        for h in page.query_selector_all(
            ".modal section[aria-labelledby='target-structure-title'] th"
        )
    ]
    x5["reference_rows"] = reference_rows
    footnote = page.query_selector(
        ".modal section[aria-labelledby='target-structure-title'] footer"
    )
    x5["footnote"] = footnote.inner_text() if footnote else None
    # 「不写入任务定义」这半句从页脚搬到了折叠头的副标题上——收起时也读得到，
    # 而原来它只在整张表都渲染出来时才在最底下露一行。
    x5["structure_subtitle"] = page.eval_on_selector(
        ".modal .structure-toggle span", "(el) => el.innerText",
    ) if page.query_selector(".modal .structure-toggle span") else None
    page.screenshot(path=f"{SHOTS}/x5-x7-builder-{width}.png", full_page=True)
    return {"X5": x5, "X6": x6, "X7": x7}


def open_jobs(page, width=1440, height=1200):
    page.set_viewport_size({"width": width, "height": height})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector(f"{JOB} tbody tr")


def cells_of(page, key):
    """按列名取一整列的文本——位置索引只写在 `JOB_COLUMNS` 里。"""
    index = JOB_COLUMNS[key] + 1
    return [
        c.inner_text().replace("\n", " · ").strip()
        for c in page.query_selector_all(f"{JOB} tbody tr td:nth-child({index})")
    ]


def observe_task_screen(page):
    """X8（ADR-0043 改判）：「源 → 目标」单列拆成源表 / 目标表两列，每格两行。

    实质没变：下行给的是**数据源名字**，不是 id（ADR-0039 §8）。
    这里把两格的子元素数一并读出来——「两行一格」这句话靠它兑现。
    """
    open_jobs(page)
    headers = [h.inner_text().strip() for h in page.query_selector_all(f"{JOB} thead th")]
    source_index = JOB_COLUMNS["source_table"] + 1
    target_index = JOB_COLUMNS["target_table"] + 1
    style = page.eval_on_selector(
        f"{JOB} tbody tr td:nth-child({source_index})",
        "(el) => ({children: el.children.length,"
        " lines: [...el.children].map(c => ({cls: c.className, text: c.innerText,"
        " color: getComputedStyle(c).color, font: getComputedStyle(c).fontFamily.split(',')[0]}))})",
    )
    page.screenshot(path=f"{SHOTS}/x8-job-list.png", full_page=True)
    # 每一行的源表格都量一遍，不只是第一行：自定义 SQL 的任务在这一列里是另一种形态
    # （类别徽标 + 截断的 SQL + `title` 里的全文），只看第一行永远看不见它。
    all_source_cells = page.eval_on_selector_all(
        f"{JOB} tbody tr td:nth-child({source_index})",
        "(els) => els.map(el => ({children: el.children.length,"
        " lines: [...el.children].map(c => ({cls: c.className, text: c.innerText})),"
        " title: el.querySelector('.table-cell')?.getAttribute('title'),"
        " ligatures: getComputedStyle(el.querySelector('.table-cell'))"
        ".fontVariantLigatures}))",
    )
    return {
        "columns": headers,
        "source_cells": cells_of(page, "source_table"),
        "target_cells": cells_of(page, "target_table"),
        "source_cell_shape": style,
        "source_cell_shapes": all_source_cells,
        "target_cell_children": page.eval_on_selector(
            f"{JOB} tbody tr td:nth-child({target_index})",
            "(el) => el.children.length",
        ),
    }


def fill_value(field):
    """给一个**没被预填**的参数编一个值——只为把表单填满，不主张值本身有意义。"""
    kind = field.get_attribute("type")
    return {"date": "2026-08-18", "number": "1"}.get(kind, "HZ")


def row_of(page, key, needle):
    """按某一列的文本找行，返回它的 `nth-child` 选择器——不认行号。

    行号在活台架上不可靠（建任务的顺序不定），而这几条判据点名的都是「哪一种态」。
    """
    index = JOB_COLUMNS[key] + 1
    rows = page.query_selector_all(f"{JOB} tbody tr")
    for position, row in enumerate(rows, start=1):
        cell = row.query_selector(f"td:nth-child({index})")
        if cell is not None and needle in cell.inner_text():
            return f"{JOB} tbody tr:nth-child({position})"
    return None


def open_drawer(page, task_name):
    """点某一行的「运行详情」图标开抽屉。开不了（禁用态）时回 None。"""
    selector = row_of(page, "task", task_name)
    if selector is None:
        return None
    button = page.query_selector(f'{selector} button[aria-label^="运行详情"]:not([disabled])')
    if button is None:
        return None
    button.click()
    page.wait_for_selector(".drawer")
    return selector


def observe_rerun(page):
    """X9（前半改判、后半一字不改）：重跑入口搬到**详情抽屉底部**，没有随历史屏消失。

    前半要看的三件事：
      * 失败与结局不明的抽屉底部**有**「重跑」；
      * 进行中与成功的抽屉底部**没有**（不出这颗按钮，也不留空位）；
      * 尚未运行的任务**根本开不了抽屉**——行内「运行详情」是禁用态，
        原因挂在外层 `.row-actions` 的 `title` 与按钮的 `aria-label` 上
        （浏览器不给 `disabled` 控件派发指针事件）。
    「任务已删除」那一态随历史屏退役：列表的一行就是任务本身，任务删了行也就没了。

    后半（预填三规则、并跑提示、结局不明不出提示、发起后是一条新记录）判据一字未改。
    """
    open_jobs(page)
    per_state = {}
    for row in page.query_selector_all(f"{JOB} tbody tr"):
        cells = [c.inner_text().strip() for c in row.query_selector_all("td")]
        name = cells[JOB_COLUMNS["task"]].split("\n")[0]
        state = cells[JOB_COLUMNS["state"]]
        detail = row.query_selector('button[aria-label^="运行详情"]')
        per_state[state] = {
            "task": name,
            "detail_disabled": detail.is_disabled() if detail else None,
            "detail_aria_label": detail.get_attribute("aria-label") if detail else None,
            "wrapper_title": (
                detail.evaluate("(el) => el.closest('.row-actions')?.getAttribute('title')")
                if detail else None
            ),
        }

    # 逐个状态开抽屉，看底部给不给「重跑」。
    drawers = {}
    for state, info in per_state.items():
        if info["detail_disabled"] is not False:
            drawers[state] = {"opened": False, "reason": info["detail_aria_label"]}
            continue
        open_drawer(page, info["task"])
        footer_buttons = [
            {"label": b.inner_text().strip(), "disabled": b.is_disabled()}
            for b in page.query_selector_all(".drawer-footer button")
        ]
        drawers[state] = {
            "opened": True,
            "task": info["task"],
            "title": page.query_selector(".drawer-header h2").inner_text(),
            "run_record_id": page.query_selector(".drawer-header .sub").inner_text(),
            "footer_buttons": footer_buttons,
            "has_rerun": any("重跑" in b["label"] for b in footer_buttons),
        }
        page.screenshot(path=f"{SHOTS}/x9-drawer-{state}.png", full_page=True)
        page.click('.drawer-footer button.is-ghost')
        page.wait_for_timeout(200)

    failed_task = per_state.get("失败", {}).get("task")
    if failed_task is None:
        return {"rows": per_state, "drawers": drawers,
                "skipped": "这一屏没有失败的任务——X9 后半的对象不存在"}

    # 后半：点抽屉底部的「重跑」，开出来的必须是**既有的**发起对话框。
    open_drawer(page, failed_task)
    page.click('.drawer-footer button.is-primary')
    page.wait_for_selector(".modal")
    dialog = {
        "source_task": failed_task,
        "title": page.query_selector(".modal h2").inner_text(),
        "context": page.query_selector(".modal .modal-context").inner_text(),
        "submit_label": page.query_selector('.modal button[type="submit"]').inner_text(),
        "drawer_closed": page.query_selector(".drawer") is None,
        "prefilled": [
            {
                "parameter": label.query_selector(".field-label").inner_text().split("\n")[0],
                "value": label.query_selector("input").input_value(),
                "editable": not label.query_selector("input").is_disabled(),
            }
            for label in page.query_selector_all(".modal .run-params label.form-field")
        ],
    }
    page.screenshot(path=f"{SHOTS}/x9-prefilled-dialog.png", full_page=True)

    edited = None
    for label in page.query_selector_all(".modal .run-params label.form-field"):
        field = label.query_selector("input")
        if field.input_value() == "":
            field.fill(fill_value(field))
            edited = label.query_selector(".field-label").inner_text().split("\n")[0]
    dialog["edited_empty_field"] = edited
    dialog["values_after_edit"] = [
        field.input_value()
        for field in page.query_selector_all(".modal .run-params input")
    ]
    try:
        page.wait_for_selector(".modal .stale-run-hint", timeout=5000)
        dialog["stale_run_hint"] = page.query_selector(".modal .stale-run-hint").inner_text()
    except Exception:
        dialog["stale_run_hint"] = None
    page.screenshot(path=f"{SHOTS}/x9-concurrent-hint.png", full_page=True)

    page.click('.modal button[type="submit"]')
    try:
        page.wait_for_selector(".modal .form-error", timeout=8000)
        dialog["submit_result"] = {"rejected": page.query_selector(".modal .form-error").inner_text()}
    except Exception:
        dialog["submit_result"] = {"accepted": True}

    # 结局不明那条单独再走一遍：它的 `outcome` 也是 null，并跑提示如果按 `outcome` 认
    # 「进行中」，就会指着你刚点的这条早已死掉的记录说它还在跑（#150 审查所见）。
    # 这里要看到的是**没有**提示条。
    unknown = None
    unknown_task = per_state.get("结局不明", {}).get("task")
    if unknown_task is not None:
        open_jobs(page)
        open_drawer(page, unknown_task)
        page.click('.drawer-footer button.is-primary')
        page.wait_for_selector(".modal")
        for label in page.query_selector_all(".modal .run-params label.form-field"):
            field = label.query_selector("input")
            if field.input_value() == "":
                field.fill(fill_value(field))
        page.wait_for_timeout(1500)
        hint = page.query_selector(".modal .stale-run-hint")
        unknown = {
            "task": unknown_task,
            "values": [f.input_value()
                       for f in page.query_selector_all(".modal .run-params input")],
            "stale_run_hint": None if hint is None else hint.inner_text(),
        }
        page.screenshot(path=f"{SHOTS}/x9-unknown-rerun.png", full_page=True)
        page.click('.modal .modal-footer button.is-ghost')

    open_jobs(page)
    return {
        "rows": per_state,
        "drawers": drawers,
        "dialog": dialog,
        "unknown_row_rerun": unknown,
        "states_after": cells_of(page, "state"),
    }


def observe_pagination(page):
    """分页条的实际观察。**总数不超过一页时组件自己不渲染**，那时回 `None`——
    这本身就是一条要看的事实，不是「没观察到」。

    形态自 ADR-0043 起是 `共 N 条` + 页码按钮（当前页填主色）+ 上一页 / 下一页 +
    每页条数下拉，不再是「第 x / y 页」那对箭头。
    """
    footer = page.query_selector(f"{JOB} .list-pagination")
    if footer is None:
        return None
    size = footer.query_selector(".page-size")
    return {
        "total_text": footer.query_selector(".pagination-total").inner_text(),
        "page_buttons": [
            {
                "label": b.inner_text().strip(),
                "aria_label": b.get_attribute("aria-label"),
                "disabled": b.is_disabled(),
                "active": "is-active" in (b.get_attribute("class") or ""),
                "background": b.evaluate("(el) => getComputedStyle(el).backgroundColor"),
            }
            for b in footer.query_selector_all("button")
        ],
        "page_size_value": size.input_value() if size else None,
        "page_size_options": (
            [o.inner_text() for o in size.query_selector_all("option")] if size else None
        ),
    }


def filter_strip(page):
    """筛选条上摆了哪几项、按钮是哪几个——认标签文字，不认位置。

    自 ADR-0043 起它是表格卡的**兄弟**（`.filter-card`），不再是卡内的一条。
    """
    return {
        "strips_on_screen": len(page.query_selector_all(".filter-card")),
        "fields": [
            label.query_selector("span").inner_text()
            for label in page.query_selector_all(".filter-card label.filter-field")
        ],
        "buttons": [
            b.inner_text().strip()
            for b in page.query_selector_all(".filter-card button")
            if b.inner_text().strip() != ""
        ],
    }


def observe_task_filters(page):
    """X10（改判）：作业中心的筛选条、运行状态 / 启动时间 / 运行时长三列与客户端分页。

    改判的是原判那一列「最近运行」的「两行纯文字、不着色、不出彩色标签」——两屏合并后
    它就是列表本身，拆成三列，状态是实心方角标签（形态判据在 X17）。
    「读取失败」一态随之退役：运行数据与任务清单现在是同一次读取。
    筛选条本身**照跑**：四项 + 查询 / 重置，改下拉不重筛。
    """
    open_jobs(page)
    before = {
        "strip": filter_strip(page),
        "count": page.query_selector(f"{JOB} .table-count").inner_text(),
        "row_count": len(page.query_selector_all(f"{JOB} tbody tr")),
        "state_cells": cells_of(page, "state"),
        "started_at_cells": cells_of(page, "started_at"),
        "elapsed_cells": cells_of(page, "elapsed"),
        "delay_note": page.eval_on_selector(
            f"{JOB} thead th:nth-child({JOB_COLUMNS['state'] + 1})",
            "(el) => el.innerText",
        ),
        "pagination": observe_pagination(page),
    }
    page.screenshot(path=f"{SHOTS}/x10-job-filters.png", full_page=True)

    # 改下拉**不**重筛（查询是显式的）：先记下改完还没点查询时的行数。
    status = page.query_selector(
        '.filter-card label.filter-field:has(span:text-is("运行状态")) select')
    status.select_option(label="结局不明")
    page.wait_for_timeout(300)
    untouched = len(page.query_selector_all(f"{JOB} tbody tr"))

    # 两次查询：一次筛得出东西，一次一条都筛不出——后者要看的是「没有匹配的任务」
    # 这句自陈**出来了没有**，空表格加一个孤零零的分页条不算回答。
    # 三次查询：两次筛得出东西，一次一条都筛不出——最后那次要看的是「没有匹配的任务」
    # 这句自陈**出来了没有**，空表格加一个孤零零的分页条不算回答。
    keyword = page.query_selector(
        '.filter-card label.filter-field:has(span:text-is("任务名")) input')
    queried = {}
    for label in ("结局不明", "尚未运行", "筛不出任何东西"):
        if label == "筛不出任何东西":
            status.select_option(value="")
            keyword.fill("这个关键词一条也匹配不上")
        else:
            keyword.fill("")
            status.select_option(label=label)
        page.click('.filter-card button.is-primary')
        page.wait_for_timeout(300)
        queried[label] = {
            "count": page.query_selector(f"{JOB} .table-count").inner_text(),
            "row_count": len(page.query_selector_all(f"{JOB} tbody tr")),
            "state_cells": cells_of(page, "state"),
            "no_results": (page.query_selector(f"{JOB} .no-results").inner_text()
                           if page.query_selector(f"{JOB} .no-results") else None),
            "pagination": observe_pagination(page),
        }
        page.screenshot(path=f"{SHOTS}/x10-job-filtered-{label}.png", full_page=True)

    page.click('.filter-card button.is-ghost')
    page.wait_for_timeout(300)
    reset = {
        "count": page.query_selector(f"{JOB} .table-count").inner_text(),
        "row_count": len(page.query_selector_all(f"{JOB} tbody tr")),
        "status_select_value": status.input_value(),
        "keyword_value": keyword.input_value(),
    }
    return {
        "before": before,
        "row_count_after_select_before_query": untouched,
        "after_query": queried,
        "after_reset": reset,
    }


def observe_history_filters(page):
    """X11：**前半 N/A**（运行历史屏的筛选条与列序整屏取消），只剩分页那一半。

    分页仍是客户端分页且不装成服务端分页；形态改成页码按钮 + 每页条数。
    这一条要在**加量过的**桩上跑（`X_BULK=1`），否则分页条本来就不该出现。
    """
    open_jobs(page)
    before = {
        "retired_history_screen": {
            "history_nav_items": len(page.query_selector_all('nav[aria-label="主导航"] a[href="#history"]')),
            "history_section_on_page": page.query_selector("#history") is not None,
        },
        "row_count": len(page.query_selector_all(f"{JOB} tbody tr")),
        "count": page.query_selector(f"{JOB} .table-count").inner_text(),
        "pagination": observe_pagination(page),
    }
    page.screenshot(path=f"{SHOTS}/x11-pagination.png", full_page=True)

    turned = None
    footer = page.query_selector(f"{JOB} .list-pagination")
    if footer is not None:
        first_before = cells_of(page, "task")[0]
        page.click(f'{JOB} .list-pagination button[aria-label="第 2 页"]')
        page.wait_for_timeout(400)
        turned = {
            "first_cell_before": first_before,
            "first_cell_after": cells_of(page, "task")[0],
            "row_count": len(page.query_selector_all(f"{JOB} tbody tr")),
            "pagination": observe_pagination(page),
        }
        page.screenshot(path=f"{SHOTS}/x11-page2.png", full_page=True)

    resized = None
    if page.query_selector(f"{JOB} .list-pagination .page-size") is not None:
        # 每次重新取这个 `select`：换完条数 React 会把它连同整条分页重建，
        # 旧句柄当场脱离 DOM（第一次跑就是被这个绊住的）。
        page.select_option(f"{JOB} .list-pagination .page-size", label="50 / 页")
        page.wait_for_timeout(400)
        resized = {
            "row_count": len(page.query_selector_all(f"{JOB} tbody tr")),
            "pagination": observe_pagination(page),
            "first_cell": cells_of(page, "task")[0],
        }
        page.screenshot(path=f"{SHOTS}/x11-page-size-50.png", full_page=True)
        # 换回 20：换过条数之后分页条**必须还在**，否则就再也换不回来了。
        # 这一格本身就是判据——`strip_survived` 为 False 就是一条要如实记下的观察。
        resized["strip_survived"] = (
            page.query_selector(f"{JOB} .list-pagination .page-size") is not None
        )
        if resized["strip_survived"]:
            page.select_option(f"{JOB} .list-pagination .page-size", label="20 / 页")
            page.wait_for_timeout(300)
    return {"before": before, "after_next_page": turned, "after_page_size": resized}


def observe_row_test(page):
    """X12（改判）：数据源行内「测试连接」改**图标按钮**，语义靠 `title` / `aria-label` 认。

    其余判据一字未改：只锁自己那一行、结果落在这一行、成功一行纯文字、
    失败原样回显驱动报错不出错误码标签、这一屏仍然没有搜索框 / 筛选条 / 连接状态列。
    """
    open_datasources(page)
    rows = page.query_selector_all("#datasources tbody tr")
    buttons = [
        r.query_selector('button[aria-label="测试连接"], button[aria-label="正在连接"]')
        for r in rows
    ]
    before = {
        "row_count": len(rows),
        "buttons_present": sum(1 for b in buttons if b is not None),
        "button_is_icon_only": [
            {"text": b.inner_text().strip(), "title": b.get_attribute("title"),
             "aria_label": b.get_attribute("aria-label"),
             "svg_children": b.evaluate("(el) => el.querySelectorAll('svg').length")}
            for b in buttons if b is not None
        ][:1],
        "results_present": len(page.query_selector_all("#datasources .row-test-result")),
        "search_fields": len(page.query_selector_all("#datasources .search-field")),
        "filter_strips": len(page.query_selector_all(".filter-card")),
        "selects_on_screen": len(page.query_selector_all("#datasources select")),
    }

    buttons[0].click()
    # 只锁自己那一行：点下去的**当场**采样，一毫秒都不等。
    # 桩答得比采样还快时会采到已经落定的态——那不是缺陷，是桩太快；
    # `already_settled` 把这一点如实标出来，别把采空读成「没实现」。
    while_testing = {
        "clicked_row_aria_label": buttons[0].get_attribute("aria-label"),
        "clicked_row_title": buttons[0].get_attribute("title"),
        "clicked_row_disabled": buttons[0].is_disabled(),
        "other_rows_disabled": [b.is_disabled() for b in buttons[1:] if b is not None],
        "already_settled": len(page.query_selector_all("#datasources .row-test-result")) > 0,
    }
    page.wait_for_selector("#datasources tbody tr .row-test-result", timeout=20000)
    page.wait_for_timeout(200)
    first_cell = rows[0].query_selector(".row-test-result")
    settled = {
        "row_name": rows[0].query_selector(".task-name").inner_text(),
        "result_text": first_cell.inner_text(),
        "result_classes": first_cell.get_attribute("class"),
        "aria_role": first_cell.get_attribute("role"),
        "error_code_tags_on_screen": len(page.query_selector_all("#datasources .error-code")),
        "rows_with_result": len(page.query_selector_all("#datasources .row-test-result")),
        "button_back_to_idle": buttons[0].get_attribute("aria-label"),
    }
    page.screenshot(path=f"{SHOTS}/x12-datasource-row-test.png", full_page=True)
    return {"before": before, "while_testing": while_testing, "settled": settled}


def observe_sider_collapse(page):
    """X13（新增）：侧栏 256 ⇄ 48、图标居中、`localStorage` 记住折叠态。

    「图标居中」这条量的是**选中项那个圆角块**：参照物折叠后没改 `padding-left`，
    蓝块被 48px 切掉大半只剩一条竖边，那是它的渲染瑕疵，我们明确不照抄
    （ADR-0043 文末自决 2）。所以这里读的是块的左右边距差与图标的水平中心偏移。
    """
    open_jobs(page)

    def snapshot():
        return {
            "sidebar_width": page.eval_on_selector(
                "aside.sidebar", "(el) => el.getBoundingClientRect().width"),
            "content_left": page.eval_on_selector(
                "main.main-column", "(el) => el.getBoundingClientRect().left"),
            "toggle_title": page.query_selector(".fold-toggle").get_attribute("title"),
            "toggle_aria_expanded": page.query_selector(".fold-toggle").get_attribute("aria-expanded"),
            "nav_texts_visible": page.evaluate(
                "() => [...document.querySelectorAll('.nav-item .nav-text')]"
                ".filter(el => getComputedStyle(el).display !== 'none').length"),
            "nav_titles": [
                el.get_attribute("title")
                for el in page.query_selector_all(".nav-item")
            ],
            "active_block": page.eval_on_selector(
                ".nav-item.is-active",
                "(el) => { const r = el.getBoundingClientRect();"
                " const s = el.querySelector('svg').getBoundingClientRect();"
                " return {left: r.left, width: r.width,"
                "  icon_center_offset: (s.left + s.width / 2) - (r.left + r.width / 2),"
                "  padding_left: getComputedStyle(el).paddingLeft,"
                "  padding_right: getComputedStyle(el).paddingRight}; }"),
        }

    expanded = snapshot()
    page.click(".fold-toggle")
    page.wait_for_timeout(250)
    collapsed = snapshot()
    page.screenshot(path=f"{SHOTS}/x13-sider-collapsed.png", full_page=True)

    stored = page.evaluate("() => window.localStorage.getItem('db-qbs.sider-collapsed')")
    page.reload(wait_until="networkidle")
    page.wait_for_selector(f"{JOB} tbody tr")
    after_reload = snapshot()

    page.click(".fold-toggle")
    page.wait_for_timeout(250)
    expanded_again = snapshot()
    page.screenshot(path=f"{SHOTS}/x13-sider-expanded.png", full_page=True)
    return {
        "expanded": expanded,
        "collapsed": collapsed,
        "local_storage": stored,
        "after_reload": after_reload,
        "expanded_again": expanded_again,
    }


def observe_job_columns(page):
    """X14（新增）：列序逐字、两行一格、1024 下横滚且操作列吸附右侧、行内五图标分三组。"""
    open_jobs(page, width=1024)
    headers = [h.inner_text().strip().split("\n")[0]
               for h in page.query_selector_all(f"{JOB} thead th")]
    overflow = page.eval_on_selector(
        f"{JOB} .table-wrap", "(el) => el.scrollWidth - el.clientWidth")
    action_style = page.eval_on_selector(
        f"{JOB} tbody tr td.action-column",
        "(el) => { const cs = getComputedStyle(el);"
        " return {position: cs.position, right: cs.right, shadow: cs.boxShadow}; }")
    # 横滚到最右之前 / 之后各量一次操作列的位置：吸附的话它基本不动。
    before_scroll = page.eval_on_selector(
        f"{JOB} tbody tr td.action-column", "(el) => el.getBoundingClientRect().right")
    page.eval_on_selector(f"{JOB} .table-wrap", "(el) => { el.scrollLeft = el.scrollWidth; }")
    page.wait_for_timeout(200)
    after_scroll = page.eval_on_selector(
        f"{JOB} tbody tr td.action-column", "(el) => el.getBoundingClientRect().right")
    actions = page.eval_on_selector(
        f"{JOB} tbody tr td.action-column .row-actions",
        "(el) => [...el.children].map(c => ({tag: c.tagName, cls: c.className,"
        " label: c.getAttribute('aria-label'), title: c.getAttribute('title'),"
        " color: getComputedStyle(c).color}))")
    page.screenshot(path=f"{SHOTS}/x14-job-columns-1024.png", full_page=True)
    return {
        "columns": headers,
        "column_count": len(headers),
        # 主键 / 条件 / 错误码 / 目标表效果一个都不在这张表上——它们在抽屉里。
        "absent_columns": [
            name for name in ("主键", "条件", "错误码", "目标表效果", "结局")
            if name in headers
        ],
        "table_overflow_x": overflow,
        "action_column_style": action_style,
        "action_right_before_scroll": before_scroll,
        "action_right_after_scroll": after_scroll,
        "row_actions": actions,
        "task_cell_lines": page.eval_on_selector(
            f"{JOB} tbody tr td:nth-child({JOB_COLUMNS['task'] + 1})",
            "(el) => [...el.children].map(c => ({cls: c.className, text: c.innerText,"
            " color: getComputedStyle(c).color}))"),
    }


def observe_bulk(page):
    """X15（新增）：勾选与两个批量按钮的禁用态、表头全选**只全选当前页**、确认框列全名字。

    跨页那一半要在加量过的桩上跑（`X_BULK=1`）；不加量时列表只有一页，
    「翻到第 2 页勾选不跟着跑」这一态没有对象——那时如实回 `None`，不是没观察到。
    """
    open_jobs(page)

    def bulk_buttons():
        return {
            b.inner_text().strip(): b.is_disabled()
            for b in page.query_selector_all(f"{JOB} .table-toolbar button")
        }

    idle = bulk_buttons()
    boxes = page.query_selector_all(f"{JOB} tbody .check-column input")
    boxes[0].check()
    if len(boxes) > 1:
        boxes[1].check()
    page.wait_for_timeout(150)
    two_checked = {
        "buttons": bulk_buttons(),
        "checked_count": len(page.query_selector_all(f"{JOB} tbody .check-column input:checked")),
    }
    page.screenshot(path=f"{SHOTS}/x15-two-checked.png", full_page=True)

    header_box = page.query_selector(f"{JOB} thead .check-column input")
    header_box.check()
    page.wait_for_timeout(150)
    page_rows = len(page.query_selector_all(f"{JOB} tbody tr"))
    all_on_page = {
        "header_title": header_box.get_attribute("title"),
        "rows_on_page": page_rows,
        "checked_on_page": len(page.query_selector_all(f"{JOB} tbody .check-column input:checked")),
    }

    # 翻到第 2 页：表头全选只管当前页，第 2 页应当一个都没勾上。
    cross_page = None
    if page.query_selector(f'{JOB} .list-pagination button[aria-label="第 2 页"]') is not None:
        page.click(f'{JOB} .list-pagination button[aria-label="第 2 页"]')
        page.wait_for_timeout(300)
        cross_page = {
            "rows_on_page_2": len(page.query_selector_all(f"{JOB} tbody tr")),
            "checked_on_page_2": len(
                page.query_selector_all(f"{JOB} tbody .check-column input:checked")),
            "header_checked_on_page_2": page.query_selector(
                f"{JOB} thead .check-column input").is_checked(),
        }
        page.screenshot(path=f"{SHOTS}/x15-page2-not-selected.png", full_page=True)
        page.click(f'{JOB} .list-pagination button[aria-label="第 1 页"]')
        page.wait_for_timeout(300)

    # 批量删除的确认框：名字要逐条列全，不是「确定删除 N 个任务？」
    page.click(f'{JOB} .table-toolbar button.is-danger')
    page.wait_for_selector(".modal .delete-copy")
    confirm = {
        "title": page.query_selector(".modal h2").inner_text(),
        "copy": page.query_selector(".modal .delete-copy p").inner_text(),
        "named_tasks": [li.inner_text() for li in page.query_selector_all(".modal .delete-copy li")],
    }
    page.screenshot(path=f"{SHOTS}/x15-bulk-delete-confirm.png", full_page=True)
    page.click(".modal .modal-footer button.is-ghost")
    page.wait_for_timeout(200)

    # 批量发起：串行跑完出一行汇总，**不是只报最后一条**。
    page.click(f'{JOB} .table-toolbar button:text-is("批量发起")')
    page.wait_for_selector(".bulk-summary", timeout=30000)
    summary = page.query_selector(".bulk-summary").inner_text()
    page.screenshot(path=f"{SHOTS}/x15-bulk-start-summary.png", full_page=True)
    return {
        "idle_buttons": idle,
        "after_two_checked": two_checked,
        "after_select_page": all_on_page,
        "cross_page": cross_page,
        "delete_confirm": confirm,
        "bulk_start_summary": summary,
    }


def observe_progress(page):
    """X16（新增）：迁移进度只有一个整数百分比 + 细进度条，三种空态互不混淆。

    要看的四格（桩里的分母是故意摆的）：
      * 99.983% 那条**必须显示 99%**——向下取整，四舍五入成 100% 等于拿显示撒谎；
      * 跑完的是 100%；
      * 尚未运行是 `—`（不是 0%）；
      * 开跑前计数失败是 `—` 且 `title` 自陈「未取到总行数」，但那次运行照常跑完。
    """
    open_jobs(page)
    index = JOB_COLUMNS["progress"] + 1
    rows = []
    for row in page.query_selector_all(f"{JOB} tbody tr"):
        cells = row.query_selector_all("td")
        cell = cells[JOB_COLUMNS["progress"]]
        fill = cell.query_selector(".progress-fill")
        rows.append({
            "task": cells[JOB_COLUMNS["task"]].inner_text().split("\n")[0],
            "state": cells[JOB_COLUMNS["state"]].inner_text().strip(),
            "text": cell.inner_text().strip(),
            "title": (cell.query_selector("[title]").get_attribute("title")
                      if cell.query_selector("[title]") else None),
            "has_bar": fill is not None,
            "bar_width": (fill.evaluate("(el) => el.style.width") if fill else None),
            "bar_color": (fill.evaluate("(el) => getComputedStyle(el).backgroundColor")
                          if fill else None),
        })
    page.screenshot(path=f"{SHOTS}/x16-progress.png", full_page=True)
    return {
        "column_header": page.eval_on_selector(
            f"{JOB} thead th:nth-child({index})", "(el) => el.innerText"),
        "cells": rows,
        # 不带小数、不附行数：整列里出现小数点或「行」字都是回退。
        "cells_with_decimal": [r["text"] for r in rows if "." in r["text"]],
        "cells_with_row_count": [r["text"] for r in rows if "行" in r["text"]],
    }


def observe_state_tags(page):
    """X17（新增）：五个状态词**都是同一种实心方角标签**——齐是对的。

    这一列是一维索引，不是轴二（ADR-0043 §4）；V9 那条「齐了就是假的」已反转退役。
    这一列上**不出错误码标签、不出终态块**——三轴在抽屉里（X18）。
    """
    open_jobs(page)
    index = JOB_COLUMNS["state"] + 1
    tags = page.eval_on_selector_all(
        f"{JOB} tbody td:nth-child({index}) .state",
        "(els) => els.map(el => { const cs = getComputedStyle(el);"
        " return {text: el.innerText, cls: el.className, background: cs.backgroundColor,"
        "  color: cs.color, radius: cs.borderRadius, border: cs.borderWidth + ' ' + cs.borderColor,"
        "  height: el.getBoundingClientRect().height}; })")
    page.screenshot(path=f"{SHOTS}/x17-state-tags.png", full_page=True)
    return {
        "tags": tags,
        "distinct_words": sorted({t["text"] for t in tags}),
        "non_tag_cells": page.eval_on_selector_all(
            f"{JOB} tbody td:nth-child({index})",
            "(els) => els.filter(el => !el.querySelector('.state')).map(el => el.innerText)"),
        "error_code_tags_in_list": len(page.query_selector_all(f"{JOB} .error-code")),
        "terminal_blocks_in_list": len(page.query_selector_all(f"{JOB} .terminal-block")),
    }


def observe_drawer(page):
    """X18（新增）：右侧抽屉的分区齐全——原运行历史屏展开行里有的，一样不少。"""
    open_jobs(page)
    failed = None
    for row in page.query_selector_all(f"{JOB} tbody tr"):
        cells = [c.inner_text().strip() for c in row.query_selector_all("td")]
        if cells[JOB_COLUMNS["state"]] == "失败":
            failed = cells[JOB_COLUMNS["task"]].split("\n")[0]
            break
    if failed is None:
        return {"skipped": "这一屏没有失败的任务——X18 的对象不存在"}
    open_drawer(page, failed)
    geometry = page.eval_on_selector(
        ".drawer",
        "(el) => { const r = el.getBoundingClientRect(); const cs = getComputedStyle(el);"
        " return {left: r.left, right: r.right, width: r.width, position: cs.position,"
        "  viewport: window.innerWidth}; }")
    report = {
        "geometry": geometry,
        "title": page.query_selector(".drawer-header h2").inner_text(),
        "run_record_id_beside_title": page.query_selector(".drawer-header .sub").inner_text(),
        "panels": [h.inner_text() for h in page.query_selector_all(".drawer .panel > h3")],
        "terminal_blocks": [b.inner_text().replace("\n", " ")
                            for b in page.query_selector_all(".drawer .terminal-block")],
        "error_codes": [b.inner_text().replace("\n", " ")
                        for b in page.query_selector_all(".drawer .error-code")],
        "kv": page.eval_on_selector_all(
            ".drawer .kv > div",
            "(els) => els.map(el => [el.querySelector('.k').innerText,"
            " el.querySelector('.v').innerText])"),
        "source_sql": page.query_selector(".drawer .drawer-sql").inner_text(),
        "footer_note": page.query_selector(".drawer-footer .drawer-note").inner_text(),
        "footer_buttons": [b.inner_text().strip()
                           for b in page.query_selector_all(".drawer-footer button")],
    }
    page.screenshot(path=f"{SHOTS}/x18-drawer.png", full_page=True)
    page.click(".drawer-footer button.is-ghost")
    return report


def observe_history_redirect(page):
    """ADR-0043 §2：旧的 `#history` 地址**重定向**到作业中心，不是 404 也不是空屏。"""
    page.set_viewport_size({"width": 1440, "height": 1200})
    page.goto(f"{BASE}/#history", wait_until="networkidle")
    page.wait_for_selector(f"{JOB} tbody tr")
    return {
        "hash_after_load": page.evaluate("() => window.location.hash"),
        "job_center_rendered": page.query_selector(f"{JOB}") is not None,
        "history_section": page.query_selector("#history") is not None,
        "active_nav": page.query_selector(".nav-item.is-active").inner_text().strip(),
    }

def observe_empty_datasources(page):
    """ADR-0039 §8：一个数据源都没有时，下拉里给的不是空白，而是一条通往数据源屏的路。"""
    page.set_viewport_size({"width": 1440, "height": 1200})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector(f"{JOB} tbody tr")
    page.click(f'{JOB} .table-toolbar button.is-primary')
    page.wait_for_selector(".modal .builder-guide")
    hints = page.query_selector_all('.modal a.text-button[href="#datasources"]')
    page.screenshot(path=f"{SHOTS}/empty-datasource-hint.png", full_page=True)
    return {
        "hint_count": len(hints),
        "hint_texts": [hint.inner_text() for hint in hints],
        "select_placeholder": [
            option.inner_text()
            for option in page.query_selector_all(".modal .builder-guide select option")
        ],
    }


def observe_agents(page):
    """X19（新增，ADR-0044 §6）：目标端 Agent 屏。

    五件事，一件都不许靠「看上去对」：
      1. 列结构与三档状态**分开报**，且身份不符比不在线更红（量 `backgroundColor`）；
      2. 注册一个打不通的地址 → **502 + 列表不多出一行**（探不通就不落库）；
      3. 注册一个通的 → 状态在线、身份列有值；这一屏**没有「测试连接」按钮**；
      4. 删一台被数据源引用的 → 被拒且**点名列出**是哪几条数据源；
      5. 数据源对话框里 agent 是**必选下拉**，换一台之后保存按钮重新变灰。
    """
    open_agents(page)
    headers = [h.inner_text() for h in page.query_selector_all("#agents thead th")]
    rows_before = page.query_selector_all("#agents tbody tr")
    states = [
        {
            "text": tag.inner_text(),
            "background": tag.evaluate("(el) => getComputedStyle(el).backgroundColor"),
        }
        for tag in page.query_selector_all("#agents tbody .state")
    ]
    reasons = [r.inner_text() for r in page.query_selector_all("#agents tbody .row-test-result")]
    page.screenshot(path=f"{SHOTS}/x19-agent-list.png", full_page=True)

    # ①.5 点一行「探测」：只锁自己那一行，结果**落回那一行**，失败也不弹对话框。
    #     采样口径与 X12 同一条：桩答得比采样快时会采到已落定的态，如实标出来。
    probe_button = rows_before[1].query_selector(
        'button[aria-label="探测"], button[aria-label="正在探测"]'
    )
    probe_button.click()
    probe_click = {
        "clicked_row_aria_label": probe_button.get_attribute("aria-label"),
        "clicked_row_disabled": probe_button.is_disabled(),
        "other_rows_disabled": [
            b.is_disabled()
            for b in (
                r.query_selector('button[aria-label="探测"], button[aria-label="正在探测"]')
                for r in [rows_before[0], rows_before[2]]
            )
            if b is not None
        ],
    }
    page.wait_for_timeout(400)
    probe_click["settled_aria_label"] = probe_button.get_attribute("aria-label")
    probe_click["row_after"] = [
        c.inner_text().replace("\n", " ") for c in rows_before[1].query_selector_all("td")
    ][:3]
    probe_click["dialogs_on_screen"] = len(page.query_selector_all(".modal"))

    # ② 注册一个打不通的地址：报错，且列表不多出一行。
    page.click("#agents .card-header button.is-primary")
    page.wait_for_selector(".modal")
    field_input(page, "名称").fill("走查 · 打不通的")
    field_input(page, "地址").fill("http://127.0.0.1:59999")
    page.click('.modal button[type="submit"]')
    page.wait_for_selector(".modal .form-error")
    dead_error = page.query_selector(".modal .form-error").inner_text()
    has_test_button = any(
        "测试连接" in b.inner_text() for b in page.query_selector_all(".modal button")
    )
    page.screenshot(path=f"{SHOTS}/x19-register-unreachable.png", full_page=True)

    # ③ 改成通的那个地址：保存即连接。
    field_input(page, "地址").fill("http://127.0.0.1:8080")
    page.click('.modal button[type="submit"]')
    page.wait_for_selector(".modal", state="detached")
    page.wait_for_timeout(200)
    rows_after = [
        [c.inner_text().replace("\n", " ") for c in r.query_selector_all("td")]
        for r in page.query_selector_all("#agents tbody tr")
    ]
    page.screenshot(path=f"{SHOTS}/x19-registered.png", full_page=True)

    # ④ 删一台被数据源引用的：被拒 + 点名。
    refusal = None
    refusal_names = []
    for row in page.query_selector_all("#agents tbody tr"):
        cells = row.query_selector_all("td")
        if "未被引用" in cells[6].inner_text():
            continue
        row.query_selector('button[aria-label="删除 Agent"]').click()
        page.wait_for_selector(".modal")
        page.click('.modal .modal-footer button.is-danger')
        page.wait_for_selector(".modal .form-error")
        refusal = page.query_selector(".modal .form-error").inner_text()
        refusal_names = [li.inner_text() for li in page.query_selector_all(".modal li")]
        page.screenshot(path=f"{SHOTS}/x19-delete-refused.png", full_page=True)
        page.click('.modal .modal-footer button.is-ghost')
        page.wait_for_selector(".modal", state="detached")
        break

    # ⑤ 数据源对话框里的 agent 下拉：必选、换一台之后保存重新变灰。
    # **要挑一条 MySQL 的行**：Oracle 数据源不绑 agent（ADR-0044 §1），
    # 对着它开对话框根本没有这个字段，量出来的 null 是「挑错了行」不是「字段没了」。
    open_datasources(page)
    for row in page.query_selector_all("#datasources tbody tr"):
        if row.query_selector_all("td")[1].inner_text().strip() == "MySQL":
            row.query_selector('button[aria-label="编辑数据源"]').click()
            break
    page.wait_for_selector(".modal")
    selector = '.modal label.form-field:has(span.field-label:text-is("目标端 Agent")) select'
    select = page.query_selector(selector)
    dropdown = None
    if select is not None:
        options = [o.inner_text() for o in page.query_selector_all(f"{selector} option")]
        submit = page.query_selector('.modal button[type="submit"]')
        before = submit.is_disabled()
        values = [o.get_attribute("value") for o in page.query_selector_all(f"{selector} option")]
        other = next((v for v in values if v and v != select.input_value()), None)
        if other is not None:
            page.select_option(selector, other)
        dropdown = {
            "required": select.get_attribute("required") is not None,
            "options": options,
            "submit_disabled_before_switch": before,
            "submit_disabled_after_switch": page.query_selector(
                '.modal button[type="submit"]').is_disabled(),
        }
        page.screenshot(path=f"{SHOTS}/x19-datasource-agent-select.png", full_page=True)
    page.click('.modal .modal-footer button.is-ghost')

    return {
        "columns": headers,
        "row_count_before": len(rows_before),
        "state_tags": states,
        "reasons_on_row": reasons,
        "probe_click": probe_click,
        "register_unreachable_error": dead_error,
        "register_dialog_has_test_button": has_test_button,
        "rows_after_register": rows_after,
        "delete_refusal": refusal,
        "delete_refusal_names": refusal_names,
        "datasource_agent_dropdown": dropdown,
    }


def main():
    os.makedirs(SHOTS, exist_ok=True)
    only_empty = os.environ.get("V1_MOCK_EMPTY_DATASOURCES") == "1"
    # `X_ONLY=bulk`：对着**加量过的**桩（`X_BULK=1`）只跑要填充行才有对象的那几条
    # （分页 X11、跨页全选 X15），同时不把前面那些态的实录塞满填充行。
    # `pagination` 是 2026-08-21 之前的旧名，留着让旧编排还能跑。
    only_bulk = os.environ.get("X_ONLY") in ("bulk", "pagination")
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch()
        page = browser.new_page()
        if only_empty:
            report = {"empty_datasources": observe_empty_datasources(page)}
            browser.close()
            print(json.dumps(report, ensure_ascii=False, indent=2))
            return
        if only_bulk:
            report = {
                "X11_pagination_bulk": observe_history_filters(page),
                "X15_bulk": observe_bulk(page),
            }
            browser.close()
            print(json.dumps(report, ensure_ascii=False, indent=2))
            return
        report = {
            "X1_nav": observe_nav(page),
            "X2_list": observe_list(page),
            "X3_new_dialog": observe_new_dialog(page),
            "X4_rename_and_delete": observe_rename_and_delete(page),
            "X5_X6_X7_at_1440": observe_builder(page, 1440),
            "X6_at_1024": observe_builder(page, 1024),
            "X8_job_tables": observe_task_screen(page),
            "X10_job_filters": observe_task_filters(page),
            "X12_datasource_row_test": observe_row_test(page),
            "X13_sider_collapse": observe_sider_collapse(page),
            "X14_job_columns": observe_job_columns(page),
            "X16_progress": observe_progress(page),
            "X17_state_tags": observe_state_tags(page),
            "X18_drawer": observe_drawer(page),
            # X19 也会**改状态**（注册一台新 agent），所以同样排在只读的那些之后。
            "X19_agents": observe_agents(page),
            # X9 摆在最后：它会**真的发起一次运行**，把桩里的最近一次运行换掉。
            # 放在前面会让 X16 / X17 / X8 看到的是被它改过的那一屏。
            "X9_rerun": observe_rerun(page),
            "history_redirect": observe_history_redirect(page),
        }
        browser.close()
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
