# 设计系统走查实录 · V1–V25（2026-08-24，一轮 QA 修复）

- **触发**：`CLAUDE.md` 视觉门禁表第 1 行——`docs/design-system/tokens.css` 加了四个令牌
  （`--sql-keyword` / `--sql-string` / `--sql-number` / `--sql-quoted`），
  `README.md` §7 的组件清单加了「自定义 SQL 输入框」一条
  （[ADR-0046](../../../adr/0046-qa-round-editor-nav-and-dead-column.md) §走查触发）。
- **这一趟大半是回归**：新增的是 §7 那条组件，其余判据一条未动。
  上一轮（`20260824T044212Z`）判「无改判、无新增、无回归」，这一轮要看的是**它还是不是那个形状**，
  外加导航次序变了之后 V24 该跟着变。
- **怎么跑的**：`walkthrough/run-v-walkthrough.sh`（`v-mock.py` + `v-probe.py`），
  真跑在用户 mac 上（`rexec`，`lmj-mac-mini-269d`），喂本次提交现构的 `web/dist`。`exit=0`。
- **判据版本**：`m2-visual-walkthrough.md`，含 2026-08-19 的 N/A 处置与 2026-08-21 的 P2 改判。

原始输出：mac 上的 `/tmp/v-out.json`（27 组观察）。下面是实际观察，不是「通过」。

---

## 与本票直接相关的那一条：V24（外壳）

```
sidebar_bg: rgb(0, 21, 41)
sidebar_items:
  A  nav-item is-active   作业中心        rgb(255, 255, 255)
  A  nav-item             数据源          rgba(255, 255, 255, .75)
  A  nav-item             目标端 Agent    rgba(255, 255, 255, .75)
  A  nav-item             系统设置        rgba(255, 255, 255, .75)
collapsed_icon_centering: 展开块宽 236 / 折叠块宽 40 / 侧栏 48 / icon_center_offset 0 / text 隐藏
builder_hits_in_nav: 0
badges: []
```

**次序变了，形态一项没变**：仍是四个 `<a class="nav-item">`、仍是深色侧栏、
折叠后图标仍居中（`icon_center_offset: 0`）、构建器仍不是导航项、仍没有一个 `M3+` 灰标。
与上一轮相比唯一的差别就是 `is-active` 从第二项挪到了第一项——
上一轮的实录逐字写着 `目标端 Agent · 作业中心(is-active) · 数据源 · 系统设置`。

## 新增的那一条：§7 组件清单

新加的四个令牌**在 V 系列里没有对象**——它们只出现在构建器模态的自定义 SQL 输入框里，
而 V 系列的探针走的是运行详情 / 发起 / 构建器表模式这几屏。这一条如实记下来：
**颜色本身的判据在 X20**（`v1-visual-walkthrough-20260824T153058Z.md` 第三节，
六类记号的实测色值与两层同框的九项比对都在那里），本清单只负责「它进了令牌文件、
进了组件清单」这件账。

## 三轴（V1 / V2 / V7 / V8）

```
V1  phase_item 3 / terminal_block 0 / error_code 0
    PREPARING 准备中  is-done     dot 8x8 50% rgb(82,196,26)
    STREAMING 传输中  is-current  dot 8x8 50% rgb(60,126,255)
    COMMITTING 提交中 （无）      dot 8x8 50% rgba(0,0,0,.45)
    phase_after "→ 终态待定"，其后 dot 0 个
V2  terminal_block 1 / error_code 0 / phase_item 0
    "SWAPPED | 目标表已切换"  is-swapped  bg rgb(205,235,217)  color rgb(11,102,55)  border 1px solid rgb(183,235,143)
V7  error_code 1，terminal_block 0，phase_item 0
V8  terminal_block 0 / error_code 0 / phase_item 0
    "结局不明 | 进程消失，无终态日志 | 无法确认目标表是否被修改，请到目标库核对。"
```

三轴仍然各占各的，**结局不明那一态仍然一个错误码标签都不出**。

## V5（可量的那条）

```
swapped   中位 227  (min 227 / max 227)  6x14 @(851,134)
discarded 中位 255  (min 255 / max 255)  6x14 @(829,134)
diff_median 28   diff_pct 11.0   passes_25_over_255_bar: true
```

整页 `grayscale(1)` 后差 **28/255（11.0%）**，与上一轮逐字相同，仍在 ≥25 的下界之上。

## 其余各条

- **V11（映射预检）**：一段、`is-failed`、表头 `列 · 源端 · 目标端 · 规则 · 建议`、3 行、
  `总计 3 项问题`，`skipped_placeholder_cards: 0`（灰色占位卡确实不存在）。
- **V13（业务值告警框）**：遮罩态 `blur(5px)` + `user-select: none` + 按钮「显示」；
  点开后 `filter: none`、`user-select: auto`、按钮变「隐藏」。
- **V16**：陈旧运行提示左边 3px `rgb(250,173,20)` 竖条 + `rgb(255,251,230)` 底，
  **发起按钮不禁用**（`submit_disabled: false`，`cursor: pointer`）——提示不拦人。
- **V17**：「取消运行」是幽灵按钮；`STREAMING` 上给 `已发送 SIGTERM，等待子进程退出`，
  未进入可取消阶段时给 `run 尚未进入可取消阶段`，两态都不禁用按钮。
- **V18**：`textarea_count: 0`，SQL 区写 `构建 SQL | 只读预览`，没有「手改」徽标、没有「重走向导」。
  **本轮特别核对了这一条**：自定义 SQL 那个可编辑的 `textarea` 长在**构建器的源端半边**，
  与这里说的「派生面只读」不是同一个对象，V18 的对象仍然是 0 个。
- **V21**：目标表是 `<input list>` + 一个 `<datalist>`，长度栏那句静态说明在，
  `not_drawn_copy_hits: 0`。
- **V22 / V23 / V15 / V19 / V20 / RETIRED 组**：与 `20260824T044212Z` 逐条一致，
  缺席理由由探针原样打印。
- **V25**：`--weight-em` 实测 **500**，页面只用 `400 / 500` 两档；
  `dark_media_matches: false`、暗色条件规则 **0**；`top_level_rules: 372`；
  侧栏深色不是暗色主题（`sider_bg rgb(0,21,41)` / `card_bg rgb(255,255,255)`）。

## 结论

**V1–V25 无回归**。改判只有 V24 里的一处次序，新增只有 §7 那条组件清单，
而那条组件的实际判据由 X20 守（见 `v1-visual-walkthrough-20260824T153058Z.md`）。

四个新令牌**在本清单里没有被看到**，这一点如实写在上面，不当作「通过」。
