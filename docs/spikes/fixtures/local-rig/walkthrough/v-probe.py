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


JOB = "#jobs"


def open_tasks(page):
    """打开作业中心。屏的 id 自 ADR-0043 §2 起是 `#jobs`（任务屏与运行历史屏已合并）。"""
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector(f"{JOB} tbody tr", timeout=20000)


def open_drawer(page, task_name):
    """点某一行的「运行详情」图标开抽屉——轴二 / 轴三自 ADR-0043 §4 起在这里看。"""
    open_tasks(page)
    for row in page.query_selector_all(f"{JOB} tbody tr"):
        cell = row.query_selector("td:nth-child(2)")
        if cell is not None and task_name in cell.inner_text():
            button = row.query_selector('button[aria-label="运行详情"]')
            if button is None:
                return False
            button.click()
            page.wait_for_selector(".drawer", timeout=10000)
            # 指针必须挪开：`.data-grid tbody tr:hover td` 会换底色，V5 量的正好是底色。
            page.mouse.move(0, 0)
            page.wait_for_timeout(150)
            return True
    return False


def start_run(page, load_date):
    """从任务列表发起一个 run，停在 RunScreen 上。日期即造态开关（见 v-mock.py）。"""
    open_tasks(page)
    page.click(f'{JOB} tbody tr button[aria-label="发起运行"]')
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
    page.click(f'{JOB} tbody tr button[aria-label="发起运行"]')
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


# ---- 二、详情抽屉：V2 / V3 / V4 / V5 / V25（原历史列表，ADR-0043 §2 已并入作业中心） ----

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


