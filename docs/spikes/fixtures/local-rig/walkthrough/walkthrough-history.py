#!/usr/bin/env python3
"""M2 渲染面走查 · 运行历史一屏的机器观察。

不是断言脚本，是**观察工具**：只取渲染文本、计算样式、几何位置与真实像素亮度，
产出一份 JSON 供人写走查记录。按 ADR-0028 §1，一行 DOM 断言都不进验收套件。

V5 那条按 #89 已改成可量判据：整页 grayscale(1) 后取块内中位亮度，差须 ≥ 25/255。
"""

import json
import statistics
import sys

from playwright.sync_api import sync_playwright

BASE = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:18088"
OUT = sys.argv[2] if len(sys.argv) > 2 else "/tmp/m2-walkthrough-history.json"

STYLE_PROBE = """
(el) => {
  const cs = getComputedStyle(el);
  const r = el.getBoundingClientRect();
  return {
    text: el.innerText,
    className: el.className,
    background: cs.backgroundColor,
    color: cs.color,
    borderStyle: cs.borderTopStyle,
    borderColor: cs.borderTopColor,
    borderWidth: cs.borderTopWidth,
    fontFamily: cs.fontFamily,
    fontWeight: cs.fontWeight,
    fontVariantNumeric: cs.fontVariantNumeric,
    rect: { x: r.x, y: r.y, w: r.width, h: r.height },
  };
}
"""


def probe_all(page, selector):
    return [el.evaluate(STYLE_PROBE) for el in page.query_selector_all(selector)]


def median_luma(png_bytes, rect, inset_x, inset_y, w, h):
    """块内取一小片的中位亮度（PIL 'L' = ITU-R 601-2）。"""
    from io import BytesIO

    from PIL import Image

    img = Image.open(BytesIO(png_bytes)).convert("L")
    # 截图按 CSS 像素取，devicePixelRatio 为 1 时坐标直接对应
    x0 = int(rect["x"]) + inset_x
    y0 = int(rect["y"]) + inset_y
    patch = img.crop((x0, y0, x0 + w, y0 + h))
    px = list(patch.getdata())
    return {
        "median": statistics.median(px),
        "min": min(px),
        "max": max(px),
        "sample": f"{w}x{h} @({x0},{y0})",
    }


