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
    page.wait_for_selector("#jobs tbody tr")
    # 发起没有对话框了：点了就跑，直接落到运行详情。
    page.click('button[aria-label="发起运行"]')
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
    """打开构建器、填目标表、取列。

    **选择器跟着第一版的构建器走，不是 M3 那套**（ADR-0039 §5/§6，实现在 #131）：

    * 目标表从普通输入框换成了原生 `<input list>` + `<datalist>`。旧写法
      `.form-field:has(input[value="M3_B1"]) input` 认的是 `value` **属性**，而 React 的
      受控输入只写 property、不反射属性，在 v1 构建器上永远选不中。改认 `list` 属性——
      它是静态写死的，不随输入变。
    * `.column-fetch-section` 在 v1 有**两处**（「目标表建表 SQL」与新增的「目标表列参考」
      共用这个类），裸选会命中歧义。认 `aria-labelledby="column-fetch-title"` 才是唯一那一个。
    * 「拿建表 SQL」按钮在 v1 落到了模态框滚动区的**视口之外**（1440x1200 下 y≈1568）。
      `page.click()` 会自己滚过去再点，但填完目标表名触发的 `/api/builder/sql` 会在滚动途中
      重渲染这一段，点击落空——**一次 `/api/columns` 都不发**，`.fetch-ready` 永远等不到。
      改成先等 SQL 回来、把按钮滚进视野，再走 DOM 的 `click()`：不依赖坐标，就不怕重渲染。
    """
    page.set_viewport_size({"width": 1440, "height": 1200})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector("#jobs tbody tr")
    page.click('button[aria-label="编辑任务定义"]')
    page.wait_for_selector(".builder-guide")
    # 2026-08-24：目标表输入框**又换了一次形态**。`f371935`（"Refine task builder
    # table selection"，2026-08-21）把 `<input list="target-table-options">` 换成了
    # 目标端那棵树上的搜索框，于是这一行从那天起就选不中任何东西——W1–W6 整份
    # 一跑就 30s 超时。那一票没有触发 W 系列，中间几票也没有，所以没人发现。
    # 这与 `47a2fed` 摘掉建表 SQL 区块是同一种事故：**判据还在，跑它的手断了**。
    # 认 `.target-tree-shell .tree-search input`——它是目标端那半边唯一的输入框；
    # 填完要 blur，`loadTargetColumns` 挂在 onBlur 上（与 X 走查的 X6 同一处坑）。
    target_input = page.wait_for_selector(".target-tree-shell .tree-search input")
    target_input.fill(target_table)
    target_input.evaluate("(el) => el.blur()")
    page.wait_for_timeout(500)
    button = page.query_selector('section[aria-labelledby="column-fetch-title"] header button')
    if button is None:
        # 2026-08-21：这一卡在界面上**已经不存在**了。它不是被本次改动删的——
        # `47a2fed`（"Prepare x2doris P1 frontend handoff"）把整段
        # 「目标表建表 SQL / 拿建表 SQL / .fetch-ready」从构建器里摘掉了，
        # 而那一票没有跑 W1–W6（`CLAUDE.md` 规则 1 挡的正是这个），于是没人发现。
        # 所有者 2026-08-21 裁定按有意的收窄对待，W3 / W4 / W5 就此判废（ADR-0043）。
        # 探针**只观察不断言**：这里不抛错，如实回一条「对象不存在」，让走查记录看得见。
        return False
    button.scroll_into_view_if_needed()
    button.evaluate("(el) => el.click()")
    page.wait_for_selector(".fetch-ready")
    return True


