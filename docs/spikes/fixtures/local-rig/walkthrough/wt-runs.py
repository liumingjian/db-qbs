#!/usr/bin/env python3
"""M2 渲染面走查 · 运行详情屏（RunScreen）的机器观察。

RunScreen 只在「从 UI 发起一个 run」之后才到得了（App.tsx 里 setActiveRun 的唯一调用点在
StartRunDialog 的 onStarted），所以这支脚本自己发起 run。

按 ADR-0028 §1：只观察，不断言；一行 DOM 断言都不进验收套件。

用法：
    python wt-runs.py live   # 台架 child mode = hang-streaming：V1 / V16 / V17 / V8
    python wt-runs.py real   # 台架 child mode = real：V6 V10 V12 V15 / V7 V11 V22 V4 / V2
"""

import json
import sys

from playwright.sync_api import sync_playwright

BASE = "http://127.0.0.1:18088"
BIZ_DATE = "2026-08-14"

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
    borderRadius: cs.borderRadius,
    padding: cs.padding,
    opacity: cs.opacity,
    cursor: cs.cursor,
    pointerEvents: cs.pointerEvents,
    disabled: el.disabled === undefined ? null : el.disabled,
    rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) },
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


def open_tasks(page):
    page.goto(BASE, wait_until="networkidle")
    page.wait_for_selector("#tasks tbody tr", timeout=20000)


def start_run(page, task_name):
    """从任务列表发起一个 run，停在 RunScreen 上。"""
    row = page.locator("#tasks tbody tr").filter(has_text=task_name).first
    row.locator("button[aria-label='发起运行']").click()
    page.wait_for_selector(".modal.is-narrow", timeout=10000)
    page.fill(".modal.is-narrow input[type=date]", BIZ_DATE)
    page.click(".modal.is-narrow button[type=submit]")
    page.wait_for_selector(".run-card", timeout=20000)


def wait_streaming(page, timeout_ms=30000):
    """等子进程真的报到 STREAMING：刚发起时 stage 还是 null，取消会被如实拒绝
    （「run 尚未进入可取消阶段」），阶段串也还没有 is-current 那一点。"""
    page.wait_for_selector(".phase-item.is-current", timeout=timeout_ms)


def wait_finished(page, timeout_ms=120000):
    page.wait_for_selector(".run-result, .unknown-conclusion", timeout=timeout_ms)


def phase_dots(page):
    return page.evaluate(
        """() => Array.from(document.querySelectorAll('.phase-item')).map(item => {
             const dot = item.querySelector('.phase-dot');
             const cs = dot ? getComputedStyle(dot) : null;
             return {
               text: item.innerText.replace(/\\n/g, ' '),
               cls: item.className,
               dot: cs ? { w: cs.width, h: cs.height, radius: cs.borderRadius,
                           bg: cs.backgroundColor, border: cs.borderTopColor } : null,
             };
           })"""
    )


def live_walk(page, out):
    # ---- V1：进行中（hang-streaming 下停在 STREAMING）----
    start_run(page, "A10 并发")
    page.wait_for_selector(".live-state", timeout=30000)
    wait_streaming(page)
    out["V1_phase_items"] = phase_dots(page)
    out["V1_phase_after"] = page.eval_on_selector(".phase-after", "el => el.innerText")
    out["V1_phase_after_dot_count"] = page.evaluate(
        "() => document.querySelectorAll('.phase-after .phase-dot').length"
    )
    out["V1_counts"] = counts(page)
    out["V1_conclusion"] = page.eval_on_selector(".live-state strong", "el => el.innerText")
    out["V1_metrics"] = page.evaluate(
        """() => Array.from(document.querySelectorAll('.run-metrics > div'))
             .map(d => d.innerText.replace(/\\n/g, ' '))"""
    )
    out["V1_identity"] = page.evaluate(
        """() => Array.from(document.querySelectorAll('.run-identity > div'))
             .map(d => d.innerText.replace(/\\n/g, ' | '))"""
    )
    page.screenshot(path="/tmp/m2-v1-live.png", full_page=True)

    # ---- V16：同任务同日期再打开发起对话框 ----
    page.click(".back-button")
    page.wait_for_selector("#tasks tbody tr", timeout=15000)
    row = page.locator("#tasks tbody tr").filter(has_text="A10 并发").first
    row.locator("button[aria-label='发起运行']").click()
    page.wait_for_selector(".modal.is-narrow", timeout=10000)
    page.fill(".modal.is-narrow input[type=date]", BIZ_DATE)
    page.wait_for_selector(".stale-run-hint", timeout=15000)
    out["V16_hint"] = probe_all(page, ".stale-run-hint")
    out["V16_hint_left_border"] = page.eval_on_selector(
        ".stale-run-hint",
        "el => { const cs = getComputedStyle(el);"
        " return { borderLeft: cs.borderLeftWidth + ' ' + cs.borderLeftStyle + ' ' + cs.borderLeftColor,"
        " bg: cs.backgroundColor }; }",
    )
    out["V16_submit"] = probe_all(page, ".modal.is-narrow button[type=submit]")
    page.click(".modal.is-narrow button.is-ghost")

    # ---- V17：取消按钮常亮；点下去当场如实回话 ----
    start_run(page, "A14 生命周期")
    page.wait_for_selector(".live-state", timeout=30000)
    wait_streaming(page)
    out["V17_cancel_button"] = probe_all(page, ".run-header button")
    out["V17_run_record_id"] = page.eval_on_selector(
        ".run-header .card-subtitle", "el => el.innerText")
    page.get_by_role("button", name="取消运行").click()
    page.wait_for_selector(".run-notice", timeout=15000)
    out["V17_notice"] = page.evaluate(
        """() => Array.from(document.querySelectorAll('.run-notice'))
             .map(el => ({ cls: el.className, text: el.innerText.replace(/\\n+/g, ' | ') }))"""
    )

    # ---- V8：取消后落成「结局不明」----
    wait_finished(page, timeout_ms=30000)
    out["V8_counts"] = counts(page)
    out["V8_result_text"] = page.eval_on_selector(
        ".run-content", "el => el.innerText.replace(/\\n+/g, ' | ')"
    )
    page.screenshot(path="/tmp/m2-v8-unknown.png", full_page=True)


