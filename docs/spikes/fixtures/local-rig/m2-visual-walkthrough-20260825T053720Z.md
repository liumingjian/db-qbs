# 设计语言走查实录 · V1–V25（2026-08-25，WHERE 文本框与运行参数链退役）

- **触发**：`CLAUDE.md` 视觉门禁表第一行——**任何**对 `docs/design-system/README.md`
  或 `docs/design-system/tokens.css` 的改动。本次两份都动了：
  - README §7 那条组件从「自定义 SQL 编辑器 `.source-sql-editor`」改写成
    「**带高亮的 SQL 输入框** `.sql-text-input` + `.sql-highlight`」——它不再只出现在一处，
    过滤条件的 WHERE 文本框也用同一个控件（[ADR-0047](../../../adr/0047-where-textbox-and-the-end-of-run-parameters.md) §6）；
    抽屉那一行的内容清单里删掉「运行参数」。
  - `tokens.css` 那四个 SQL 高亮令牌的**取值一个没动**，只把注释里「只出现在自定义 SQL
    输入框」改成「两处共用」。
  **按规则 1，取值没变也照跑，不找豁免**——先例就是「v1 数据源屏订正 README 两处事实，
  那一下就触发了 V1–V25」。
- **本次跑的是回归 + 三条改判**：V14 / V15 换锚点（抽屉那块面板改名），V16 改判
  （发起对话框整个没了，同一件事改由屏顶的 409 横幅承担）。其余按回归判。
- **怎么跑的**：`walkthrough/run-v-walkthrough.sh`，真跑在用户 mac 上（`rexec`，
  `lmj-mac-mini-269d`），喂本次提交现构的 `web/dist`（`index-BeDaMWPK.css` 37.22 kB）。`exit=0`。
- **桩不是真库**：态由 `v-mock.py` 编出来，只答「渲染出来没有」。

原始输出：mac 上的 `/tmp/v-out.json`，截图在 `/tmp/m2-visual/`。下面是实际观察，不是「通过」。

---

## 一、三条轴（V1–V4）

**V1（进行中，停在 STREAMING）**

```
phase_items:  PREPARING 准备中   is-done     dot 8px 圆 rgb(82,196,26)
              STREAMING 传输中   is-current  dot 8px 圆 rgb(60,126,255)
              COMMITTING 提交中  （无类）     dot 8px 圆 rgba(0,0,0,.45)
phase_after:  → 终态待定          （后面 0 个点：终态不预告）
conclusion:   运行中 STREAMING
metrics:      已推行数 3 | 总行数 100,000 | 当前批次序号 1 | 已用时 00:00 | 累计字节 96 B
counts:       terminal_block 0   error_code 0   phase_item 3
```

进行中这一屏**没有终态块、没有错误码**——轴二与轴三都只在有结论之后才出现，与原判一致。

**跑这一条时揪出一个桩的缺陷并当场修了**：`总行数` 原本渲染成 `NaN`。产品没错——
`RunScreen` 写的是 `detail.total_rows === null ? "—" : formatCount(...)`；是 `v-mock.py` 的
`LIVE_STREAMING` 里**整个缺了 `total_rows` / `precount_ms` 两个字段**，前端拿到 `undefined`，
`=== null` 不成立，于是走进 `formatCount(undefined)`。真接口那两个字段是可空但**必给**的。
桩已补上（进行中给 100000/180，已受理给 None/None），实录里这一行才是产品真正的样子。

**V2（成功，SWAPPED）**

```
terminal_block: "SWAPPED 目标表已切换"  cls terminal-block is-swapped
                bg rgb(205,235,217)  color rgb(11,102,55)  border 1px solid rgb(183,235,143)
result_text:    outcome SUCCEEDED | SWAPPED 目标表已切换 |
                目标端：运行成功：已推送 100,000 行，暂存表已切换为目标表。
counts:         terminal_block 1  error_code 0
```

**V3（校验失败，DISCARDED）**

