# M3 渲染面走查实录 · W1–W6（2026-08-21，P2 作业中心换皮后）

- **触发**：`CLAUDE.md` 视觉门禁表第 2 行 + [ADR-0043](../../../adr/0043-p2-job-center.md) §走查触发——
  `web/src/app.css` **整体重写**（不是增补），必然碰到 `.precheck-reports` 的布局。
- **怎么跑的**：`walkthrough/run-w-walkthrough.sh`（`m3-mock.py` + `m3-probe.py`，
  喂真实的 `web/dist`）。真跑在用户 mac 上（`rexec --mac lmj-macbook`）。
- **台架改动**：`m3-probe.py` 的 `#tasks` 改成 `#jobs`（屏的 id 随 ADR-0043 §2 变了）；
  `open_builder()` 由「取不到按钮就崩」改成「取不到就如实回一条对象不存在」——
  探针只观察不断言（ADR-0028 §1），崩掉换来的是没有记录，不是记录了失败。

原始输出：`/tmp/w-report.json`（4.4 KB）。下面是实际观察，不是「通过」。

---

**W1（映射预检报告只剩一段，不分栏、不留占位空栏）**：1440 视口下
`.precheck-reports section` **1 个**，容器盒 `1120 x 369.4`、内部 section 盒 `1118 x 367.4`——
**只差那 1px 边框，没有第二栏的空位**。`.is-skipped` 占位卡 0 个。

**W2（1024 下不横滚）**：`.precheck-reports section` 仍是 1 个，
盒 `702 x 455.4`（高度变高、宽度收窄，说明是换行而不是压缩）；
`.diagnostic-table` 的 `scrollWidth - clientWidth = 0`、`document.body` 的也是 **0**——
**表格没横滚，页面也没横滚**。

**W6（诊断表五列 + 每行都有建议 + 总计行）**：两个视口下列头都是
`列 · 源端 · 目标端 · 规则 · 建议`，6 行，**空建议单元格 0 个**，收尾 `总计 6 项问题`。
六行逐字：

| 列 | 源端 | 目标端 | 规则 | 建议 |
|---|---|---|---|---|
| PAYLOAD | CLOB | `<missing>` | 目标表缺列 | 在目标表加列，或把该列从源 SQL 里去掉 |
| V_TEXT | VARCHAR2(200) | VARCHAR(80) | 目标列过窄 | 把目标列放宽到 VARCHAR(200) |
| D_WRONG | DATE | VARCHAR(20) | 类型不兼容 | 把目标列改成 DATETIME(0) |
| N_TOO_WIDE | NUMBER(38,-30) | DECIMAL(65,30) | 超出 MySQL DECIMAL(65,30) | 改源 SQL 或 CAST 收窄值域 |
| N_MISSING | NUMBER | DECIMAL(10,2) | 裸 NUMBER 未声明精度 | 在取列面为该列配 (p,s) |
| N_BARE | NUMBER | DECIMAL(10,2) | 值域校核：3 行超出目标 DECIMAL(10,2) | 调整任务定义和目标 DECIMAL 的 (p,s)，或改源 SQL / CAST 收窄值域 |

**旁证（构建器面，走查清单之外）**：两个数据源下拉各自只列自己那一侧；
条件行 3 行；源端 SQL `readonly`；「主键用于去重，必须至少选一列。」在位。

---

## W3 / W4 / W5：**对象已经不存在了（不是本轮删的）**

| # | 观察 |
|---|---|
| **W3** | 构建器里 `.column-fetch-section` **0 个**（`aria-labelledby` 清单为空），`.fetch-ready` 不存在。三档标记那一列无从看起 |
| **W4** | 同上：`.ddl-placeholder` 与建表 SQL 区块都没有对象 |
| **W5** | 同上：目标表填 `REJECTED` 这一态无从制造——切态入口本身在取列卡上 |

**根因与 V19 / V20 同一条**：`47a2fed`（*Prepare x2doris P1 frontend handoff*，2026-08-21）
把构建器里整段「目标表建表 SQL / 拿建表 SQL / `.fetch-ready`」摘掉了。
父提交 `33e9ec5` 与 v1 验收那次的 `85805b1` 里 `column-fetch-title` 都还在（各命中 2 次）。

**不是本轮 P2 改动造成的**，但它是一次**没被门禁接住**的回退：摘掉这段的那一票没跑 W1–W6。
本轮如实记下，不代所有者决定要不要加回来。

---

## 结论

- **判据成立、实测已贴**：W1 / W2 / W6。
- **对象不存在（`47a2fed` 引入的回退，非本轮）**：W3 / W4 / W5。
