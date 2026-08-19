#!/usr/bin/env python3
"""整份 V1–V25 的机器观察（配 `v-mock.py` 的桩后端），#133。

按 ADR-0028 §1 的先例：**只观察，不断言**；一行 DOM 断言都不进验收套件。
输出是给人抄进走查记录的实际观察，不是 pass/fail。

**判据已退役的条目照实报 `retired`，连同「对象为什么不存在」一起**——
不许报「通过」，也不许静悄悄跳过（ADR-0039 §9 / ADR-0040 §6.1）。

用法：~/pwvenv/bin/python docs/spikes/fixtures/local-rig/walkthrough/v-probe.py [port]
"""

import json
import statistics
import sys
from io import BytesIO

from playwright.sync_api import sync_playwright

BASE = f"http://127.0.0.1:{sys.argv[1] if len(sys.argv) > 1 else 18097}"
SHOTS = "/tmp/v-visual"

STYLE_PROBE = """
(el) => {
  const cs = getComputedStyle(el);
  const r = el.getBoundingClientRect();
  return {
    text: (el.innerText || '').replace(/\\n+/g, ' | '),
    className: el.className,
    background: cs.backgroundColor,
    color: cs.color,
    borderStyle: cs.borderTopStyle,
    borderColor: cs.borderTopColor,
    borderWidth: cs.borderTopWidth,
    rect: { x: r.x, y: r.y, w: r.width, h: r.height },
  };
}
"""


def probe_all(page, selector):
    return [el.evaluate(STYLE_PROBE) for el in page.query_selector_all(selector)]


def counts(page):
    return {
        "terminal_block": len(page.query_selector_all(".terminal-block")),
        "error_code": len(page.query_selector_all(".error-code")),
        "phase_item": len(page.query_selector_all(".phase-item")),
    }


def texts(page, selector):
    return [el.inner_text().replace("\n", " | ") for el in page.query_selector_all(selector)]


def open_tasks(page):
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector("#tasks tbody tr", timeout=20000)


def start_run(page, load_date):
    """从任务列表发起一个 run，停在 RunScreen 上。日期即造态开关（见 v-mock.py）。"""
    open_tasks(page)
    page.click('#tasks tbody tr button[aria-label="发起运行"]')
    page.wait_for_selector(".modal.is-narrow", timeout=10000)
    page.fill(".modal.is-narrow input[type=date]", load_date)
    page.click(".modal.is-narrow button[type=submit]")
    page.wait_for_selector(".run-card", timeout=20000)
    page.wait_for_selector(".live-state, .run-result", timeout=20000)


# ---- 一、三条形状轴 ---------------------------------------------------------

def v1_live(page, out):
    start_run(page, "2026-01-01")
    page.wait_for_selector(".phase-item.is-current", timeout=15000)
    out["V1"] = {
        "phase_items": page.evaluate(
            """() => Array.from(document.querySelectorAll('.phase-item')).map(item => {
                 const dot = item.querySelector('.phase-dot');
                 const cs = dot ? getComputedStyle(dot) : null;
                 return { text: item.innerText.replace(/\\n/g, ' '), cls: item.className,
                          dot: cs ? { w: cs.width, h: cs.height, radius: cs.borderRadius,
                                      bg: cs.backgroundColor } : null };
               })"""),
        "phase_after": page.eval_on_selector(".phase-after", "el => el.innerText"),
        "phase_after_dot_count": page.evaluate(
            "() => document.querySelectorAll('.phase-after .phase-dot').length"),
        "counts": counts(page),
        "conclusion": page.eval_on_selector(".live-state strong", "el => el.innerText"),
        "metrics": texts(page, ".run-metrics > div"),
        "identity": texts(page, ".run-identity > div"),
    }
    page.screenshot(path=f"{SHOTS}/v1-live.png", full_page=True)

    # V17 就在这一屏上：取消按钮常亮，点下去当场如实回话。
    button = page.query_selector(".run-header > button")
    out["V17"] = {
        "button": probe_all(page, ".run-header > button"),
        "disabled": button.is_disabled(),
        "cursor": button.evaluate("el => getComputedStyle(el).cursor"),
        "pointer_events": button.evaluate("el => getComputedStyle(el).pointerEvents"),
    }
    button.click()
    page.wait_for_selector(".run-notice", timeout=10000)
    out["V17"]["notice_on_streaming"] = texts(page, ".run-notice")