```
terminal_block: "DISCARDED 目标表未被触碰"  cls terminal-block is-discarded
                bg 透明  color rgba(0,0,0,.65)  border 1px solid rgb(217,217,217)
result_text:    outcome FAILED | DISCARDED 目标表未被触碰 | VERIFY_FAILED | HTTP 409 |
                [校验门禁] 目标端：行数校验未通过：暂存 100,000 行、目标端点到 99,998 行，暂存表已丢弃。
counts:         terminal_block 1  error_code 1
```

两个终态块**形状相同、只差着色**（V5 量的就是这个差），DISCARDED 是描边中性态而不是又一种红。

**V4（轴三错误码标签，4xx 与 5xx 分色）**

```
4xx: "PRECHECK_FAILED HTTP 422"  cls error-code is-rejected
     bg rgb(255,241,240)  color rgb(245,34,45)  border 1px dashed rgb(255,163,158)
     后面跟一句：[类型映射] 目标端：映射预检未通过：一次发现 3 项问题，未创建暂存表
5xx: "INTERNAL_PRECHECK_ESCAPE HTTP 500"  cls error-code is-internal
     bg rgb(255,251,230)  color rgb(250,173,20)  border 1px solid rgb(255,229,143)
```

4xx 虚线红、5xx 实线黄——「你配错了」与「我们出问题了」两种语气分得开。

## 二、V5：两个终态块的明度差

```
swapped   中位亮度 227   取样 6x14 @(851,134)
discarded 中位亮度 255
diff_median 28   diff_pct 11%   passes_25_over_255_bar: true
```

差 28/255 = 11%，过 25 那条线。取样方式：两块各自开一个抽屉取（一行只展示最近一次运行）。

## 三、V7 / V11 / V22：映射预检失败屏

```
V7:   terminal_block 0   error_code 1        ← 预检失败**没有终态块**（暂存表都没建）
V11:  sections 1  classes ["is-failed"]  header "映射预检 | 目标端"
      columns ["列","源端","目标端","规则","建议"]
      C_NAME     VARCHAR2(200)  VARCHAR(80)       目标列过窄        把目标列放宽到 VARCHAR(200)
      LOAD_DATE  DATE           VARCHAR(20)       类型不兼容        把目标列改成 DATETIME(0)
      ROW_NO     （未映射）      int(11) NOT NULL   未映射且不允许留空  目标表的 ROW_NO 列未被映射且不允许留空，请映射它或给它默认值
      total_line "总计 3 项问题"   skipped_placeholder_cards 0
V22:  exit_text "目标表结构与本次取数的列对不上。请在目标库中调整目标表，或回到任务编辑修改字段映射。"
      exit_buttons ["编辑任务"]     create_table_hits 0   ← 不诱导自动建表
```

## 四、V8 / V13 / V15：结局不明、业务值、两个 id

```
V8:  result_text  outcome FAILED | 结局不明 | 进程消失，无终态日志 |
                  无法确认目标表是否被修改，请到目标库核对。
     cls unknown-conclusion is-process_disappeared   bg #fff  无边框
     counts: terminal_block 0  error_code 0          ← 不冒充成一个终态，也不编一个错误码
V13: 遮住时  filter blur(5px)   user-select none   按钮「显示」  dl.is-masked
     揭开后  filter none        user-select auto   按钮「隐藏」  值 张三丰·测试客户名·2026
V15: identity 运行记录 rec-not-started
              目标端运行号 「未发起，目标端不知道这次运行」   ← 不是空白、不是横杠
              所属任务 task-not-started        暂存表 —
     conclusion outcome FAILED | [Oracle 连接] 源端：连接 Oracle 失败：ORA-12541: TNS:no listener，未向 sink 发出请求。
V14_V15_two_ids: 标题「运行详情 · 校验失败那条」，旁边 rec-verify
                 （rgba(0,0,0,.45) / ui-monospace / 12px）；
                 run_id 在「目标端运行号」栏，值 20260819121000_cccccc
```

## 五、V16 · **改判**：并跑拦截从对话框里的预警改成屏顶的 409 横幅

