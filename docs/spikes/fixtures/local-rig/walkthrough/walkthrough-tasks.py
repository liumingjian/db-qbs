#!/usr/bin/env python3
"""M2 渲染面走查 · 任务屏 / 建任务对话框 / 导航的机器观察（V18~V21、V23、V24）。

同 walkthrough-history.py：只观察，不断言（ADR-0028 §1）。

V19 / V20 要真取一次列（走 Oracle），所以拿一条 SQL 本来就合法的既有任务（`A2 取列`）
开「编辑四个字段」对话框，把 target_table 清空后再点「拿建表 SQL」。
"""

import json
import sys

from playwright.sync_api import sync_playwright

BASE = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:18088"
OUT = sys.argv[2] if len(sys.argv) > 2 else "/tmp/m2-walkthrough-tasks.json"

STYLE_PROBE = """
(el) => {
  const cs = getComputedStyle(el);
  return {
    text: el.innerText,
    className: el.className,
    background: cs.backgroundColor,
    color: cs.color,
    border: cs.borderTopWidth + ' ' + cs.borderTopStyle + ' ' + cs.borderTopColor,
  };
}
"""


def probe_all(page, selector):
    return [el.evaluate(STYLE_PROBE) for el in page.query_selector_all(selector)]


def main():
    out = {}
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(viewport={"width": 1440, "height": 1000},
                                device_scale_factor=1)
        page.goto(BASE, wait_until="networkidle")
        page.wait_for_selector("#tasks tbody tr", timeout=20000)

        # ---- V24：主导航 ----
        out["V24_nav"] = page.evaluate(
            """() => {
                 const nav = document.querySelector('nav');
                 return { html_shape: Array.from(nav.querySelectorAll('*'))
                            .filter(el => el.className && typeof el.className === 'string')
                            .map(el => ({ tag: el.tagName, cls: el.className,
                                          text: (el.innerText || '').replace(/\\n/g, ' '),
                                          color: getComputedStyle(el).color })),
                          builder_hits: (nav.innerText.match(/构建器/g) || []).length };
               }"""
        )

        # ---- V23：任务屏措辞 ----
        out["V23_tasks_page"] = page.evaluate(
            """() => ({ retry: (document.body.innerText.match(/重试/g) || []).length,
                        relaunch: (document.body.innerText.match(/重新发起/g) || []).length })"""
        )

        # ---- 打开「A2 取列」的编辑对话框 ----
        row = page.locator("#tasks tbody tr").filter(has_text="A2 取列").first
        row.locator("button[aria-label='编辑四个字段']").click()
        page.wait_for_selector(".modal .builder-entry", timeout=15000)

        # ---- V21：目标端只有两个文本框 ----
        out["V21"] = page.evaluate(
            """() => {
                 const modal = document.querySelector('.modal');
                 return {
                   select_count: modal.querySelectorAll('select').length,
                   datalist_count: modal.querySelectorAll('datalist').length,
                   field_labels: Array.from(modal.querySelectorAll('.form-field > span, label > span'))
                                   .map(s => s.innerText.trim()).filter(Boolean),
                   target_side_note: (modal.querySelector('.target-side-note') || {}).innerText || null,
                 };
               }"""
        )

        # ---- V19 / V20：target_table 清空后取列 ----
        target_input = page.locator(".modal .field-grid .form-field").filter(
            has_text="target_table").locator("input")
        target_input.fill("")
        page.click(".modal .column-fetch-section button")
        page.wait_for_selector(".modal .fetch-ready, .modal .fetch-failure", timeout=90000)
        out["V19_V20_panel_kind"] = page.evaluate(
            """() => document.querySelector('.modal .fetch-ready') ? 'ready'
                     : (document.querySelector('.modal .fetch-failure') || {}).innerText || 'none'"""
        )
        out["V19_placeholder"] = probe_all(page, ".modal .ddl-placeholder")
        out["V19_ddl_text"] = page.evaluate(
            "() => (document.querySelector('.modal .ddl-output') || {}).innerText || null"
        )
        out["V20_scope_note"] = page.evaluate(
            "() => (document.querySelector('.modal .fetch-scope-note') || {}).innerText || null"
        )
        out["V20_row_size_warning"] = page.evaluate(
            """() => (document.querySelector('.modal .row-size-warning') || {}).innerText
                       ? document.querySelector('.modal .row-size-warning').innerText.replace(/\\n+/g, ' ')
                       : null"""
        )
        out["V20_columns"] = page.evaluate(
            """() => Array.from(document.querySelectorAll('.modal .fetch-ready .data-grid tbody tr'))
                 .map(tr => Array.from(tr.querySelectorAll('td')).map(td => td.innerText))"""
        )
        page.screenshot(path="/tmp/m2-v19-ddl.png", full_page=True)

        # ---- V18：手改 SQL 后的角标与确认框 ----
        textarea = page.locator(".modal textarea").first
        textarea.click()
        textarea.press("End")
        textarea.type(" ")
        page.wait_for_selector(".modal .field-badge", timeout=10000)
        out["V18_badge"] = probe_all(page, ".modal .field-badge")
        page.click(".modal .builder-entry button")
        page.wait_for_selector(".confirm-dialog", timeout=10000)
        out["V18_confirm"] = page.evaluate(
            """() => {
                 const d = document.querySelector('.confirm-dialog');
                 return { text: d.innerText.replace(/\\n+/g, ' | '),
                          buttons: Array.from(d.querySelectorAll('button')).map(b => b.innerText) };
               }"""
        )
        page.screenshot(path="/tmp/m2-v18-confirm.png", full_page=True)

        browser.close()

    with open(OUT, "w") as fh:
        json.dump(out, fh, ensure_ascii=False, indent=2)
    print(json.dumps(out, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