def v17_accepted(page, out):
    """V17 的另一半：run 还没进可取消阶段时，按钮仍不禁用，回的是如实拒绝。"""
    start_run(page, "2026-01-02")
    button = page.query_selector(".run-header > button")
    out["V17"]["accepted_state"] = {
        "conclusion": page.eval_on_selector(".live-state strong", "el => el.innerText"),
        "phase_item_classes": page.evaluate(
            "() => Array.from(document.querySelectorAll('.phase-item')).map(e => e.className.trim())"),
        "button_text": button.inner_text(),
        "disabled": button.is_disabled(),
    }
    button.click()
    page.wait_for_selector(".run-notice", timeout=10000)
    out["V17"]["accepted_state"]["notice"] = texts(page, ".run-notice")


def v2_success(page, out):
    start_run(page, "2026-01-03")
    out["V2"] = {
        "counts": counts(page),
        "terminal_block": probe_all(page, ".terminal-block"),
        "result_text": page.eval_on_selector(".run-result", "el => el.innerText.replace(/\\n+/g, ' | ')"),
        "metrics": texts(page, ".run-metrics > div"),
    }
    page.screenshot(path=f"{SHOTS}/v2-success.png", full_page=True)


def v3_discarded(page, out):
    start_run(page, "2026-01-04")
    out["V3"] = {
        "counts": counts(page),
        "terminal_block": probe_all(page, ".terminal-block"),
        "result_text": page.eval_on_selector(".run-result", "el => el.innerText.replace(/\\n+/g, ' | ')"),
    }


def v4_v7_v11_v22_mapping(page, out):
    start_run(page, "2026-01-05")
    out["V4_on_run_screen"] = {
        "error_summary_children": page.evaluate(
            """() => Array.from(document.querySelectorAll('.error-summary > *')).map(el => ({
                 cls: el.className, tag: el.tagName,
                 text: el.innerText.replace(/\\n/g, ' '),
                 x: Math.round(el.getBoundingClientRect().x) }))"""),
        "error_code_style": probe_all(page, ".error-code"),
    }
    out["V7"] = {"counts": counts(page)}
    out["V11_mapping_card"] = {
        "sections": len(page.query_selector_all(".precheck-reports > section")),
        "section_classes": page.evaluate(
            "() => Array.from(document.querySelectorAll('.precheck-reports > section')).map(s => s.className)"),
        "header": texts(page, ".precheck-reports section header"),
        "columns": texts(page, ".diagnostic-table thead th"),
        "rows": page.evaluate(
            """() => Array.from(document.querySelectorAll('.diagnostic-table tbody tr'))
                 .map(tr => Array.from(tr.querySelectorAll('td')).map(td => td.innerText))"""),
        "total_line": page.eval_on_selector(".precheck-reports small", "el => el.innerText"),
        "skipped_placeholder_cards": len(page.query_selector_all(".precheck-reports .is-skipped")),
    }
    out["V22"] = {
        "exit_text": page.eval_on_selector(".precheck-exit", "el => el.innerText.replace(/\\n+/g, ' | ')"),
        "exit_buttons": texts(page, ".precheck-exit button"),
        "create_table_hits": page.evaluate(
            "() => (document.body.innerText.match(/CREATE TABLE/g) || []).length"),
    }
    out["V23_run_screen"] = page.evaluate(
        """() => ({ retry: (document.body.innerText.match(/重试/g) || []).length,
                    relaunch: (document.body.innerText.match(/重新发起/g) || []).length })""")
    page.screenshot(path=f"{SHOTS}/v4-v11-v22-mapping.png", full_page=True)


def v8_unknown(page, out):
    start_run(page, "2026-01-06")
    out["V8"] = {
        "counts": counts(page),
        "result_text": page.eval_on_selector(".run-result", "el => el.innerText.replace(/\\n+/g, ' | ')"),
        "unknown_conclusion": probe_all(page, ".unknown-conclusion"),
    }