def main():
    out = {}
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(viewport={"width": 1440, "height": 1000},
                                device_scale_factor=1)
        page.goto(f"{BASE}/#history", wait_until="networkidle")
        page.wait_for_selector(".history-grid tbody tr", timeout=15000)

        # ---- V9 / V14：列表层 ----
        out["V14_columns"] = [th.inner_text() for th in
                              page.query_selector_all(".history-grid thead th")]
        rows = []
        for tr in page.query_selector_all(".history-grid tbody tr:not(.history-detail-row)"):
            tds = tr.query_selector_all("td")
            if len(tds) < 6:
                continue
            outcome_cell = tds[4]
            block = outcome_cell.query_selector(".terminal-block")
            neutral = outcome_cell.query_selector(".neutral-outcome")
            rows.append({
                "run_record_id": tds[0].inner_text().strip(),
                "run_id_cell": tds[1].inner_text().strip(),
                "run_id_class": tds[1].get_attribute("class"),
                "outcome_text": outcome_cell.inner_text().strip(),
                "outcome_kind": "block" if block else ("neutral-text" if neutral else "?"),
                "error_cell": tds[5].inner_text().strip(),
            })
        out["V9_V14_rows"] = rows
        out["V14_first_col_link"] = probe_all(page, ".history-grid tbody tr td:first-child .history-link")[:1]
        out["V14_run_id_cells"] = probe_all(page, ".history-grid tbody .run-id-cell")[:2]
        out["V9_neutral_outcomes"] = probe_all(page, ".history-grid tbody .neutral-outcome")

        # ---- V2 / V3：轴二两个块 ----
        out["V2_swapped"] = probe_all(page, ".history-grid .terminal-block.is-swapped")
        out["V3_discarded"] = probe_all(page, ".history-grid .terminal-block.is-discarded")

        # ---- V4：错误码标签 4xx 虚边 / 5xx 实边 ----
        out["V4_error_codes"] = probe_all(page, ".history-grid .error-code")
        out["V4_error_summaries"] = [
            el.inner_text() for el in page.query_selector_all(".history-grid .error-summary")
        ]

        # ---- V25：排版 ----
        out["V25_body_font"] = page.evaluate(
            "() => { const cs = getComputedStyle(document.body);"
            " return { fontFamily: cs.fontFamily, colorScheme: cs.colorScheme,"
            " bg: cs.backgroundColor }; }"
        )
        out["V25_mono_cells"] = probe_all(page, ".history-grid td.mono")[:2]
        out["V25_weights"] = page.evaluate(
            "() => Array.from(new Set(Array.from(document.querySelectorAll('*'))"
            ".map(el => getComputedStyle(el).fontWeight))).sort()"
        )
        out["V25_dark_media"] = page.evaluate(
            "() => window.matchMedia('(prefers-color-scheme: dark)').matches"
        )

        # ---- V5：整页灰度后量真实像素 ----
        swapped = page.query_selector(".history-grid .terminal-block.is-swapped")
        discarded = page.query_selector(".history-grid .terminal-block.is-discarded")
        neutral = page.query_selector(".history-grid .neutral-outcome")
        if swapped and discarded:
            page.add_style_tag(content="html { filter: grayscale(1) !important; }")
            page.wait_for_timeout(200)
            # 必须整页截图：DISCARDED 那一行落在 1000px 视口以下，只截视口会把它裁成黑边，
            # 量出来是 0 而不是纸白。页面不滚动时 bounding_box 的坐标即整页图坐标。
            shot = page.screenshot(full_page=True)
            v5 = {}
            for name, el in (("swapped", swapped), ("discarded", discarded),
                             ("neutral_text_cell", neutral)):
                if el is None:
                    continue
                r = el.bounding_box()
                # 取块左侧内边距那一小片：块的 padding-left 是 9px，取样窗口收在
                # x∈[2,8) 才完全避开字形，量到的才是填充本身（原来 10 宽会切到第一个字母）
                v5[name] = median_luma(shot, r, 2, 3, 6, 14)
                v5[name]["rect"] = r
            if "swapped" in v5 and "discarded" in v5:
                diff = v5["discarded"]["median"] - v5["swapped"]["median"]
                v5["diff_median"] = diff
                v5["diff_pct"] = round(diff / 255 * 100, 1)
                v5["passes_10pct_bar"] = diff >= 25
            out["V5_grayscale"] = v5
            page.screenshot(path="/tmp/m2-v5-grayscale.png", full_page=True)

        # ---- V13：展开哨兵逃逸那一行，看敏感值框 ----
        page.reload(wait_until="networkidle")
        page.wait_for_selector(".history-grid tbody tr", timeout=15000)
        escape_row = page.query_selector(
            ".history-grid tbody tr:has(.error-code:text-matches('INTERNAL_PRECHECK_ESCAPE'))"
        )
        if escape_row is not None:
            escape_row.query_selector(".history-link").click()
            page.wait_for_selector(".history-detail .sensitive-value", timeout=15000)
            out["V13_masked"] = page.evaluate(
                """() => {
                     const box = document.querySelector('.sensitive-value');
                     const dl = box.querySelector('dl');
                     const dd = box.querySelector('dd');
                     const cs = getComputedStyle(dd);
                     return { head: box.innerText.split('\\n')[0],
                              full: box.innerText.replace(/\\n+/g, ' | '),
                              dlClass: dl ? dl.className : null,
                              filter: cs.filter, userSelect: cs.userSelect,
                              button: (box.querySelector('button') || {}).innerText || null };
                   }"""
            )
            page.click(".sensitive-value button")
            page.wait_for_timeout(200)
            out["V13_revealed"] = page.evaluate(
                """() => {
                     const box = document.querySelector('.sensitive-value');
                     const dl = box.querySelector('dl');
                     const dd = box.querySelector('dd');
                     const cs = getComputedStyle(dd);
                     return { dlClass: dl ? dl.className : null, filter: cs.filter,
                              userSelect: cs.userSelect,
                              button: (box.querySelector('button') || {}).innerText || null,
                              text: box.innerText.replace(/\\n+/g, ' | ') };
                   }"""
            )

        browser.close()

    with open(OUT, "w") as fh:
        json.dump(out, fh, ensure_ascii=False, indent=2)
    print(json.dumps(out, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
