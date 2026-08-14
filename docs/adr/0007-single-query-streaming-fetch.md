# ADR-0007: 取数模型——单查询流式，批次是纯粹的推送切分单位

**状态**: 已接受
**日期**: 2026-08-13

## 背景

`CONTEXT.md` 的部署图写的是「直连 Oracle **分页**拉取」，而 M0 spike §4 实测的是
**单查询流式 fetch**。这两句话不是一回事，M1 开工前必须选一个——
地图 [#22](https://github.com/liumingjian/db-qbs/issues/22) 的
[#23](https://github.com/liumingjian/db-qbs/issues/23) 就是为此开的。

两个候选：

- **A. 单查询流式**：一条 SQL `execute` 之后边 fetch 边攒批，攒够 5000 行推一批。
  没有分页 SQL，没有 `ROW_NUMBER()`，不要求排序键。
- **B. 分页多查询**：每批一条带 `ROW_NUMBER() OVER (ORDER BY ...)` 的分页 SQL。
  Oracle 11g 无 `OFFSET/FETCH`，要求排序键全序，且源表运行期间不冻结。

## 决策

**选 A：一条 SQL、一个游标、边 fetch 边攒批。**

```
  let cursor = conn.execute("SELECT c1..cN FROM t WHERE d_biz = :biz_date", &[biz_date]);
  let mut buf = Vec::with_capacity(5000);
  for row in cursor {                     // 驱动每 fetch_array_size 行走一个网络来回
      buf.push(to_canonical(row));        // ADR-0003 规范形式
      if buf.len() == 5000 { http_post(run_id, seq, &buf); buf.clear(); }
  }
  if !buf.is_empty() { http_post(run_id, seq, &buf); }
  http_post_commit(run_id, total_batches, total_rows);   // ADR-0002 的封口点
```

**批次 = 纯粹的推送切分单位**：*一次运行内，源端把结果流按固定行数切开推送的一段，
除「属于哪个 run、是第几段」外不携带任何数据身份。*
协议里**不带边界信息**——不带首尾主键、不带 WHERE 片段、不带 `ROW_NUMBER` 范围。
sink 只管往暂存表追加，不做任何按批定位。批次是**协议层**概念，不是领域概念。

**参数**：批次 **5000 行**（只按行数，不设字节判据），`fetch_array_size = 100`，
`prefetch_rows` 不设。三者在 V1 **写死成常量，不做配置项**。

**序号保留，语义降级**：从 1 单调递增，是**顺序断言与诊断锚点，不是幂等键**。
sink 维护 `expected_seq`，不匹配即硬错误、整 run 失败。源端封口时同时报总批数与总行数。

## 理由

- **内存不构成选 B 的理由。** spike §4.2：批次固定 5000，行数 1k→10k→100k，`VmHWM` 在
  10k 与 100k 上一字不差（27,656 kB）；反证组（全量 collect）同样数据涨到 55 MB，
  证明测量对「随行数涨」是敏感的。峰值 ≈ 地板 + 批次行数 × 单行字节 + 取数缓冲，
  **三项里没有总行数**。
- **B 唯一的好处在 M1 里收益为零。** 「按批次号重查」只在**重试**时兑现，而 V1 的前提是
  **失败即整 run 失败**——不重试、不暂存、不续传，永远不会去重查第 37 批。
  失败就是丢暂存表、整条重跑，重跑是一条新 `run_id` 从头再 `execute` 一次。
  B 的代价全付，收益不取。
- **B 在 11g 上要额外买三样**：`ROW_NUMBER()` 套子查询、排序键**必须全序**
  （否则并列行顺序无保证、批次边界漂——[#21](https://github.com/liumingjian/db-qbs/issues/21)）、
  每翻一页服务端重排重扫一次全集。
- **源表不冻结时 A 更强。** 一次 `execute` 给的是**单一读一致性快照**；
  B 的 N 条查询各自取自不同时点，运行期间有并发写入就会漏行/重行——
  那正是行数校验会报错、却查不出原因的一类故障。
- **批次不做「可定位区间」**，是因为给它造身份的两条路（记首尾主键 / 源端留副本）
  都只为重试服务：前者把 B 的全序键约束请回来，后者就是
  [#15](https://github.com/liumingjian/db-qbs/issues/15) 的暂存方案。M1 没有重试。

## 代价

- **游标要在源库上挂满整个 run。** Oracle 为读一致性维持快照，源表并发 DML 写 undo，
  undo 被覆盖则 fetch 到一半抛 `ORA-01555 snapshot too old`（`undo_retention` 默认 900 秒）。
  风险随「游标寿命 × 源表并发写入量」涨。
  **处置：归入运行时故障**，撞上就整 run 失败——这本就落在「失败即整 run 失败」的语义里，
  不新增机制。但**错误处理必须显式识别这个错误码**并给人话：
  「源端结果集在读取过程中失效，通常是运行时间过长且源表有大量并发写入，
  请缩小业务日期范围或联系 DBA 调大 undo 保留」。
  **「真实源端的 run 时长与 `undo_retention` 配置」加进
  [#2](https://github.com/liumingjian/db-qbs/issues/2) 的上线前复验清单**——
  台架没有并发写入压力，答不了这个；它决定「DBA 保证 `undo_retention` ≥ 预期 run 时长」
  要不要在 M4 升成硬前置。
  注意 B 并不能免掉这个代价，它换来的是更糟的东西（分页期间源表在变）。
- **fetch 与推送串行**：ADR-0001 是同步阻塞 IO，`http_post` 期间游标停着不 fetch，
  推送时间直接计入游标寿命，加重上一条。**M1 不做流水线**（双缓冲引入线程、背压、
  跨线程错误传播，是最容易埋暗坑的地方）。
  **但台架 10 万行验收时必须记录三个数：fetch 累计耗时 / 推送累计耗时 / 全程游标寿命。**
  若推送占比过半，流水线就从「优化」变成 ORA-01555 的风险控制手段，届时另开票，
  依据是这三个数，不是感觉。
- ~~**批次大小只按行数**，不设字节上限。生产表 70 列若行宽远超预估，5000 行的载荷会偏大——
  真实行宽等 [#2](https://github.com/liumingjian/db-qbs/issues/2) 拿回来再谈调值。~~
  **2026-08-13 订正（[ADR-0011](0011-batch-payload-wire-format.md) §6，[#27](https://github.com/liumingjian/db-qbs/issues/27)）**：
  #2 拿不回来，而载荷形状定下来后典型批次约 15 MB、离 64 MiB 上限只剩 4 倍。
  **批次改为「5000 行或 16 MiB，先到先切」。** 这不动本 ADR 对批次的定性——
  它仍是纯粹的推送切分单位，按字节切只让 `seq` 多涨几个数，协议一个字不改；
  序号本来就是诊断锚点而非幂等键，不受影响。**代价：批次不再恒为 5000 行**，
  日志与监控不能假设 `rows == 5000`。

## 对既有决策的影响

- **ADR-0002 的「批次带序号，断线可重推」在 V1 不适用。** 原文不改（已接受的 ADR 是历史记录），
  由本 ADR 声明适用范围：V1 的序号只做顺序断言与诊断；M4 的重推语义由
  [ADR-0018](0018-delayed-batch-retry-model.md) 定义。
  **commit 作为暂存表封口点照旧。**
- **`CONTEXT.md` 部署图的「分页拉取」措辞与「批次」词条按本 ADR 订正。**
- **[#21](https://github.com/liumingjian/db-qbs/issues/21)（分页边界可复现实测）与
  [#15](https://github.com/liumingjian/db-qbs/issues/15)（批次重试模型）不阻塞 M1**——
  两者都是 B 的前置事实，A 走的路上碰不到。
- ADR-0001、ADR-0003、ADR-0004、ADR-0006 不受影响。

## 时效

**V1 先按此方案走，实践中发现问题再优化或改进。**
最可能触发复审的两条：#2 复验拿回的真实行宽（→ 批次取值）与真实 run 时长
（→ ORA-01555 与流水线）。
