# ADR-0018: 延迟批次重推——失败分类、本地持久化、断路器与幂等替换

**状态**: 已接受
**日期**: 2026-08-14
**来源**: [#15](https://github.com/liumingjian/db-qbs/issues/15)
**关联**: [ADR-0002](0002-staging-table-atomic-swap.md)（暂存表与封口点）、
[ADR-0007](0007-single-query-streaming-fetch.md)（单查询流式取数）、
[ADR-0010](0010-http-protocol-contract.md)（M1 `retry = 0`）、
[ADR-0011](0011-batch-payload-wire-format.md)（批次原始载荷）、
[ADR-0012](0012-run-lifecycle-and-state-authority.md)（M1 状态只在内存）、
[ADR-0015](0015-staging-table-write-path.md)（每批一事务）、
[Oracle 11g 分页实测](../spikes/0001-oracle-driver.md#710-oracle-11g-分页批次边界实测21-2026-08-14)

## 边界

本 ADR 定义 **M4** 的批次失败处理模型，不改变已经交付的 M1 行为：M1 的 `/v1`
仍然不重试，任一批次失败即 abort 整个 run。这里说的重推是「Oracle 游标已经读完以后，
人工触发重放失败批次」，不是从某个 checkpoint 重新执行 Oracle 查询。

只处理「批次已经形成，但送往 MySQL 暂存表失败」这一段。Oracle fetch、describe、预检、
commit 和校验失败都不能通过重放某个批次恢复，不进本模型，照旧整 run 失败。

## 决策

### 1. 失败先分三类，未知错误默认整 run 失败

| 类别 | 判定 | 处置 |
| --- | --- | --- |
| `TRANSIENT` | HTTP 连接/读超时、连接重置、408/429/502/503/504；MySQL 1040/1205/1213 与客户端连接错误 2002/2003/2006/2013 | 首次命中时物化原始载荷并当场重试；耗尽后保留为失败批次并继续 |
| `DETERMINISTIC` | MySQL 1048/1264/1292/1366/1406 等值或列约束错误 | 不重试、不暂存，立即整 run 失败 |
| `RUN_FATAL` | Oracle 读失败、预检逃逸、协议错误、环境配置错误、commit/校验错误，以及所有未列出的错误 | 立即整 run 失败 |

重试名单是**闭集**：只有明确列在 `TRANSIENT` 中的错误才可重试。把未知错误先当瞬时错误会把
程序缺陷、协议漂移或坏数据藏在一次次重放后面。MySQL 错误由 sink 翻成稳定的失败类别，
source 只分类自己直接观察到的 HTTP 传输错误与状态码，不解析中文 `message`。

一次正常发送失败后，最多再试 **2 次**，等待固定为 **1 秒、2 秒**；总尝试次数是 3。
自动重试期间 Oracle 游标停止 fetch，因此上限必须小且不可配置，避免无界拉长
ADR-0007 的游标寿命。人工重试的每次点击仍使用同一组三次尝试。

### 2. 选择本地持久化原始载荷，不重新查询 Oracle

#21 在 Oracle 11g 上实测：即使排序键是全序，只要两次查询之间在页前提交一行，
本地表和 dblink 的第 3 页都会出现 `1 missing / 1 added`。因此只记批次号或分页边界不能重建
原批次。失败批次必须重放**首次形成的那份数据**。

source 在某批第一次命中 `TRANSIENT` 时，先持久化该批 HTTP JSON body 的原始字节，再把待确认记录
持久化到 manifest；两者都发布成功后才做两次自动重试。任一次尝试确认 sink 已提交后，
先在 manifest 中持久化该批的确认结果，再删除批次文件；三次均未确认则保留为失败批次。
目录形状固定为：

```text
spool/<run_id>/manifest.json
spool/<run_id>/batches/<seq>.json
```

`manifest.json` 只保存恢复所需的运行身份、原始 `POST /runs` 元数据、阶段、批次/行数总计、
批次确认结果、失败批次号、首次失败时间与过期时间。批次文件就是首次发送的 body，不重新规范化、
不重新查 Oracle。文件与 manifest 都必须用「同目录临时文件 -> `fsync` -> 原子 rename ->
目录 `fsync`」发布；发布失败或磁盘满即 `RUN_FATAL`，不能在没有可靠副本时继续。
崩溃发生在「持久化确认结果」与「删除批次文件」之间，只会留下一个可清扫的多余文件，
不能丢失待重推记录。

不为本地字节另拍 `50 MB` 之类的固定阈值。批次本身已有 16 MiB 断路器，真正决定系统是否还应
继续的是失败面，而不是不同表宽下意义不同的字节数。`spool_bytes` 必须进日志；写盘失败是硬失败。

只有 Oracle 游标正常读到 EOF、总行数和总批数已经确定的 run 才能进入可人工重试的
`INCOMPLETE`。source 若在 `STREAMING` 中退出，重启后无法证明尚未读取的行有哪些，必须 abort；
持久化过一两个失败批次不能把它变成断点续传。

### 3. 上限按失败面和时间拍，不按字节拍

一个批次经过三次尝试仍失败，才计为一个「失败批次」。每完成一批就检查两条断路器：

- 连续失败批次达到 **3**，整 run 失败；
- 累计失败批次超过 `max(1, floor(已处理批次数 / 10))`，整 run 失败。

流式读取期间，「已处理批次」同时包含已确认批次与失败批次；任一批确认成功都会把
「连续失败批次」归零。第二条的上限在已处理数不足 20 时固定为 1；从 20 起每增加 10 批，
上限增加 1，始终不超过当前已处理批数的 10%。
达到上限时仍可继续，**超过**才失败。任一断路器触发后立即 abort，已经物化的文件随 run 一起清理，
不再为了凑满一份明显失效的数据继续占着目标端暂存表。

`INCOMPLETE` 的 TTL 固定为 **24 小时**，从第一次进入该状态起算；人工点击不能续期，
也不另设点击次数上限。source 在启动时及运行期间每小时扫描一次：过期后标记
`FAILED(EXPIRED)`、调用幂等 abort，并告警。abort 未确认时，run 仍是 `FAILED(EXPIRED)`，
manifest 的清理阶段记为 `CLEANUP_PENDING`，下一轮继续清理；确认目标端已丢弃后才删除
整个 spool 目录。
source 停机期间没有程序能执行清理，重启后的第一次扫描必须先补做。

### 4. `__batch_no` 把未知发送结果变成幂等替换

网络超时不能证明 sink 没提交。所有批次（不是只有失败批次）写暂存表时都带内部列：

```sql
`__batch_no` BIGINT UNSIGNED NOT NULL,
INDEX (`__batch_no`)
```

映射预检保留该名字，源列或目标列命中 `__batch_no` 时拒绝。切换仍显式列出业务列，
`__batch_no` 不进入目标表。

首次发送与每次重推都执行同一个批次事务：

```sql
BEGIN;
DELETE FROM <stg> WHERE __batch_no = :seq;
INSERT INTO <stg> (`business columns...`, `__batch_no`)
VALUES (..., :seq), ...;
COMMIT;
```

DELETE 与 ADR-0015 的全部子 INSERT 必须在同一事务内。INSERT 失败则回滚并保留该批此前的完整版本；
连接在 COMMIT 回包前断掉时再次执行仍只留下一个版本。`rows_written` 只报告本次插入的业务行数，
不把 DELETE 行数算进去。

延迟重推允许批次号有缺口和乱序到达，原来的 `expected_seq` 不再能作接收门禁。commit 时改为同时验证：

- `COUNT(*) == source_rows`；
- 非空 run 的 `MIN(__batch_no) == 1`、`MAX(__batch_no) == source_batches`、
  `COUNT(DISTINCT __batch_no) == source_batches`；空 run 三者为空且 `source_batches == 0`。

这组判断证明收到的恰好是 `1..source_batches`，不能只比较请求次数。commit 仍是封口点；
封口后所有重推照旧拒绝。

### 5. 生命周期增加可持久化的 `INCOMPLETE`，但不扩大恢复承诺

延迟重推启用后，source 状态集合在 M1 五态之外增加 `INCOMPLETE` 与 `RETRYING`：

```text
STREAMING --全部读取且有失败批次--> INCOMPLETE --人工点击--> RETRYING
RETRYING --仍有瞬时失败-----------> INCOMPLETE
RETRYING --全部批次确认------------> COMMITTING --> SUCCEEDED
INCOMPLETE/RETRYING --确定性或未知错误/过期--> FAILED
```

人工重试按 `seq` 顺序重放 spool 中的文件。某批确认后先更新并持久化 manifest，再删除批次文件；
全部确认后才发 commit。重试中再次出现瞬时错误就回 `INCOMPLETE`，确定性或未知错误立即 abort。

`INCOMPLETE`/`RETRYING` 的 manifest 是恢复权威。source 重启时，`RETRYING` 归一为
`INCOMPLETE` 后等待人工点击；`STREAMING` 残留直接失败；`COMMITTING` 残留沿用 ADR-0012 的
一次 GET 诊断，不自动重发 commit。

sink 重启后，source 可以用本地 manifest 中的原始开 run 元数据，请求 sink **重新挂接**
既有暂存表：只允许挂接，不允许在恢复路径创建或覆盖表；必须重新跑目标元数据与暂存表结构校验，
并从 `__batch_no` 重建已收批次集合。暂存表不存在或结构不符即整 run 失败。这样既保留 ADR-0002
「已有表绝不 DROP 重建」，也不要求 sink 为未完成 run 另建一套持久状态库。

## 后果

- 瞬时目标端故障不再自动废掉整个 run，且未知提交结果不会制造重复行。
- source 为失败批次承担本地持久化与 24 小时清理责任；目标端暂存表在此期间继续占空间。
- 暂存表多一个内部列和索引，每次写批次前多一次 DELETE；这是换取幂等的固定成本。
- M1 `/v1`、当前五态、`retry = 0` 和无索引暂存表实现全部保持不变。实现本 ADR 时必须显式升级
  协议/表结构，不能悄悄改变 M1 契约。

## 实现顺序

1. 先落 sink 的 `__batch_no` 事务替换、序号集合门禁与重挂接；没有幂等接收前不得打开 source 重试。
2. 再落 source 的失败分类、三次尝试与断路器，保持人工重试关闭。
3. 最后落 durable spool、`INCOMPLETE`/`RETRYING`、人工入口与 TTL 清理，并做进程重启验收。
