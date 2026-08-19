#!/usr/bin/env python3
"""第一版渲染面走查 X1–X8 的机器观察。

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
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch()
        page = browser.new_page()
        if only_empty:
            report = {"empty_datasources": observe_empty_datasources(page)}
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
        }
        browser.close()
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