def drawer_walk(page, out):
    """V2 / V3 / V4 / V5 / V25 在**详情抽屉**里看（ADR-0043 §4）。

    2026-08-21 之前这一段叫 `history_walk`，对着运行历史列表看。那一屏已随 ADR-0043 §2
    整屏并入作业中心，于是：

    * **V9 / V14 退役**——判据的对象（历史列表的「结局」列、`run_record_id` 主列）没了；
      V9 的方向还反转了（作业中心的「运行状态」是一维索引，五个词齐是对的，由 X17 守）。
    * **轴二的形状判据整体移交抽屉**：SWAPPED 实心块 / DISCARDED 描边块一个没变，
      只是要开两个任务的抽屉才凑得齐——一行只展示最近一次运行，一个抽屉里只有一种终态。
    * **V5 的灰度取样因此分两张截图**：判据量的是两个块各自的块内中位亮度差，
      不要求它们同屏；同屏是旧形态的副产物，不是判据本身。
    """
    blocks = {}
    for name, task, selector in (
        ("swapped", "成功那条", ".drawer .terminal-block.is-swapped"),
        ("discarded", "校验失败那条", ".drawer .terminal-block.is-discarded"),
    ):
        if not open_drawer(page, task):
            blocks[name] = {"missing": f"打不开「{task}」的抽屉"}
            continue
        out[f"V{'2' if name == 'swapped' else '3'}_in_drawer"] = {
            "panels": texts(page, ".drawer .panel > h3"),
            "terminal_blocks": probe_all(page, ".drawer .terminal-block"),
            "terminal_text": texts(page, ".drawer .terminal-block"),
            "error_codes": probe_all(page, ".drawer .error-code"),
            "error_text": texts(page, ".drawer .error-code"),
            "counts": counts(page),
        }
        el = page.query_selector(selector)
        if el is None:
            blocks[name] = {"missing": f"抽屉里没有 {selector}"}
        else:
            rect = el.bounding_box()
            page.add_style_tag(content="html { filter: grayscale(1) !important; }")
            page.wait_for_timeout(200)
            shot = page.screenshot(full_page=True)
            blocks[name] = median_luma(shot, rect, 2, 3, 6, 14)
            blocks[name]["rect"] = rect
            page.screenshot(path=f"{SHOTS}/v5-grayscale-{name}.png", full_page=True)
        page.screenshot(path=f"{SHOTS}/v2-v3-drawer-{name}.png", full_page=True)
        page.reload(wait_until="networkidle")

    if "median" in blocks.get("swapped", {}) and "median" in blocks.get("discarded", {}):
        diff = blocks["discarded"]["median"] - blocks["swapped"]["median"]
        blocks["diff_median"] = diff
        blocks["diff_pct"] = round(diff / 255 * 100, 1)
        blocks["passes_25_over_255_bar"] = diff >= 25
    blocks["sampling_note"] = "两块各自开一个抽屉取样（一行只展示最近一次运行）"
    out["V5"] = blocks

    # V4 的 4xx 例子：校验失败那条抽屉里的 VERIFY_FAILED。
    open_drawer(page, "校验失败那条")
    out["V4_in_drawer"] = {
        "error_codes": probe_all(page, ".drawer .error-code"),
        "error_text": texts(page, ".drawer .error-code"),
        "conclusion": texts(page, ".drawer .error-summary, .drawer .plain-conclusion"),
    }

    # V15 兼守原 V14：两个 id 谁也不替代谁——`run_record_id` 在标题旁，
    # `run_id`（「目标端运行号」）在「运行参数与标识」里。
    out["V14_V15_two_ids"] = page.evaluate(
        """() => { const d = document.querySelector('.drawer');
             const sub = d.querySelector('.drawer-header .sub');
             const kv = [...d.querySelectorAll('.kv > div')]
               .map(el => [el.querySelector('.k').innerText, el.querySelector('.v').innerText]);
             const runId = kv.find(([k]) => k === '目标端运行号');
             return { title: d.querySelector('h2').innerText,
                      run_record_id_beside_title: sub ? sub.innerText : null,
                      run_record_id_style: sub ? { color: getComputedStyle(sub).color,
                        font: getComputedStyle(sub).fontFamily.split(',')[0],
                        size: getComputedStyle(sub).fontSize } : null,
                      run_id_field: runId || null }; }""")

    out["V25"] = {
        "body": page.evaluate(
            "() => { const cs = getComputedStyle(document.body);"
            " return {fontFamily: cs.fontFamily, colorScheme: cs.colorScheme,"
            " bg: cs.backgroundColor}; }"),
        "mono": page.evaluate(
            "() => { const el = document.querySelector('.drawer .kv .v');"
            " const cs = getComputedStyle(el);"
            " return {fontFamily: cs.fontFamily, variantNumeric: cs.fontVariantNumeric}; }"),
        # V25 改判（ADR-0043 §走查触发）：强调字重从「600 不是 700」改成「500」，
        # 取值来自对 x2doris 表头 / 标题块的实测。
        "emphasis_weight": page.evaluate(
            "() => getComputedStyle(document.querySelector('#jobs thead th')).fontWeight"),
        "title_block_weight": page.evaluate(
            "() => getComputedStyle(document.querySelector('#jobs .table-title')).fontWeight"),
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
        # 深色侧栏**不是暗色主题**（ADR-0043 §8）：它是参照物浅色布局的一部分。
        # 这一格摆在这里，是为了让「没有暗色主题」这句话下次仍然量得出来。
        "sider_is_not_a_dark_theme": page.evaluate(
            "() => ({ sider_bg: getComputedStyle(document.querySelector('aside.sidebar')).backgroundColor,"
            " content_bg: getComputedStyle(document.querySelector('.content')).backgroundColor,"
            " card_bg: getComputedStyle(document.querySelector('#jobs')).backgroundColor })"),
    }
    page.screenshot(path=f"{SHOTS}/v25-drawer-typography.png", full_page=True)


# ---- 三、任务屏与构建器：V19 / V20 / V23 / V24 ------------------------------