原判的对象是发起对话框里那条「只提示不拦你」的 info 人话条。**对话框整个没了**
（ADR-0047 §3：点了就跑），那条提示随之失去对象。实测顶上来的是：

```
notice:  "持仓日明细：该任务已有一次运行进行中 | 知道了"
         cls notice is-error
         bg rgb(255,241,240)   color rgba(0,0,0,.85)   border-left rgb(245,34,45)
         rect x 272  y 66  w 1152  h 42        ← 屏顶通栏，在内容区最上面
dismiss: 「知道了」
```

判的仍是**同一种语气**：一句人话、说清是哪一条任务（前缀带任务名）、给得掉一个收起它的动作。
原判「只提示不拦你」的那一半**反转**了——那条提示本来就是客户端发起前的一次预读，
与真闸门赛跑；现在报的是服务端真回的 409。

## 六、V17：取消键常亮，点下去当场如实回话

```
button:   「取消运行」 cls button is-ghost  bg #fff  border 1px solid rgb(217,217,217)
          disabled false   cursor pointer   pointer-events auto
STREAMING 时点下去： 「已发送 SIGTERM，等待子进程退出」
已受理（子进程还没报到）：
          conclusion 「已受理，正在拉起」
          phase_item_classes ["phase-item","phase-item","phase-item"]   ← 三点全灰
          button_text 「取消运行」  disabled false
          notice 「run 尚未进入可取消阶段」                              ← 当场如实拒绝
```

「禁用状态本身会说谎」那一条原判仍然成立：按钮不禁用，拒绝由回话给出。

## 七、V24 / V25：外壳与字体

```
V24: sidebar_bg rgb(0,21,41)
     items 作业中心(is-active, #fff) · 数据源 · 目标端 Agent · 系统设置（后三项 rgba(255,255,255,.75)）
     折叠：sider 48px，块宽 236→40，icon_center_offset 0（居中，不是被切掉大半）
     text_hidden none      builder_hits_in_nav 0    badges []
V25: body   -apple-system … PingFang SC …           colorScheme light   bg rgb(244,247,249)
     mono   ui-monospace, SF Mono, JetBrains Mono … variantNumeric tabular-nums
     字重只有两档：400 / 500（强调 500、卡内标题块 500）
     dark_media_matches false   dark_conditional_rules 0   top_level_rules 358
     深色侧栏不是深色主题：sider rgb(0,21,41) 而 card #fff、内容区透明
```

## 八、判废与已改判的那几条

```
V6  / V10 / V11 第一张卡 / V12 : 形状预检整段随 ADR-0036 §5 取消，屏不存在
V9  / V14                     : 运行历史屏随 ADR-0043 §2 并入作业中心；V9 方向已反转，形态判据归 X17
V18 / V21                     : 源端 SQL 改为规格现算只读（实测 textarea_count 1 = 那唯一一个是
                                自定义 SQL 输入框，「构建 SQL」头写「只读预览」，手改徽标 0 次、
                                重开向导 0 次）；目标表下拉与目标列参考表判据方向已反
                                （datalist 1 / input[list] 1，长度栏注「单位是字符…1 个汉字通常按 3 字节」）
V19 / V20                     : 构建器的建表 SQL 区块随 47a2fed 摘掉，2026-08-21 裁定判废；
                                探针仍去取一次并如实回一条 object_missing，别读成崩了
V23                           : 运行详情屏「重新发起」1 次、「重试」0 次；任务屏两者都 0
```

## 九、W 系列：**未跑（未触发）**

`.precheck-reports` / `.precheck-exit` / `.diagnostic-table` 与 `DiagnosticTable` 的列结构
一个字符未改，W1–W6 不触发。它的探针本票改过一处（`m3-probe.py` 不再去填一个已经不存在的
发起表单，并把 `.condition-row` 那一格的含义改成「旧控件真的一个不剩」），
按 `CLAUDE.md` 规则 4 冒烟跑到 `exit=0` 证明工具还能驱动——**冒烟不是判据**。
