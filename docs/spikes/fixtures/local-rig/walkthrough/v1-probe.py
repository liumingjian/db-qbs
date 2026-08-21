#!/usr/bin/env python3
"""第一版渲染面走查 X1–X12 的机器观察。

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


# 运行历史表的列序（P1 / ADR-0042 §4 之后）。位置索引只在这一处写，别再散到各处。
HISTORY_COLUMNS = {
    "task": 0, "outcome": 1, "error_code": 2, "rows": 3,
    "elapsed": 4, "started_at": 5, "action": 6, "expand": 7,
}


def field_input(page, label):
    """按字段标签取输入框——不认位置。"""
    return page.query_selector(
        f'.modal label.form-field:has(span.field-label:text-is("{label}")) input'
    )


def open_datasources(page, width=1440, height=1200):
    page.set_viewport_size({"width": width, "height": height})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector("#tasks tbody tr")
    page.click('nav[aria-label="主导航"] a[href="#datasources"]')
    page.wait_for_selector("#datasources")


def observe_nav(page):
    """X1：数据源是第三项，与既有两项同一套导航元素。"""
    page.set_viewport_size({"width": 1440, "height": 1200})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector("#tasks tbody tr")
    items = page.query_selector_all('aside.sidebar nav[aria-label="主导航"] > *')
    return {
        "order": [item.inner_text().strip().replace("\n", " ") for item in items],
        "tags": [item.evaluate("(el) => el.tagName") for el, item in zip(items, items)],
        "classes": [item.get_attribute("class") for item in items],
        "third_item_class": items[2].get_attribute("class") if len(items) > 2 else None,
    }


def observe_list(page):
    """X2：列表七列，没有搜索框 / 类型筛选 / 连接状态列。"""
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
    page.wait_for_selector("#tasks tbody tr")
    page.click('button[aria-label="编辑任务定义"]')
    page.wait_for_selector(".modal .builder-guide")


def observe_builder(page, width):
    """X5 / X6 / X7：目标表下拉、映射两栏、单位标注。"""
    open_builder(page, width)
    # 先读一次源列，好让「没选中的列」这一态有对象（桩里 N_AMT 不在任务规格里）。
    page.click('.modal .builder-guide button:has-text("读取列")')
    page.wait_for_selector('.modal .builder-columns tbody tr:nth-child(4)')

    # X5：原生 input list + datalist，打字仍然能打。
    target_input = page.query_selector('.modal input[list]')
    datalist = page.query_selector(f'.modal datalist#{target_input.get_attribute("list")}')
    x5 = {
        "input_list_attr": target_input.get_attribute("list"),
        "input_readonly": target_input.get_attribute("readonly") is not None,
        "datalist_options": [o.get_attribute("value") for o in datalist.query_selector_all("option")],
        "current_value": target_input.input_value(),
    }

    # X6：映射两栏。
    headers = [h.inner_text() for h in page.query_selector_all(".modal .builder-columns th")]
    rows = []
    for row in page.query_selector_all(".modal .builder-columns tbody tr"):
        cells = row.query_selector_all("td")
        cell_input = row.query_selector(".cell-input")
        boxes = row.query_selector_all('input[type="checkbox"]')
        rows.append({
            "column": cells[1].inner_text(),
            "checkbox_count": len(boxes),
            "select_checked": boxes[0].is_checked(),
            "target_value": cell_input.input_value() if cell_input else None,
            "target_disabled": cell_input.is_disabled() if cell_input else None,
            "key_checked": boxes[-1].is_checked() if len(boxes) > 1 else None,
            "key_disabled": boxes[-1].is_disabled() if len(boxes) > 1 else None,
            "input_height": cell_input.bounding_box()["height"] if cell_input else None,
            "row_height": row.bounding_box()["height"],
            "input_right_edge": (
                cell_input.bounding_box()["x"] + cell_input.bounding_box()["width"]
                if cell_input else None
            ),
            "cell_right_edge": (
                cells[5].bounding_box()["x"] + cells[5].bounding_box()["width"]
                if len(cells) > 5 else None
            ),
        })
    x6 = {
        "viewport": width,
        "headers": headers,
        "rows": rows,
        "table_overflow_x": page.eval_on_selector(
            ".modal .builder-columns",
            "(el) => el.scrollWidth - el.clientWidth",
        ),
        "dark_theme_media": page.evaluate(
            "() => [...document.styleSheets].flatMap(s => { try { return [...s.cssRules] }"
            " catch { return [] } }).filter(r => (r.conditionText || '').includes('prefers-color-scheme')).length"
        ),
    }

    # 改一个目标名，看主键跟不跟着走（ADR-0039 增补 1 的渲染面）。
    rename = None
    for row in page.query_selector_all(".modal .builder-columns tbody tr"):
        cell_input = row.query_selector(".cell-input")
        boxes = row.query_selector_all('input[type="checkbox"]')
        if cell_input and not cell_input.is_disabled() and boxes[-1].is_checked():
            before = cell_input.input_value()
            cell_input.fill("CUST_ID_RENAMED")
            rename = {
                "before": before,
                "after": cell_input.input_value(),
                "key_still_checked": boxes[-1].is_checked(),
            }
            cell_input.fill(before)
            break
    x6["rename_follows_key"] = rename

    # X7：单位标注与静态说明。
    x7 = {
        "length_headers": [h for h in headers if "长度" in h],
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
    page.wait_for_selector(".modal table.data-grid tr.is-unmapped", timeout=5000)
    reference_rows = []
    for row in page.query_selector_all(
        ".modal section[aria-labelledby='target-columns-title'] tbody tr"
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
            ".modal section[aria-labelledby='target-columns-title'] th"
        )
    ]
    x5["reference_rows"] = reference_rows
    x5["footnote"] = page.query_selector(
        ".modal section[aria-labelledby='target-columns-title'] footer"
    ).inner_text()
    page.screenshot(path=f"{SHOTS}/x5-x7-builder-{width}.png", full_page=True)
    return {"X5": x5, "X6": x6, "X7": x7}


def observe_task_screen(page):
    """X8：任务屏新增一列「源 → 目标」，显示的是数据源名字不是 id。"""
    page.set_viewport_size({"width": 1440, "height": 1200})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector("#tasks tbody tr")
    headers = [h.inner_text() for h in page.query_selector_all("#tasks thead th")]
    cells = [c.inner_text() for c in page.query_selector_all("#tasks tbody tr td:nth-child(2)")]
    style = page.eval_on_selector(
        "#tasks tbody tr td:nth-child(2)",
        "(el) => { const cs = getComputedStyle(el);"
        " return {color: cs.color, background: cs.backgroundColor, children: el.children.length}; }",
    )
    page.screenshot(path=f"{SHOTS}/x8-task-list.png", full_page=True)
    return {"columns": headers, "binding_cells": cells, "binding_cell_style": style}


def open_history(page, width=1440):
    page.set_viewport_size({"width": width, "height": 1200})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector("#tasks tbody tr")
    page.click('nav[aria-label="主导航"] a[href="#history"]')
    page.wait_for_selector("#history tbody tr")


def fill_value(field):
    """给一个**没被预填**的参数编一个值——只为把表单填满，不主张值本身有意义。"""
    kind = field.get_attribute("type")
    return {"date": "2026-08-18", "number": "1"}.get(kind, "HZ")


def observe_rerun(page):
    """X9：运行历史的「重跑」入口三态 + 发起对话框的预填。

    判据在 `web/src/rerun.ts`：FAILED 与结局不明给入口，进行中与 SUCCEEDED 不给，
    任务已删除给**禁用**入口加原因。预填三规则「有取行值 / 缺留空 / 多丢弃」。
    """
    open_history(page)
    headers = [h.inner_text() for h in page.query_selector_all("#history thead th")]
    rows = []
    for tr in page.query_selector_all("#history tbody tr:not(.history-detail-row)"):
        cells = [c.inner_text().strip() for c in tr.query_selector_all("td")]
        # 认前缀：禁用那条的 `aria-label` 把原因也带在里面（原生 tooltip 对
        # `disabled` 控件不出现，原因只能进无障碍名字）。
        button = tr.query_selector('button[aria-label^="重跑"]')
        rows.append({
            "run_record_id": cells[HISTORY_COLUMNS["task"]] if cells else None,
            "outcome_cell": (
                cells[HISTORY_COLUMNS["outcome"]].replace("\n", " ")
                if len(cells) > HISTORY_COLUMNS["outcome"] else None),
            "action_cell": (
                cells[HISTORY_COLUMNS["action"]]
                if len(cells) > HISTORY_COLUMNS["action"] else None),
            "rerun_button": None if button is None else {
                "disabled": button.is_disabled(),
                "aria_label": button.get_attribute("aria-label"),
                "wrapper_title": button.evaluate(
                    "(el) => el.closest('.row-actions')?.getAttribute('title')"),
            },
        })
    page.screenshot(path=f"{SHOTS}/x9-history-rerun.png", full_page=True)

    enabled = page.query_selector_all(
        '#history tbody button[aria-label^="重跑"]:not([disabled])')
    if not enabled:
        return {"columns": headers, "rows": rows,
                "skipped": "这批历史里没有一条可重跑——X9 的对象不存在"}

    # 点第一条可重跑的：对话框是**既有的那一个**，确认键仍是「发起」。
    source_row = next(r for r in rows if (r["rerun_button"] or {}).get("disabled") is False)
    enabled[0].click()
    page.wait_for_selector(".modal")
    dialog = {
        "source_row": source_row,
        "title": page.query_selector(".modal h2").inner_text(),
        "context": page.query_selector(".modal .modal-context").inner_text(),
        "submit_label": page.query_selector('.modal button[type="submit"]').inner_text(),
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

    # 改一个字段，证明预填不是只读的；顺带把留空的那些填满，好让并跑提示有机会出现。
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

    # 确认发起：既有闸门原样走，出来的要么是新记录、要么是后端的拒绝报文。
    page.click('.modal button[type="submit"]')
    try:
        page.wait_for_selector(".modal .form-error", timeout=8000)
        dialog["submit_result"] = {"rejected": page.query_selector(".modal .form-error").inner_text()}
    except Exception:
        dialog["submit_result"] = {"accepted": True}

    open_history(page)
    after = [
        tr.query_selector("td").inner_text().strip()
        for tr in page.query_selector_all("#history tbody tr:not(.history-detail-row)")
    ]

    # 结局不明那条单独再点一次：它的 `outcome` 也是 null，并跑提示如果按 `outcome` 认
    # 「进行中」，就会指着你刚点的这条早已死掉的记录说它还在跑（#150 审查所见）。
    # 这里要看到的是**没有**提示条。
    unknown = None
    for tr in page.query_selector_all("#history tbody tr:not(.history-detail-row)"):
        cells = [c.inner_text().strip() for c in tr.query_selector_all("td")]
        button = tr.query_selector('button[aria-label^="重跑"]:not([disabled])')
        if button is not None and cells and "结局不明" in cells[HISTORY_COLUMNS["outcome"]]:
            button.click()
            page.wait_for_selector(".modal")
            for label in page.query_selector_all(".modal .run-params label.form-field"):
                field = label.query_selector("input")
                if field.input_value() == "":
                    field.fill(fill_value(field))
            page.wait_for_timeout(1500)
            hint = page.query_selector(".modal .stale-run-hint")
            unknown = {
                "run_record_id": cells[HISTORY_COLUMNS["task"]],
                "values": [f.input_value()
                           for f in page.query_selector_all(".modal .run-params input")],
                "stale_run_hint": None if hint is None else hint.inner_text(),
            }
            page.screenshot(path=f"{SHOTS}/x9-unknown-rerun.png", full_page=True)
            page.click('.modal .modal-footer button.is-ghost')
            break

    return {
        "unknown_row_rerun": unknown,
        "columns": headers,
        "rows": rows,
        "dialog": dialog,
        "records_before": [r["run_record_id"] for r in rows],
        "records_after": after,
    }


def observe_pagination(page, scope):
    """分页条的实际观察。**总数不超过一页时组件自己不渲染**，那时回 `None`——
    这本身就是一条要看的事实，不是「没观察到」。"""
    footer = page.query_selector(f"{scope} .list-pagination")
    if footer is None:
        return None
    return {
        "total_text": footer.query_selector(".pagination-total").inner_text(),
        "page_text": footer.query_selector(".pagination-page").inner_text(),
        "buttons": [
            {"label": b.inner_text(), "disabled": b.is_disabled()}
            for b in footer.query_selector_all("button")
        ],
    }


def filter_strip(page, scope):
    """筛选条上摆了哪几项、按钮是哪几个——认标签文字，不认位置。"""
    return {
        "fields": [
            label.query_selector("span").inner_text()
            for label in page.query_selector_all(f"{scope} .history-filters label.filter-field")
        ],
        "buttons": [
            b.inner_text().strip()
            for b in page.query_selector_all(f"{scope} .history-filters button")
            if b.inner_text().strip() != ""
        ],
    }


def latest_run_cells(page):
    """任务屏「最近运行」一列的实际内容（每行两段：状态 + 时间；没跑过的只有一段）。"""
    return [
        cell.inner_text().replace("\n", " · ")
        for cell in page.query_selector_all("#tasks tbody tr td.latest-run-column")
    ]


def observe_task_filters(page):
    """X10：任务屏的筛选条、「最近运行」一列与客户端分页。"""
    page.set_viewport_size({"width": 1440, "height": 1200})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector("#tasks tbody tr")
    before = {
        "strip": filter_strip(page, "#tasks"),
        "summary": page.query_selector("#tasks .card-subtitle").inner_text(),
        "row_count": len(page.query_selector_all("#tasks tbody tr")),
        "latest_run_header": page.query_selector("#tasks thead th.latest-run-column").inner_text(),
        "latest_run_cells": latest_run_cells(page),
        "pagination": observe_pagination(page, "#tasks"),
    }
    page.screenshot(path=f"{SHOTS}/x10-task-filters.png", full_page=True)

    # 改下拉**不**重筛（查询是显式的）：先记下改完还没点查询时的行数。
    status = page.query_selector('#tasks .history-filters label.filter-field:has(span:text-is("最近状态")) select')
    status.select_option(label="结局不明")
    page.wait_for_timeout(300)
    untouched = len(page.query_selector_all("#tasks tbody tr"))

    # 两次查询：一次筛得出东西，一次一条都筛不出——后者要看的是「没有匹配的任务」
    # 这句自陈**出来了没有**，空表格加一个孤零零的分页条不算回答。
    queried = {}
    for label in ("结局不明", "失败"):
        status.select_option(label=label)
        page.click('#tasks .history-filters button.is-primary')
        page.wait_for_timeout(300)
        queried[label] = {
            "summary": page.query_selector("#tasks .card-subtitle").inner_text(),
            "row_count": len(page.query_selector_all("#tasks tbody tr")),
            "latest_run_cells": latest_run_cells(page),
            "no_results": (page.query_selector("#tasks .no-results").inner_text()
                           if page.query_selector("#tasks .no-results") else None),
            "pagination": observe_pagination(page, "#tasks"),
        }
        page.screenshot(path=f"{SHOTS}/x10-task-filtered-{label}.png", full_page=True)

    page.click('#tasks .history-filters button.is-ghost')
    page.wait_for_timeout(300)
    reset = {
        "summary": page.query_selector("#tasks .card-subtitle").inner_text(),
        "row_count": len(page.query_selector_all("#tasks tbody tr")),
        "status_select_value": status.input_value(),
    }
    return {
        "before": before,
        "row_count_after_select_before_query": untouched,
        "after_query": queried,
        "after_reset": reset,
    }


def observe_history_filters(page):
    """X11：运行历史的任务 / 状态两维筛选与客户端分页。"""
    open_history(page)
    before = {
        "strip": filter_strip(page, "#history"),
        "summary": page.query_selector("#history .card-subtitle").inner_text(),
        "row_count": len(page.query_selector_all("#history tbody tr:not(.history-detail-row)")),
        "pagination": observe_pagination(page, "#history"),
    }
    page.screenshot(path=f"{SHOTS}/x11-history-filters.png", full_page=True)

    # 翻页在**没筛**的那份上看：筛完只剩两条时分页条本来就该消失，那时没有翻页这回事。
    turned = None
    footer = page.query_selector("#history .list-pagination")
    if footer is not None:
        first_before = page.query_selector("#history tbody tr td").inner_text().strip()
        next_button = footer.query_selector_all("button")[-1]
        if not next_button.is_disabled():
            next_button.click()
            page.wait_for_timeout(400)
            turned = {
                "page_text": page.query_selector("#history .pagination-page").inner_text(),
                "prev_disabled_on_page_2": page.query_selector_all(
                    "#history .list-pagination button")[0].is_disabled(),
                "next_disabled_on_last_page": page.query_selector_all(
                    "#history .list-pagination button")[-1].is_disabled(),
                "first_cell_before": first_before,
                "first_cell_after": page.query_selector("#history tbody tr td").inner_text().strip(),
                "row_count": len(page.query_selector_all(
                    "#history tbody tr:not(.history-detail-row)")),
            }
            page.screenshot(path=f"{SHOTS}/x11-history-page2.png", full_page=True)
            # 翻回第 1 页再做筛选，免得「筛完还停在第 2 页」这一态混进下面的观察。
            page.query_selector_all("#history .list-pagination button")[0].click()
            page.wait_for_timeout(300)

    status = page.query_selector('#history .history-filters label.filter-field:has(span:text-is("状态")) select')
    status.select_option(label="失败")
    page.wait_for_timeout(300)
    untouched = len(page.query_selector_all("#history tbody tr:not(.history-detail-row)"))

    page.click('#history .history-filters button.is-primary')
    page.wait_for_timeout(600)
    outcomes = [
        tr.query_selector_all("td")[HISTORY_COLUMNS["outcome"]].inner_text().replace("\n", " ")
        for tr in page.query_selector_all("#history tbody tr:not(.history-detail-row)")
    ]
    queried = {
        "summary": page.query_selector("#history .card-subtitle").inner_text(),
        "row_count": len(outcomes),
        "outcome_cells": outcomes,
        "pagination": observe_pagination(page, "#history"),
    }
    page.screenshot(path=f"{SHOTS}/x11-history-filtered.png", full_page=True)

    page.click('#history .history-filters button.is-ghost')
    page.wait_for_timeout(600)
    return {
        "before": before,
        "row_count_after_select_before_query": untouched,
        "after_next_page_unfiltered": turned,
        "after_query_status_failed": queried,
        "after_reset": {
            "summary": page.query_selector("#history .card-subtitle").inner_text(),
            "row_count": len(page.query_selector_all(
                "#history tbody tr:not(.history-detail-row)")),
        },
    }


def observe_row_test(page):
    """X12：数据源行内「测试连接」——瞬态结果、只锁自己那一行、仍然没有筛选条 / 状态列。"""
    open_datasources(page)
    rows = page.query_selector_all("#datasources tbody tr")
    buttons = [r.query_selector('button:has-text("测试连接")') for r in rows]
    before = {
        "row_count": len(rows),
        "buttons_present": sum(1 for b in buttons if b is not None),
        "results_present": len(page.query_selector_all("#datasources .row-test-result")),
        "search_fields": len(page.query_selector_all("#datasources .search-field")),
        "filter_strips": len(page.query_selector_all("#datasources .history-filters")),
        "selects_on_screen": len(page.query_selector_all("#datasources select")),
    }

    buttons[0].click()
    # 只锁自己那一行：点下去的**当场**采样，一毫秒都不等。
    # 桩答得比采样还快时会采到已经落定的态——那不是缺陷，是桩太快；
    # `already_settled` 把这一点如实标出来，别把采空读成「没实现」。
    while_testing = {
        "clicked_row_button": buttons[0].inner_text().strip(),
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
        "button_back_to_idle": buttons[0].inner_text().strip(),
    }
    page.screenshot(path=f"{SHOTS}/x12-datasource-row-test.png", full_page=True)
    return {"before": before, "while_testing": while_testing, "settled": settled}


def observe_empty_datasources(page):
    """ADR-0039 §8：一个数据源都没有时，下拉里给的不是空白，而是一条通往数据源屏的路。"""
    page.set_viewport_size({"width": 1440, "height": 1200})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector("#tasks tbody tr")
    page.click('#tasks .card-header button')
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


def main():
    os.makedirs(SHOTS, exist_ok=True)
    only_empty = os.environ.get("V1_MOCK_EMPTY_DATASOURCES") == "1"
    # `X_ONLY=pagination`：对着**加量过的**桩（`X_BULK=1`）只跑 X10/X11，
    # 好让分页那一格有真对象，同时不把 X1–X9 的实录塞满填充行。
    only_pagination = os.environ.get("X_ONLY") == "pagination"
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch()
        page = browser.new_page()
        if only_empty:
            report = {"empty_datasources": observe_empty_datasources(page)}
            browser.close()
            print(json.dumps(report, ensure_ascii=False, indent=2))
            return
        if only_pagination:
            report = {
                "X10_task_filters_bulk": observe_task_filters(page),
                "X11_history_filters_bulk": observe_history_filters(page),
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
            "X8_task_screen": observe_task_screen(page),
            "X9_rerun": observe_rerun(page),
            "X10_task_filters": observe_task_filters(page),
            "X11_history_filters": observe_history_filters(page),
            "X12_datasource_row_test": observe_row_test(page),
        }
        browser.close()
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
