#!/usr/bin/env python3
"""M3 渲染面走查 W1–W6 的机器观察（配 `m3-mock.py` 的桩后端）。

按 ADR-0028 §1 的先例：**只观察，不断言**；一行 DOM 断言都不进验收套件。
输出是给人抄进走查记录的实际观察，不是 pass/fail。
"""

import json
import sys

from playwright.sync_api import sync_playwright

BASE = f"http://127.0.0.1:{sys.argv[1] if len(sys.argv) > 1 else 18099}"
SHOTS = "/tmp/m3-visual"


def observe_run_screen(page, width, label):
    """W1 / W2 / W6：映射预检失败态的运行详情屏。"""
    page.set_viewport_size({"width": width, "height": 1000})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector("#tasks tbody tr")
    page.click('button[aria-label="发起运行"]')
    page.wait_for_selector(".run-params input")
    page.fill(".run-params input", "2026-08-18")
    page.click('button[type="submit"]')
    page.wait_for_selector(".precheck-reports")

    reports = page.query_selector(".precheck-reports")
    sections = page.query_selector_all(".precheck-reports section")
    headers = [h.inner_text() for h in page.query_selector_all(".diagnostic-table thead th")]
    rows = [[c.inner_text() for c in r.query_selector_all("td")]
            for r in page.query_selector_all(".diagnostic-table tbody tr")]
    wrap = page.query_selector(".diagnostic-table-wrap")
    page.screenshot(path=f"{SHOTS}/{label}.png", full_page=True)

    return {
        "viewport": width,
        "sections": len(sections),
        "section_boxes": [s.bounding_box() for s in sections],
        "reports_box": reports.bounding_box(),
        "columns": headers,
        "row_count": len(rows),
        "empty_suggestion_cells": [r for r in rows if len(r) < 5 or not r[4].strip()],
        "rows": rows,
        "table_overflow_x": wrap.evaluate("(el) => el.scrollWidth - el.clientWidth"),
        "body_overflow_x": page.evaluate(
            "() => document.documentElement.scrollWidth - document.documentElement.clientWidth"
        ),
        "total_line": page.query_selector(".precheck-reports small").inner_text(),
    }


def open_builder(page, target_table):
    page.set_viewport_size({"width": 1440, "height": 1200})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector("#tasks tbody tr")
    page.click('button[aria-label="编辑任务定义"]')
    page.wait_for_selector(".builder-guide")
    page.fill('.form-field:has(input[value="M3_B1"]) input', target_table)
    page.click(".column-fetch-section button")
    page.wait_for_selector(".fetch-ready")


def observe_column_fetch(page):
    """W3 / W4：取列态 —— 三档标记与建表 SQL 占位符。"""
    open_builder(page, "M3_B1")
    type_cells = page.query_selector_all(".fetch-ready tbody tr td:nth-child(2)")
    marks = []
    for cell in type_cells:
        text = cell.inner_text()
        if "[" in text:
            style = cell.evaluate(
                "(el) => { const cs = getComputedStyle(el);"
                " return {color: cs.color, background: cs.backgroundColor, className: el.className}; }"
            )
            marks.append({"text": text, **style})
    placeholders = page.query_selector_all(".ddl-placeholder")
    ddl = page.query_selector(".ddl-output").inner_text()
    page.screenshot(path=f"{SHOTS}/w3-w4-column-fetch.png", full_page=True)
    return {
        "type_column": [c.inner_text() for c in type_cells],
        "marked_cells": marks,
        "mark_element_tags": [c.evaluate("(el) => el.tagName") for c in type_cells],
        "placeholders": [p.inner_text() for p in placeholders],
        "ddl_given_in_full": ddl.strip().endswith("DEFAULT CHARSET=utf8mb4;"),
        "ddl_lines": ddl.count("\n") + 1,
    }


def observe_rejected_fetch(page):
    """W5：白名单外的列 —— 列表照给，只有 DDL 区块换成「整份不给」。"""
    open_builder(page, "REJECTED")
    listed = [c.inner_text() for c in page.query_selector_all(".fetch-ready tbody tr td:nth-child(1)")]
    crit = page.query_selector(".row-size-warning.is-crit")
    page.screenshot(path=f"{SHOTS}/w5-rejected.png", full_page=True)
    return {
        "columns_still_listed": listed,
        "ddl_block_present": page.query_selector(".ddl-output") is not None,
        "crit_block_text": crit.inner_text() if crit else None,
    }


def observe_builder_surface(page):
    """本票新增的构建器面（条件 / 排序 / 只读 SQL），走查清单之外的旁证。"""
    page.set_viewport_size({"width": 1440, "height": 1400})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector("#tasks tbody tr")
    page.click('button[aria-label="编辑任务定义"]')
    page.wait_for_selector(".generated-sql pre")
    page.screenshot(path=f"{SHOTS}/builder.png", full_page=True)
    datasource_selects = page.query_selector_all(
        '.builder-guide:has(#builder-datasource-title) select'
    )
    return {
        # ADR-0037 §8 的绑定面：两个下拉，值是任务上存着的两个数据源 id。
        "datasource_selects": [
            {
                "value": select.input_value(),
                "options": [o.inner_text() for o in select.query_selector_all("option")],
            }
            for select in datasource_selects
        ],
        "condition_rows": len(page.query_selector_all(".condition-row")),
        "sql_is_readonly": page.query_selector(".generated-sql textarea") is None,
        "sql_text": page.query_selector(".generated-sql pre").inner_text(),
        "run_parameters": page.query_selector(".run-parameter-list").inner_text(),
        "primary_key_boxes": len(page.query_selector_all('.builder-columns input[aria-label*="主键"]')),
        "key_note": page.query_selector(".builder-key-note").inner_text(),
    }


def main():
    import os

    os.makedirs(SHOTS, exist_ok=True)
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch()
        page = browser.new_page()
        report = {
            "W1_W6_at_1440": observe_run_screen(page, 1440, "w1-w6-1440"),
            "W2_at_1024": observe_run_screen(page, 1024, "w2-1024"),
            "W3_W4": observe_column_fetch(page),
            "W5": observe_rejected_fetch(page),
            "builder": observe_builder_surface(page),
        }
        browser.close()
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
