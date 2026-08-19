#!/usr/bin/env python3
"""补两条零碎观察：V9 的「结局不明」那一族形态，V25 的暗色主题条件规则计数。"""
import json

from playwright.sync_api import sync_playwright

with sync_playwright() as p:
    b = p.chromium.launch()
    page = b.new_page(viewport={"width": 1440, "height": 1000}, device_scale_factor=1)
    page.goto("http://127.0.0.1:18088/#history", wait_until="networkidle")
    page.wait_for_selector(".history-grid tbody tr", timeout=15000)
    out = {}
    out["V9_unknown_summary"] = page.evaluate(
        """() => Array.from(document.querySelectorAll('.unknown-summary')).slice(0, 3).map(el => {
             const cs = getComputedStyle(el); const r = el.getBoundingClientRect();
             return { cls: el.className, text: el.innerText.replace(/\\n/g, ' / '),
                      color: cs.color, bg: cs.backgroundColor, display: cs.display,
                      w: Math.round(r.width), h: Math.round(r.height) };
           })"""
    )
    out["V9_live_summary"] = page.evaluate(
        """() => Array.from(document.querySelectorAll('.live-summary')).map(el => {
             const cs = getComputedStyle(el); const r = el.getBoundingClientRect();
             return { text: el.innerText, color: cs.color, w: Math.round(r.width) };
           })"""
    )
    out["V25_dark_rules"] = page.evaluate(
        """() => {
             let hits = 0, scanned = 0;
             for (const sheet of document.styleSheets) {
               let rules; try { rules = sheet.cssRules; } catch (e) { continue; }
               for (const rule of rules) {
                 scanned++;
                 if (rule.media && /prefers-color-scheme/.test(rule.conditionText || '')) hits++;
               }
             }
             return { prefers_color_scheme_rules: hits, top_level_rules_scanned: scanned };
           }"""
    )
    b.close()
print(json.dumps(out, ensure_ascii=False, indent=2))