def v13_escape(page, out):
    start_run(page, "2026-01-07")
    page.wait_for_selector(".sensitive-value", timeout=10000)
    out["V13"] = {
        "masked": page.evaluate(
            """() => { const box = document.querySelector('.sensitive-value');
                 const dl = box.querySelector('dl'); const dd = box.querySelector('dd');
                 const cs = getComputedStyle(dd);
                 return { full: box.innerText.replace(/\\n+/g, ' | '),
                          dlClass: dl ? dl.className : null, filter: cs.filter,
                          userSelect: cs.userSelect,
                          button: (box.querySelector('button') || {}).innerText || null }; }"""),
    }
    page.click(".sensitive-value button")
    page.wait_for_timeout(200)
    out["V13"]["revealed"] = page.evaluate(
        """() => { const box = document.querySelector('.sensitive-value');
             const dl = box.querySelector('dl'); const dd = box.querySelector('dd');
             const cs = getComputedStyle(dd);
             return { dlClass: dl ? dl.className : null, filter: cs.filter,
                      userSelect: cs.userSelect, text: box.innerText.replace(/\\n+/g, ' | '),
                      button: (box.querySelector('button') || {}).innerText || null }; }""")
    out["V4_5xx_on_run_screen"] = probe_all(page, ".error-code")
    page.screenshot(path=f"{SHOTS}/v13-sensitive.png", full_page=True)


def v15_not_started(page, out):
    start_run(page, "2026-01-08")
    out["V15"] = {
        "identity": texts(page, ".run-identity > div"),
        "counts": counts(page),
        "conclusion": page.eval_on_selector(".run-result", "el => el.innerText.replace(/\\n+/g, ' | ')"),
    }


def v16_stale_hint(page, out):
    open_tasks(page)
    page.click('#tasks tbody tr button[aria-label="发起运行"]')
    page.wait_for_selector(".modal.is-narrow", timeout=10000)
    page.fill(".modal.is-narrow input[type=date]", "2026-01-01")
    page.wait_for_selector(".stale-run-hint", timeout=10000)
    submit = page.query_selector(".modal.is-narrow button[type=submit]")
    out["V16"] = {
        "hint": probe_all(page, ".stale-run-hint"),
        "hint_left_border": page.eval_on_selector(
            ".stale-run-hint",
            "el => { const cs = getComputedStyle(el); return {w: cs.borderLeftWidth,"
            " color: cs.borderLeftColor, bg: cs.backgroundColor}; }"),
        "submit_text": submit.inner_text(),
        "submit_disabled": submit.is_disabled(),
        "submit_cursor": submit.evaluate("el => getComputedStyle(el).cursor"),
    }
    page.screenshot(path=f"{SHOTS}/v16-stale-hint.png", full_page=True)
    page.click('.modal.is-narrow button[type="button"]')


# ---- 二、历史列表：V5 / V9 / V14 / V25 --------------------------------------

def median_luma(png_bytes, rect, inset_x, inset_y, w, h):
    """块内取一小片的中位亮度（PIL 'L' = ITU-R 601-2）。"""
    from PIL import Image
    img = Image.open(BytesIO(png_bytes)).convert("L")
    x0 = int(rect["x"]) + inset_x
    y0 = int(rect["y"]) + inset_y
    patch = img.crop((x0, y0, x0 + w, y0 + h))
    px = list(patch.getdata())
    return {"median": statistics.median(px), "min": min(px), "max": max(px),
            "sample": f"{w}x{h} @({x0},{y0})"}