def precheck_cards(page):
    return page.evaluate(
        """() => Array.from(document.querySelectorAll('.precheck-reports > section')).map(s => {
             const cs = getComputedStyle(s);
             return {
               cls: s.className,
               bg: cs.backgroundColor,
               header: (s.querySelector('header') || {}).innerText || null,
               subtitle: (s.querySelector('p') || {}).innerText || null,
               columns: Array.from(s.querySelectorAll('.diagnostic-table thead th')).map(th => th.innerText),
               rows: Array.from(s.querySelectorAll('.diagnostic-table tbody tr'))
                       .map(tr => Array.from(tr.querySelectorAll('td')).map(td => td.innerText)),
               small: (s.querySelector('small') || {}).innerText || null,
             };
           })"""
    )


def real_walk(page, out):
    # ---- A6 形状预检失败：V6 / V10 / V12 / V15 ----
    start_run(page, "A6 形状失败")
    wait_finished(page)
    out["V6_counts"] = counts(page)
    out["V6_conclusion"] = page.eval_on_selector(
        ".run-result", "el => el.innerText.replace(/\\n+/g, ' | ')"
    )
    out["V10_V11_cards"] = precheck_cards(page)
    out["V15_identity"] = page.evaluate(
        """() => Array.from(document.querySelectorAll('.run-identity > div'))
             .map(d => d.innerText.replace(/\\n/g, ' | '))"""
    )
    out["V12_error_code_count"] = len(page.query_selector_all(".error-code"))
    out["V22_precheck_exit_on_shape"] = len(page.query_selector_all(".precheck-exit"))
    page.screenshot(path="/tmp/m2-v6-shape.png", full_page=True)

    # ---- A7 映射预检失败：V7 / V11 / V22 / V4 ----
    page.click(".back-button")
    page.wait_for_selector("#tasks tbody tr", timeout=15000)
    start_run(page, "A7 映射失败")
    wait_finished(page)
    out["V7_counts"] = counts(page)
    out["V11_cards"] = precheck_cards(page)
    out["V4_error_summary_children"] = page.evaluate(
        """() => Array.from(document.querySelectorAll('.error-summary > *')).map(el => {
             const r = el.getBoundingClientRect();
             return { cls: el.className, text: el.innerText, x: Math.round(r.x) };
           })"""
    )
    out["V4_error_code_style"] = probe_all(page, ".error-code")
    out["V22_exit"] = page.evaluate(
        """() => {
             const el = document.querySelector('.precheck-exit');
             if (!el) return null;
             return { text: el.innerText.replace(/\\n+/g, ' '),
                      buttons: Array.from(el.querySelectorAll('button')).map(b => b.innerText) };
           }"""
    )
    out["V22_create_table_hits"] = page.evaluate(
        "() => (document.body.innerText.match(/CREATE TABLE/gi) || []).length"
    )
    out["V23_run_screen"] = page.evaluate(
        """() => ({ retry: (document.body.innerText.match(/重试/g) || []).length,
                    relaunch: (document.body.innerText.match(/重新发起/g) || []).length })"""
    )
    page.screenshot(path="/tmp/m2-v7-mapping.png", full_page=True)

    # ---- A5 成功：V2 ----
    page.click(".back-button")
    page.wait_for_selector("#tasks tbody tr", timeout=15000)
    start_run(page, "A5 正常 10 万行")
    wait_finished(page, timeout_ms=300000)
    out["V2_counts"] = counts(page)
    out["V2_terminal_block"] = probe_all(page, ".terminal-block")
    out["V2_result_text"] = page.eval_on_selector(
        ".run-result", "el => el.innerText.replace(/\\n+/g, ' | ')"
    )
    out["V2_metrics"] = page.evaluate(
        """() => Array.from(document.querySelectorAll('.run-metrics > div'))
             .map(d => d.innerText.replace(/\\n/g, ' '))"""
    )
    page.screenshot(path="/tmp/m2-v2-success.png", full_page=True)


def main():
    phase = sys.argv[1] if len(sys.argv) > 1 else "live"
    out = {"phase": phase}
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(viewport={"width": 1440, "height": 1000},
                                device_scale_factor=1)
        open_tasks(page)
        try:
            if phase == "live":
                live_walk(page, out)
            else:
                real_walk(page, out)
        except Exception as error:  # 观察工具：半路挂了也要把已看到的东西落盘
            out["ERROR"] = f"{type(error).__name__}: {error}"
        browser.close()
    with open(f"/tmp/m2-walkthrough-{phase}.json", "w") as fh:
        json.dump(out, fh, ensure_ascii=False, indent=2)
    print(json.dumps(out, ensure_ascii=False, indent=2))
    if "ERROR" in out:
        sys.exit(1)


if __name__ == "__main__":
    main()
