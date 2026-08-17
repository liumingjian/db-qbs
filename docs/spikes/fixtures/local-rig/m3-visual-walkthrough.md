# M3 渲染面人工走查清单

**规格来源**：[ADR-0032 §8](../../../adr/0032-m3-acceptance-criteria-and-rig-extension.md)
（决策票 [#103](https://github.com/liumingjian/db-qbs/issues/103)）。
界面增量的定形见 [#102](https://github.com/liumingjian/db-qbs/issues/102) 与
[`docs/prototypes/0102-m3-ui-increments.html`](../../../prototypes/0102-m3-ui-increments.html)，
裁定落 [ADR-0025 2026-08-16 增补](../../../adr/0025-m2-visual-language-and-design-system.md)、
[ADR-0010 增补二](../../../adr/0010-http-protocol-contract.md)、
[ADR-0027 增补二](../../../adr/0027-target-ddl-generation-and-cross-end-metadata.md)。
视觉规则的唯一来源仍是 [`docs/design-system/`](../../../design-system/)。

## 这份清单为什么存在，以及为什么**不是**重跑 V1–V25

#102 已判 **M3 零设计系统改动**——`docs/design-system/README.md` 与 `tokens.css`
一个字没改，五处新信息位全部由既有元素承载。因此
[ADR-0028 §6](../../../adr/0028-m2-acceptance-criteria-and-rig-extension.md) 的
**第 2 条触发条件不成立，整份 `m2-visual-walkthrough.md` 不重跑**。

要看的只有两类东西：**五处新信息位真的渲染出来了**，以及**一处 `web/src/app.css`
的布局裁定真的生效了**。六条，就是本清单。

**不并进 M2 那份**：并进去之后「跑过 M2 走查」这句话的含义会随 M3 漂移，
理由与 ADR-0032 §2 的三入口分立同源。

**不给退出码。** 自动验收（`run-m3-acceptance.sh` 的 B 系列）与本清单互不替代：
前者判「值搬对没有、报告说对没有」，后者判「渲染出来没有」。

## 触发条件（两条，固定）

1. **M3 验收时必跑一次。**
2. **任何改动 `web/src/app.css` 中 `.precheck-reports` 布局、或 `DiagnosticTable`
   列结构的变更，合并前必跑一次。**

跑完**贴实际观察，不是贴「通过」**——照 ADR-0014 §8 第 3 条的先例。

## 走查项

制造情形的手段与自动验收同源（ADR-0032 §4 的 B1–B6），走查时可直接复用那套编排造出各态；
`M3_KEEP_RIG=1` 的用法照 M2 那份。

| # | 打开哪一屏 · 制造什么情形 | 该看见什么 |
|---|---|---|
| **W1** | 运行详情 · 映射预检失败（复用 **B2** 的全违规表） | 预检表是**五列**，第五列「建议」**一律填动作、没有空格子**（web 不重算，值由 sink 侧填）。逐列一条 + 总计，**一次报全**，不分组、不折叠、不截断 |
| **W2** | 同上，视口 **1024** 与 **1440** 各看一次 | 映射预检失败态下两栏**整宽堆叠**，第五列**全在框内、不需要横滚**（ADR-0025 增补 §3）。对照：`shape-failed` 态**仍是两栏并置**，不得一起改掉 |
| **W3** | 构建器 · 取列（源 SQL 含裸 `NUMBER` 与 `BINARY_FLOAT`，复用 **B1/B2** 的表） | 取列卡三档标记 `[待配精度]` / `[不支持]` 是**纯文字、不着色**，不长成标签、不套 `--warn`/`--crit`（ADR-0029 §3 同构） |
| **W4** | 同上，看建表 SQL 区块 | 裸 `NUMBER` 列吐 `DECIMAL(<p>,<s>)` **占位符**（既有 `.ddl-placeholder`），**整份 DDL 照给**，不因为有占位符就整份失败 |
| **W5** | 构建器 · 取列（源 SQL 含白名单外的列，如 `CLOB`） | **第四态**：列表**照给**，只有 DDL 区块换成「整份不给」（既有 `.row-size-warning.is-crit`）。列清单不得一起消失 |
| **W6** | 运行详情 · 映射预检失败（复用 **B4** 的超域裸 `NUMBER` 表） | 值域校核的不合规记录**混在同一张五列表里**，与其余逐列规则**形态一致**（ADR-0030 §4.3），不另起区块、不另起标题 |

## 记录格式

每次走查在本目录开一份 `m3-visual-walkthrough-<UTC>.md`（与 `m3-acceptance-<UTC>.md` 并列），
逐条写实际观察。
**「W2 通过」不算记录，「W2：1024 下两栏已堆叠，第五列『建议』末列右边距 16px 在框内；
1440 下同样堆叠，未回退两栏」才算。**