def history_walk(page, out):
    page.goto(f"{BASE}/#history", wait_until="networkidle")
    page.wait_for_selector(".history-grid tbody tr", timeout=15000)
    # 指针必须挪开：`.data-grid tbody tr:hover td` 会把某一行的底色从纸白换成 --mute-bg，
    # V5 量的又正好是块下面透出来的底色——不挪开量到的是悬停态，不是常态。
    page.mouse.move(0, 0)
    page.wait_for_timeout(100)

    out["V14"] = {
        "columns": texts(page, ".history-grid thead th"),
        "first_col": probe_all(page, ".history-grid tbody tr td:first-child .history-link")[:1],
        "run_id_cells": probe_all(page, ".history-grid tbody .run-id-cell")[:2],
        "missing_run_id_cells": probe_all(page, ".history-grid tbody .missing-run-id"),
    }
    rows = []
    for tr in page.query_selector_all(".history-grid tbody tr:not(.history-detail-row)"):
        tds = tr.query_selector_all("td")
        if len(tds) < 6:
            continue
        cell = tds[4]
        kind = ("block" if cell.query_selector(".terminal-block")
                else "neutral-text" if cell.query_selector(".neutral-outcome")
                else "unknown-summary" if cell.query_selector(".unknown-summary")
                else "live-summary" if cell.query_selector(".live-summary") else "?")
        box = cell.query_selector("span").bounding_box()
        rows.append({"run_record_id": tds[0].inner_text().strip(),
                     "run_id_cell": tds[1].inner_text().strip(),
                     "outcome_text": cell.inner_text().replace("\n", " | ").strip(),
                     "outcome_kind": kind,
                     "outcome_w": round(box["width"], 1) if box else None,
                     "outcome_h": round(box["height"], 1) if box else None,
                     "error_cell": tds[5].inner_text().strip()})
    out["V9"] = {"rows": rows,
                 "neutral_outcomes": probe_all(page, ".history-grid .neutral-outcome"),
                 "unknown_summaries": probe_all(page, ".history-grid .unknown-summary"),
                 "live_summaries": probe_all(page, ".history-grid .live-summary")}
    out["V4_on_history"] = probe_all(page, ".history-grid .error-code")
    out["V2_on_history"] = probe_all(page, ".history-grid .terminal-block.is-swapped")
    out["V3_on_history"] = probe_all(page, ".history-grid .terminal-block.is-discarded")

    out["V25"] = {
        "body": page.evaluate(
            "() => { const cs = getComputedStyle(document.body);"
            " return {fontFamily: cs.fontFamily, colorScheme: cs.colorScheme,"
            " bg: cs.backgroundColor}; }"),
        "mono": page.evaluate(
            "() => { const el = document.querySelector('.history-grid td.mono');"
            " const cs = getComputedStyle(el);"
            " return {fontFamily: cs.fontFamily, variantNumeric: cs.fontVariantNumeric}; }"),
        "weights": page.evaluate(
            "() => Array.from(new Set(Array.from(document.querySelectorAll('*'))"
            ".map(el => getComputedStyle(el).fontWeight))).sort()"),
        "dark_media_matches": page.evaluate(
            "() => window.matchMedia('(prefers-color-scheme: dark)').matches"),
        "dark_conditional_rules": page.evaluate(
            "() => [...document.styleSheets].flatMap(s => { try { return [...s.cssRules] }"
            " catch { return [] } }).filter(r => (r.conditionText || '')"
            ".includes('prefers-color-scheme')).length"),
        "top_level_rules": page.evaluate(
            "() => [...document.styleSheets].flatMap(s => { try { return [...s.cssRules] }"
            " catch { return [] } }).length"),
    }

    swapped = page.query_selector(".history-grid .terminal-block.is-swapped")
    discarded = page.query_selector(".history-grid .terminal-block.is-discarded")
    neutral = page.query_selector(".history-grid .neutral-outcome")
    page.add_style_tag(content="html { filter: grayscale(1) !important; }")
    page.wait_for_timeout(200)
    shot = page.screenshot(full_page=True)
    v5 = {}
    for name, el in (("swapped", swapped), ("discarded", discarded),
                     ("neutral_text_cell", neutral)):
        if el is None:
            continue
        rect = el.bounding_box()
        v5[name] = median_luma(shot, rect, 2, 3, 6, 14)
        v5[name]["rect"] = rect
    if "swapped" in v5 and "discarded" in v5:
        diff = v5["discarded"]["median"] - v5["swapped"]["median"]
        v5["diff_median"] = diff
        v5["diff_pct"] = round(diff / 255 * 100, 1)
        v5["passes_25_over_255_bar"] = diff >= 25
    out["V5"] = v5
    page.screenshot(path=f"{SHOTS}/v5-grayscale.png", full_page=True)
    page.reload(wait_until="networkidle")
    page.wait_for_selector(".history-grid tbody tr", timeout=15000)
    page.screenshot(path=f"{SHOTS}/v9-v14-history.png", full_page=True)


# ---- 三、任务屏与构建器：V19 / V20 / V23 / V24 ------------------------------

