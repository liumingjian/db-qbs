# ADR-0006: V1 校验只比行数，行 checksum 推迟到 V2，防线前移到映射预检

**状态**: 已接受
**日期**: 2026-08-13

## 背景

ADR-0002 把**校验**定成**切换**的前置门禁，校验内容是「行数 + 行 checksum」。
`CONTEXT.md` 的**校验**词条与**行 checksum** 词条按此写。

「两端 checksum 可比」不是一句话就能兑现的。ADR-0003 决定数值全程按字符串搬运，
于是必须有**规范形式**；规范形式要在目标端复现，MySQL 的每一处行为就都变成正确性约束。
spike §7.4 把这条链拉出来的硬约束有六条。M1 开工前，为把这件事钉死开了地图
[#14](https://github.com/liumingjian/db-qbs/issues/14)，下挂 7 张决策票——
其中 5 张（[#16](https://github.com/liumingjian/db-qbs/issues/16) 算在哪一侧、
[#17](https://github.com/liumingjian/db-qbs/issues/17) 哈希/拼接/编码/NULL 标记、
[#18](https://github.com/liumingjian/db-qbs/issues/18) 聚合算子，以及
[#13](https://github.com/liumingjian/db-qbs/issues/13) 的一半、
[#19](https://github.com/liumingjian/db-qbs/issues/19) 的一半）**只为行 checksum 存在**。

代价与收益在这里失衡：V1 的核心是把整条搬运链路建起来，而这 5 张票的产出
一行数据都搬不动。

## 决策

**V1 的校验只比行数。行 checksum 推迟到 V2。**

同时，**映射预检从「省事的做法」升格为硬门禁**：不通过不许发起运行，不是警告。

## 理由：两类错的性质不同

行 checksum 防的是**值被悄悄改掉**。V1 里能改掉值的路径已经点清，全部有更早、更便宜的防线：

| 值被改掉的路径 | 挡在哪里 | 出处 |
|---|---|---|
| `DECIMAL` 标度不够，插入时静默舍入 | **映射预检**：源目标精度逐位相等 | spike §7.4 第 1 条 / [#19](https://github.com/liumingjian/db-qbs/issues/19) |
| 无精度声明的 `NUMBER` 推不出目标精度 | **映射预检**：未显式指定即拒绝 | spike §7.4 第 2 条 / [#20](https://github.com/liumingjian/db-qbs/issues/20) |
| `TIMESTAMP(9)` 落进 6 位规范形式，静默丢 3 位 | **映射预检** | spike §7.4 第 4 条 / [#12](https://github.com/liumingjian/db-qbs/issues/12) |
| MySQL `CHAR` 读取时剥掉尾部空格 | **映射规则**：目标列建成 `VARCHAR(n)` | spike §7.4 第 5 条 / [#13](https://github.com/liumingjian/db-qbs/issues/13) |
| `DATE` 落进 MySQL `TIMESTAMP`，按会话时区改值 | **映射规则**：目标列建成 `DATETIME` | spike §7.4 第 3 条 |
| 白名单外类型被按某种默认形式搬过去 | **映射预检**：类型白名单，报错拒绝 | ADR-0003 |

这六条**全是系统性的错**：一旦发生，每一行都错，第一次运行就暴露。
用每次运行都算一遍的行 checksum 去等一个只会在第一次出现的错，是把成本放错了位置。

行数校验防的是**行丢了或多了**：批次没送到、断线重推、分页边界重复
（[#21](https://github.com/liumingjian/db-qbs/issues/21)）。**这类错才是每次运行都可能不同的**，
而且行数就抓得住。

一句话：**贵的那一半防的是不随运行变化的错，便宜的那一半防的是随运行变化的错。**

## 代价

**去掉行 checksum 不是删掉一道防线，是把它从运行时挪到预检。挪过去的那一头必须真的建起来。**

- **映射预检漏掉一类，就是静默搬错数据，且没有任何下游机制会发现。**
  ADR-0003 那句「正确性的单点上，宁可停机也不要猜」，全部重量压到预检上。
  因此预检**必须是硬门禁**：不通过不许发起运行。
- 行数相等但内容不同的情形，V1 抓不到。已知能造成这种情形的只有上表六条，
  它们各自有预检或映射规则挡着；**未知路径**（排序规则漂移、驱动换版本、字符集意外）
  V1 不再有兜底——这是本决策明面上买的风险。
- **`ZHS16GBK` 中文往返**（[#2](https://github.com/liumingjian/db-qbs/issues/2) 复验清单第 3 项）
  原本指望 checksum 在上线时抓出来。现在不行了，它必须在 #2 复验时**显式逐值比对**。

> **2026-08-13 增补（[ADR-0013](0013-verification-gate-row-counting.md)，
> [#29](https://github.com/liumingjian/db-qbs/issues/29)）——「只比行数」的口径已定死。**
> 两个数各有三种数法，选错就是门禁形同虚设：源端取 **fetch 循环累加器**、暂存表取
> **切换事务内的 `SELECT COUNT(*)`**，批数同级参与。本 ADR 那句「行数相等但内容不同 V1 抓不到」
> 已规格化为 ADR-0013 §9 的三条「校验**不**保证什么」，其中第 2 条是本 ADR 没点破的：
> **`source_rows` 是 source 自报的，门禁不覆盖「源库 → source 累加器」这一段。**

## 不受影响的

- **ADR-0003 的规范形式照旧。** 它不只服务 checksum，它同时是「Oracle 取到的字符串怎么写进
  MySQL」的规则。`NUMBER` 的恒等校验、类型白名单、`CHAR` 保留尾空格全部不变。
- **ADR-0002 的暂存表 + 原子切换照旧。** 校验仍然是切换的前置门禁，只是内容变了。
- spike §7.3 的类型映射表照旧，且 §7.4 的六条硬约束**一条都不能少**——
  它们现在是防线本身，不再是「为了让 checksum 可比」的推论。

## V2 回来时要接着做的

行 checksum 的决策空间已经勘过一遍，结论留在这里，不必重走：

- **算在哪一侧**：sink 侧 Rust 流式读回暂存表，不是 MySQL 一条 SQL。
  理由是可测试性——SQL 表达式按 70 列动态拼出来，没有单测、没有类型检查。
  代价是全表多读一趟，且 §4.2 的流式内存形状约束对称适用于 sink 侧。
- **`DECIMAL` 补零躲不掉**：规范形式 `1.23` 写进 `DECIMAL(18,6)` 读回来是 `1.230000`，
  源目标精度逐位相等**也不解决**（它保的是不舍入，不是读回字符串相等）。
  目标端重规范化是必需的一步，只是做在 SQL 里还是 Rust 里。
- **校验与切换之间要封口**：源端 commit 即为封口点，此后拒绝该 `run_id` 的任何批次写入。
  否则迟到的重推批次会让「切换进目标表的」不是「被校验过的」那份。
  V1 只比行数，这条同样成立，已写进 ADR-0002。
- 详细推演见已关闭的地图 [#14](https://github.com/liumingjian/db-qbs/issues/14) 与
  [#16](https://github.com/liumingjian/db-qbs/issues/16)。