def observe_column_fetch(page):
    """W3 / W4：取列态 —— 三档标记与建表 SQL 占位符。"""
    if not open_builder(page, "M3_B1"):
        return {
            "object_missing": "构建器里没有「目标表建表 SQL」卡（`column-fetch-title`）——"
                              "整段在 47a2fed 被摘掉；所有者 2026-08-21 裁定判废（ADR-0043），"
                              "W3 / W4 已写 N/A",
            "column_fetch_sections_on_screen": page.evaluate(
                "() => [...document.querySelectorAll('.column-fetch-section')]"
                ".map(el => el.getAttribute('aria-labelledby'))"),
            "fetch_ready_present": page.query_selector(".fetch-ready") is not None,
        }
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
    # `.ddl-output` 在 v1 有**两处**：构建器上方的「生成的 SQL」也用这个类
    # （`App.tsx` 的 `GeneratedSql`）。裸选命中的是它——9 行源 SQL，不是建表 SQL。
    # 作用域收进 `.fetch-ready` 才是取列卡里那一份。
    placeholders = page.query_selector_all(".fetch-ready .ddl-placeholder")
    ddl = page.query_selector(".fetch-ready .ddl-output").inner_text()
    # 模态框有自己的滚动条：不先把 DDL 区块滚进来，截出来的只是构建器上半截，
    # W4 那条「整份 DDL 照给」在图上无从看起。
    page.query_selector(".fetch-ready .ddl-output").scroll_into_view_if_needed()
    page.screenshot(path=f"{SHOTS}/w3-w4-column-fetch.png", full_page=True)
    return {
        "type_column": [c.inner_text() for c in type_cells],
        "marked_cells": marks,
        "mark_element_tags": [c.evaluate("(el) => el.tagName") for c in type_cells],
        "placeholders": [p.inner_text() for p in placeholders],
        "ddl_given_in_full": ddl.strip().endswith("DEFAULT CHARSET=utf8mb4;"),
        "ddl_lines": ddl.count("\n") + 1,
        "ddl_first_line": ddl.strip().splitlines()[0] if ddl.strip() else None,
        "ddl_last_line": ddl.strip().splitlines()[-1] if ddl.strip() else None,
    }


def observe_rejected_fetch(page):
    """W5：白名单外的列 —— 列表照给，只有 DDL 区块换成「整份不给」。"""
    if not open_builder(page, "REJECTED"):
        return {
            "object_missing": "同 W3 / W4：取列卡不存在，W5 的第四态无从制造；判据已判废",
        }
    listed = [c.inner_text() for c in page.query_selector_all(".fetch-ready tbody tr td:nth-child(1)")]
    crit = page.query_selector(".row-size-warning.is-crit")
    if crit is not None:
        crit.scroll_into_view_if_needed()
    page.screenshot(path=f"{SHOTS}/w5-rejected.png", full_page=True)
    return {
        "columns_still_listed": listed,
        # 同上：判「DDL 区块换成整份不给」只能看取列卡里那一份。
        "ddl_block_present": page.query_selector(".fetch-ready .ddl-output") is not None,
        "crit_block_text": crit.inner_text() if crit else None,
    }


def text_or_absent(page, selector):
    """取一格的文字；这一格不在场就如实回一条缺席，**不抛**。

    走查记录要看得见「它没了」，而不是看见一个堆栈——一个 `AttributeError`
    会把后面所有判据一起带走，那才是最贵的失败方式。
    """
    node = page.query_selector(selector)
    return node.inner_text() if node is not None else {"object_missing": selector}


def observe_builder_surface(page):
    """本票新增的构建器面（条件 / 排序 / 只读 SQL），走查清单之外的旁证。"""
    page.set_viewport_size({"width": 1440, "height": 1400})
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector("#jobs tbody tr")
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
        # 过滤条件已改成一个自由 WHERE 文本框（ADR-0047 §1）——原来那套四格表单的
        # `.condition-row` 恒为 0，量的是「旧控件真的一个不剩」。
        "condition_rows": len(page.query_selector_all(".condition-row")),
        "where_clause": text_or_absent(page, ".where-clause-editor textarea"),
        "sql_is_readonly": page.query_selector(".generated-sql textarea") is None,
        "sql_text": page.query_selector(".generated-sql pre").inner_text(),
        # 这一格可能**合法地缺席**，缺席时不许抛：探针只观察不断言（ADR-0028 §1）。
        # `.run-parameter-list` 整块已随运行参数链退役（ADR-0047 §3），恒为缺席。
        # 抛出去的话，整份 W1–W6 会以一个 AttributeError 收场，而判据一条都没跑。
        "run_parameters": text_or_absent(page, ".run-parameter-list"),
        "primary_key_boxes": len(page.query_selector_all('.builder-columns input[aria-label*="主键"]')),
        "key_note": text_or_absent(page, ".builder-key-note"),
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