def tasks_and_builder(page, out):
    open_tasks(page)
    out["V24"] = {
        "sidebar_items": page.evaluate(
            """() => Array.from(document.querySelectorAll('aside.sidebar nav[aria-label="主导航"] > *'))
                 .map(el => ({ tag: el.tagName, cls: el.className,
                               text: (el.innerText || '').replace(/\\n/g, ' '),
                               color: getComputedStyle(el).color }))"""),
        "builder_hits_in_nav": page.evaluate(
            """() => (document.querySelector('aside.sidebar nav').innerText.match(/构建器/g) || []).length"""),
        "badges": texts(page, ".nav-badge"),
    }
    out["V23_tasks_page"] = page.evaluate(
        """() => ({ retry: (document.body.innerText.match(/重试/g) || []).length,
                    relaunch: (document.body.innerText.match(/重新发起/g) || []).length })""")

    page.click('#tasks tbody tr button[aria-label="编辑任务定义"]')
    page.wait_for_selector(".modal .builder-guide", timeout=15000)

    # V21 的对象：目标端下拉与目标列列表——ADR-0038 §3 / ADR-0039 §5 之后**已经建成**，
    # 判据反了，照实记数。
    out["V21_object_now"] = page.evaluate(
        """() => { const m = document.querySelector('.modal');
             return { datalist_count: m.querySelectorAll('datalist').length,
                      input_list_count: m.querySelectorAll('input[list]').length,
                      target_side_note: (m.querySelector('.target-side-note') || {}).innerText || null,
                      not_drawn_copy_hits: (m.innerText.match(/是不画/g) || []).length }; }""")

    # V18 的对象：源端 SQL 现在由规格现算、只读，没有手改入口。
    out["V18_object_now"] = page.evaluate(
        """() => { const m = document.querySelector('.modal');
             const sec = m.querySelector('.generated-sql');
             return { textarea_count: m.querySelectorAll('textarea').length,
                      generated_sql_header: sec ? sec.querySelector('header').innerText.replace(/\\n+/g, ' | ') : null,
                      manual_edit_badge_hits: (m.innerText.match(/已被手改/g) || []).length,
                      rewizard_hits: (m.innerText.match(/重走向导/g) || []).length }; }""")

    # V19 / V20：target_table 清空后取列，DDL 照给、占位符可见。
    target_input = page.query_selector(".modal input[list]")
    target_input.fill("")
    page.click('.modal .column-fetch-section button:has-text("拿建表 SQL")')
    page.wait_for_selector(".modal .fetch-ready, .modal .fetch-failure", timeout=30000)
    out["V19"] = {
        "panel_kind": page.evaluate(
            "() => document.querySelector('.modal .fetch-ready') ? 'ready'"
            " : (document.querySelector('.modal .fetch-failure') || {}).innerText || 'none'"),
        "placeholder": probe_all(page, ".modal .ddl-placeholder"),
        "ddl_text": page.evaluate(
            "() => (document.querySelector('.modal .fetch-ready .ddl-output') || {}).innerText || null"),
    }
    out["V20"] = {
        "scope_note": page.evaluate(
            "() => (document.querySelector('.modal .fetch-scope-note') || {}).innerText || null"),
        "row_size_warning": page.evaluate(
            """() => { const el = document.querySelector('.modal .row-size-warning');
                 return el ? el.innerText.replace(/\\n+/g, ' | ') : null; }"""),
        "columns": page.evaluate(
            """() => Array.from(document.querySelectorAll('.modal .fetch-ready .data-grid tbody tr'))
                 .map(tr => Array.from(tr.querySelectorAll('td')).map(td => td.innerText))"""),
    }
    page.screenshot(path=f"{SHOTS}/v19-v20-ddl.png", full_page=True)


def main():
    import os
    os.makedirs(SHOTS, exist_ok=True)
    out = {}
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch()
        page = browser.new_page(viewport={"width": 1440, "height": 1000},
                                device_scale_factor=1)
        v1_live(page, out)
        v17_accepted(page, out)
        v2_success(page, out)
        v3_discarded(page, out)
        v4_v7_v11_v22_mapping(page, out)
        v8_unknown(page, out)
        v13_escape(page, out)
        v15_not_started(page, out)
        v16_stale_hint(page, out)
        history_walk(page, out)
        tasks_and_builder(page, out)
        browser.close()

    # 判据已退役的四条半：对象不存在，报 retired 并写清是谁把它判废的。
    out["RETIRED"] = {
        "V6": "形状预检失败这一态随 ADR-0036 §5 取消，屏不存在",
        "V10": "两张卡与灰色「未执行」占位卡随同一条取消；#132 已撤 .is-skipped",
        "V11_first_card": "形状预检那张卡（六条逐条列出）随同一条取消；第二张卡照跑，见 V11_mapping_card",
        "V12": "形状预检屏不存在，「这一屏没有错误码标签」无对象",
        "V18": "源端 SQL 随 ADR-0036 §2 改为由规格现算、只读，手改入口与确认模态一并没了",
        "V21": "ADR-0038 §3 / ADR-0039 §5 明文开出目标表下拉与目标列参考表，判据方向已反",
    }
    print(json.dumps(out, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