def tasks_and_builder(page, out):
    open_tasks(page)
    out["V24"] = {
        # 改判（ADR-0043 §2 §8）：导航三项、数据源第二项、侧栏深色；
        # 「调度只是占位灰标 M3+」的对象在 P0 已被撤掉（ADR-0042 §背景），本次补记。
        "sidebar_bg": page.evaluate(
            "() => getComputedStyle(document.querySelector('aside.sidebar')).backgroundColor"),
        "collapsed_icon_centering": page.evaluate(
            """() => { const shell = document.querySelector('.app-shell');
                 const before = document.querySelector('.nav-item.is-active').getBoundingClientRect();
                 shell.classList.add('is-collapsed');
                 const item = document.querySelector('.nav-item.is-active');
                 const r = item.getBoundingClientRect();
                 const svg = item.querySelector('svg').getBoundingClientRect();
                 const out = { expanded_block_width: before.width, collapsed_block_width: r.width,
                   sider_width: document.querySelector('aside.sidebar').getBoundingClientRect().width,
                   icon_center_offset: (svg.left + svg.width / 2) - (r.left + r.width / 2),
                   text_hidden: getComputedStyle(item.querySelector('.nav-text')).display };
                 shell.classList.remove('is-collapsed');
                 return out; }"""),
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

    page.click(f'{JOB} tbody tr button[aria-label="编辑任务定义"]')
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
    #
    # 2026-08-21：这一卡在界面上**已经不存在**了。不是本次改动删的——
    # `47a2fed`（"Prepare x2doris P1 frontend handoff"）把整段
    # 「目标表建表 SQL / 拿建表 SQL / .fetch-ready」从构建器里摘掉了，而那一票没跑走查。
    # 探针只观察不断言：对象没了就如实记一条，不抛错、更不假装跑过。
    target_input = page.query_selector(".modal input[list]")
    target_input.fill("")
    fetch_button = page.query_selector('.modal .column-fetch-section button:has-text("拿建表 SQL")')
    if fetch_button is None:
        missing = {
            "object_missing": "构建器里没有「拿建表 SQL」按钮与 .fetch-ready 区块——"
                              "整段在 47a2fed 被摘掉；所有者 2026-08-21 裁定判废（ADR-0043），"
                              "V19 / V20 已写 N/A",
            "column_fetch_sections_on_screen": page.evaluate(
                "() => [...document.querySelectorAll('.modal .column-fetch-section')]"
                ".map(el => el.getAttribute('aria-labelledby'))"),
        }
        out["V19"] = missing
        out["V20"] = missing
        page.screenshot(path=f"{SHOTS}/v19-v20-object-missing.png", full_page=True)
        return
    fetch_button.click()
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
        drawer_walk(page, out)
        tasks_and_builder(page, out)
        browser.close()

    # 判据已退役的四条半：对象不存在，报 retired 并写清是谁把它判废的。
    out["RETIRED"] = {
        "V6": "形状预检失败这一态随 ADR-0036 §5 取消，屏不存在",
        "V9": "运行历史列表随 ADR-0043 §2 整屏并入作业中心；且方向已反转——"
              "作业中心的「运行状态」是一维索引，五个词齐是对的，形态判据改由 X17 守",
        "V14": "同上，历史列表没了；「两个 id 谁也不替代谁」由 V15 在抽屉里兼守，"
               "实测见 V14_V15_two_ids",
        "V10": "两张卡与灰色「未执行」占位卡随同一条取消；#132 已撤 .is-skipped",
        "V11_first_card": "形状预检那张卡（六条逐条列出）随同一条取消；第二张卡照跑，见 V11_mapping_card",
        "V12": "形状预检屏不存在，「这一屏没有错误码标签」无对象",
        "V18": "源端 SQL 随 ADR-0036 §2 改为由规格现算、只读，手改入口与确认模态一并没了",
        "V21": "ADR-0038 §3 / ADR-0039 §5 明文开出目标表下拉与目标列参考表，判据方向已反",
        "V19": "构建器的建表 SQL 区块随 47a2fed 摘掉，所有者 2026-08-21 裁定判废（ADR-0043）；"
               "本函数仍会去取一次并如实回一条 object_missing，别把它读成崩了",
        "V20": "同 V19：取列卡与它的两句说明一起没了对象",
    }
    print(json.dumps(out, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
