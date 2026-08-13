# ADR-0010: M1 的 HTTP 协议契约——四动作 + abort，预检切在 sink，commit 同步

**状态**: 已接受
**日期**: 2026-08-13
**关联**: [ADR-0001](0001-rust-with-synchronous-io.md)（同步阻塞 IO）、
[ADR-0002](0002-staging-table-atomic-swap.md)（暂存表 / 封口点 / 建表失败）、
[ADR-0007](0007-single-query-streaming-fetch.md)（批次是纯推送切分单位、序号语义）、
[ADR-0008](0008-business-date-predicate-generation.md)（清除条件由目标端生成）、
[ADR-0009](0009-m1-mapping-precheck-rules.md)（映射预检规则——本 ADR 填它 §7 的空白）、
[#26](https://github.com/liumingjian/db-qbs/issues/26)

## 背景

`source` 与 `sink` 只通过 HTTP 通信，这四个动作的报文与错误语义**就是两个二进制之间的全部契约**。
M1 第一行代码就要它，而在此之前一张票都没有。

更要紧的是 ADR-0009 §7 给出的预检固定顺序里藏着一道墙：

```
1. describe 源端 SQL                  ← 只有 source 连 Oracle
2. 查目标端 information_schema        ← 只有 sink 连 MySQL
3. 按名字对齐，逐列比对
4. 全部通过 → 才建暂存表
```

**预检天然被 HTTP 劈成两半**，而 ADR-0009 写的时候没有协议票，没人说这一刀切在哪。
本 ADR 把它切开。

## 决策

### 1. 端点：REST 资源风格，四个动作 + 一个 abort

```
POST   /v1/runs                      开任务 = 提交源列元数据 + 目标端预检 + 建暂存表
POST   /v1/runs/{run_id}/batches     推批次
POST   /v1/runs/{run_id}/commit      封口 + 校验 + 切换（同步）
GET    /v1/runs/{run_id}             sink 侧计数事实
POST   /v1/runs/{run_id}/abort       幂等清理
```

`run_id` **进路径**，唯一实质理由是排障：抓包与访问日志里一眼看得出请求属于哪个 run，
不用解 body。M1 里人能抓住位置感的东西只有 `run_id` + 批次序号（ADR-0007），
别让它藏在 body 里。

**版本只在 URL 前缀 `/v1/`，不进 body。** 两个二进制永远同版本部署，这条纯粹是为了
「有人装错版本」时报错报得像句人话——不匹配就是 404，零协商成本。

### 2. `run_id` 由 source 生成

run 的生命周期在 source 侧开始得更早（describe 与日期谓词检查都在第一个 HTTP 请求之前）。
让 sink 分配要多一个来回，还得让 source 在拿到 id 之前先无名地干一段活。

同 `run_id` 撞车由 ADR-0002 增补的「表已存在 = 硬失败、绝不 DROP 重建」兜住——
那正是 21 字符形态里 6 位随机段存在的理由。

### 3. 预检切在 sink：`POST /runs` 一个来回做完预检 + 建表

**`POST /runs` 不是「只开任务」**，它是：提交 describe 出的源列元数据 → sink 查
`information_schema` → 按名字对齐逐列比对（ADR-0009 §2~§6）→ 生成 DDL 建暂存表 →
预检报告随同一个响应返回。不通过就是 422，body 带 ADR-0009 §8 要求的逐列清单。

不拆成 `precheck` + `open` 两个动作：预检每 run 跑一次、不缓存（§7），
「只想验一下不建表」在 M1 没有使用者；拆开反而多一个「验过了但还没开」的中间态，
而 M1 没有状态存储去持有它。

#### 3.1 source 一个判断都不做——全部判定集中在 sink

ADR-0009 §4 的拒绝清单里**有一半只需源端信息就能判**（白名单外类型、裸 `NUMBER`、
`s > p`、负标度、`s > 30 || p > 65`、表达式列精度不明），source 手上 describe 完就全知道了。
**但它一条都不判**，原样上报，包括 `CLOB`、包括 precision 为 null 的表达式列。

理由是 ADR-0009 §8 那条「一次报全部列」，它的存在理由写得很清楚：生产表 70 列，
「改表 → 重跑 → 又炸一条」的循环代价是实打实的。**让 source 先判，等于把这个循环从
「逐列」降级到「逐端」，但没消灭它**——源端先炸一批类型问题，人改完 SQL 重跑，
紧接着又炸一批目标端精度问题。一次报全部就得有一个地方看得见全部，
那个地方只能是 sink：目标端元数据过不了这道墙，源端元数据可以。

**推论（容易漏，写死）**：`type` 是**驱动给出的类型名原样透传的自由字符串**，不是三值枚举。
否则 source 根本没法把 `CLOB` 报上去让 sink 拒。**协议不认识 Oracle 的类型系统，它只是搬运工。**

#### 3.2 源列元数据的形状

```json
[ { "name": "N_VA_PRICE", "type": "NUMBER",   "precision": 18, "scale": 4 },
  { "name": "C_NAME",     "type": "VARCHAR2", "length": 50 },
  { "name": "D_BIZ",      "type": "DATE" },
  { "name": "EXPR_1",     "type": "NUMBER",   "precision": null, "scale": null } ]
```

`precision` / `scale` / `length` 全部可空——表达式列就是全空，正好落进 ADR-0009 §4 的拒绝清单。

`VARCHAR2` 的 `n CHAR` / `n BYTE` 语义**不进协议**：ADR-0009 §2 已定「按 `n` 的数值直接比，
不做字符/字节换算」，带上它只会诱人去实现那个换算。

#### 3.3 `source_columns` 的顺序是批次行值的列序基准

ADR-0009 §3 的「按名字对齐、顺序无关」是**预检**的规则。批次里的值总得按某个顺序排，
那个序的锚只有本 ADR 能给（`source_columns` 是本 ADR 的字段）：
**`POST /runs` 里 `source_columns` 的顺序，即批次行值的列序基准。**

[#27](https://github.com/liumingjian/db-qbs/issues/27) 若选按序数组，锚已经在了；
若选按名字的对象，这条约定闲置，不碍事。

**两条推论**：

- **暂存表的列序照目标表，不照 `source_columns`**。ADR-0002 增补说「复制的是列名、列序与列类型」，
  切换事务的 `INSERT INTO <target> SELECT * FROM stg` 正是靠这个列序对上的。
  sink 建暂存表时读 `information_schema` 的 `ORDINAL_POSITION`。
- 于是 ~~**sink 写暂存表时必然要做一次列序重排**（`source_columns` 序 → 目标表序）。
  这不是可以省掉的一步。~~ 两个序混用的话，预检全过、数据整整齐齐灌进错的列——
  正是 ADR-0009 §3 警告的那类静默搬错。

> **2026-08-13 订正（[ADR-0015](0015-staging-table-write-path.md) §2，
> [#33](https://github.com/liumingjian/db-qbs/issues/33)）：那次重排可以省掉，且省掉之后更强。**
> sink 的 `INSERT` **显式列出列名、按 `source_columns` 序**，行值原样绑定，置换交给 MySQL 做。
> 代码里没有映射表，于是「两个序混用」不是被断言拦住的错误，而是**不可表示**——
> 列名把每个值钉到了具体的列上，写错即 `ERROR 1054` 当场炸。
> 本节保留的仍然成立：**暂存表列序照目标表 `ORDINAL_POSITION`**；
> 但切换语句同时改为显式列名（`INSERT INTO <target> (cols…) SELECT cols… FROM stg`），
> 不再依赖 `SELECT *` 的列序。

### 4. commit 同步：一个请求做完封口、校验、切换

「封口 → 数暂存表行数 → 比对 → 切换事务 → DROP 暂存表」全在 commit 请求内完成。
成功 200 带四个行数，失败带错误分类。

不做异步：那要引入「切换中」这个持久状态、轮询节奏、超时判定，而 **M1 没有状态存储**——
sink 崩在切换途中的话，异步方案里 source 永远问不到答案。同步方案下这种情况就是
一个断开的连接 = 整 run 失败，语义干净。

**代价必须钉进规格，不留给实现去猜：commit 是长请求。** 10 万行 `INSERT ... SELECT`
的事务，ADR-0002 明说锁持有时间要实测。source 对 commit 端点的读超时**单独设 30 分钟**，
不能吃默认的几十秒——否则 source 超时放弃而 sink 那边切换正在成功提交，**两端认知直接分裂**。

### 5. 重复投递：从源头消灭，不在 sink 里容忍

**source 的 HTTP 客户端显式配置 `retry = 0`**，关掉库的自动重试；任何发送或超时错误
立刻整 run 失败、不重发。

「不做重试模型 ≠ 不会重复投递」这句话是对的，但反过来说：**重复投递的唯一来源是 source 重发**，
而「失败即整 run 失败」本来就不允许 source 重发任何东西。所以正面答案是
**在协议里显式禁止 source 重发，而不是在 sink 里容忍重发**。

sink 侧对 `seq` 的**双向断言**保留（`seq < expected` 与 `seq > expected` 都硬失败），
但它的定位是**防御性断言**：一旦触发说明本节的前提被违反了，那是缺陷不是故障。

不取「sink 认出重复、回放上次 `rows_written` 并放行」那条路：它要求 sink 记住上一批的结果，
且直接滑向重试模型（[#15](https://github.com/liumingjian/db-qbs/issues/15)，M2 之后）。

**代价说清楚**：一次瞬时网络抖动废掉整个 run，重跑 = 一条新 `run_id` 从头。
这与 M1 的失败语义完全一致，不新增任何机制。

### 6. abort：要，但不承诺可靠性

[#32](https://github.com/liumingjian/db-qbs/issues/32) 把孤儿回收踢出 M1，
明码标价的代价是「崩溃遗留的暂存表一直占空间直到有人手工清」。但
**「source 进程还活着、只是这个 run 失败了」是最常见的失败形态**——这一类不该变成孤儿。
一个 `DROP TABLE` 的代价买掉绝大多数遗留表。

**但 abort 本身失败只记日志**，不重试、不改变「这个 run 已经失败」的事实。

**abort 幂等，且「sink 不认识这个 run」就是 abort 想要的结果**——回 200 不回 404。
清理动作的成功判据是「东西没了」，不是「我亲手删的」；回 404 会让 source
在本来就已经失败的路径上再纠结一次「我该不该慌」。

### 7. 错误：人话在 sink 侧成文，source 只加前缀透传

```json
{ "error": {
    "code": "PRECHECK_FAILED",
    "message": "<中文人话，直接给人看的最终措辞>",
    "run_id": "20260813091530_a3f19c",
    "details": { }
} }
```

两端已有一批 ADR 钉死「必须翻成人话、不得裸抛错误码」的错误（`ORA-01555`、
`ERROR 1118` / `1366`、表已存在须带表名里的时间、权限不足须点名缺 `CREATE` 还是 `DROP`）。
**这些措辞在 sink 侧成文，source 只加前缀「目标端：」透传，绝不重写**——
措辞和产生它的元数据都在 sink 手上，让 source 隔着协议重新拼一遍，
就是给「两端措辞漂移」开门。

M1 分类码闭集：

| `code` | HTTP | 说明 |
|---|---|---|
| `PRECHECK_FAILED` | 422 | `details` 带 ADR-0009 §8 的逐列清单 |
| `STAGING_CREATE_FAILED` | 409 / 500 | `details` 带子类 `TABLE_EXISTS` / `PERMISSION_DENIED` / `OTHER`（ADR-0002 增补四类） |
| `SEQ_MISMATCH` | 409 | 第 5 节的防御性断言，`details` 带 expected / got |
| `RUN_SEALED` | 409 | ADR-0002 封口点明文要求的拒绝 |
| `RUN_UNKNOWN` | 404 | |
| `VERIFY_FAILED` | 409 | `details` 带两端行数（口径归 [#29](https://github.com/liumingjian/db-qbs/issues/29)） |
| `SWAP_FAILED` | 500 | |
| `INTERNAL_PRECHECK_ESCAPE` | 500 | 见下 |
| `PAYLOAD_TOO_LARGE` | 413 | |
| `BAD_REQUEST` | 400 | `Content-Type` 非 `application/json` 为 **415** |

**`INTERNAL_PRECHECK_ESCAPE` 是专为哨兵加的。** ADR-0009 说 `Note 1265`
（预检漏网的静默舍入）一旦出现**属于 P0 缺陷而非运行故障**。这个区分必须在协议上可见，
否则运维看到的就是又一条运行错误。message 明写「这是程序缺陷，不是数据或环境问题，请报 issue」。
**协议层把缺陷和故障分开，这是 ADR-0009 那条要求唯一能兑现的地方。**

#### 7.1 非常规时序的响应矩阵

| 情形 | 响应 |
|---|---|
| 批次 → 未知 `run_id` | 404 `RUN_UNKNOWN`，硬失败。**不试图重建暂存表** |
| 批次 → 已封口的 run | 409 `RUN_SEALED` |
| 批次 → 已 abort 的 run | 404 `RUN_UNKNOWN`（abort 后 sink 忘掉它，等价于从不认识） |
| `POST /runs` 同一 `run_id` 两次 | 409 `STAGING_CREATE_FAILED` / `TABLE_EXISTS`。**天然由 ADR-0002「绝不 DROP 重建」兜住，不需要额外机制** |
| commit → 未知 / 已 commit 的 run | 404 `RUN_UNKNOWN`（切换成功后 sink 忘掉，暂存表已 DROP） |
| abort → 未知 run | **200**（第 6 节） |

### 8. 目标库与暂存表名

**database 来自 sink 的连接配置，不进协议；请求只带 `target_table` 裸表名。**
一个 sink 进程服务一个目标库，这是 M1 的部署形态。让请求带 database 等于允许 source
指定往哪个库写——**那是把「写哪儿」的权限从部署配置挪到了报文里**，而 V1 没有鉴权。

**暂存表名由 sink 拼**，在响应里回报给 source（source 只用于记日志，不用它拼任何东西）。
因为 ADR-0002 增补那条「目标表名 > 37 字符即拒绝」是**预检的一条**，
判它的人和拼它的人得是同一个。

### 9. 空结果集：放行，不特判

源端当天一行都没查到 → 不推任何批次 → commit `total_rows = 0` → 校验 `0 == 0` 通过 →
切换事务 **DELETE 掉目标表当天的旧数据、INSERT 0 行**，目标表当天被清空。

这正是 ADR-0008 反复警惕的那类失效，只不过这次是「合法地」发生的。**仍然放行。**

系统**没有任何办法**区分「源端当天真的没数据」和「SQL 写错了 / 业务日期填错了」——
两者在协议这一层长得一模一样。加一条「0 行就拒绝」的规则，只会逼着人在真的没数据那天
去绕过系统手工操作，那比清空危险得多。

**买一个便宜的保险**：commit 响应回报 `purged_rows`。「推了 0 行、删了 8 万行」这个组合
在响应里是明摆着的，人看得见。**能看见和被拦住，在 M1 的取舍里是两回事——这里只买前者。**

### 10. 传输与连接参数：写死，不做配置项

`Content-Type: application/json`，非此值 415。**M1 不压缩**——两端内网、总量 100MB 量级，
带宽不是瓶颈，而不压时抓包能直接读出是哪一列的什么值出的问题，M1 排障价值大。
~~压缩的口子留在 `Content-Encoding`，等 [#27](https://github.com/liumingjian/db-qbs/issues/27)
定了载荷形状再看。~~单请求体上限 **64 MiB**（超出 413）。

> **2026-08-13 增补（[ADR-0011](0011-batch-payload-wire-format.md) §6~§8，
> [#27](https://github.com/liumingjian/db-qbs/issues/27)）——三处订正：**
> 1. **「约几 MB/批」低估了一个数量级**：按 spike §4.6 的 3 kB/行估，JSON 化后典型批次**约 15 MB**。
> 2. **64 MiB 不再是「正常路径的上限」，是断路器。** ADR-0011 §6 给批次加了 16 MiB 的字节预算
>    （5000 行或 16 MiB 先到先切），M1 里触发 413 等同于**预算逻辑有 bug**——
>    **缺陷不是故障**，与 `INTERNAL_PRECHECK_ESCAPE` 同类，`PAYLOAD_TOO_LARGE` 的 message 按此措辞。
> 3. **压缩的复审线关闭**：`Content-Encoding` 连口子都不留，理由正是本节自己给的排障价值。
>    要压是 M2 的事，届时连同重试模型一起改。

| 参数 | 值 |
|---|---|
| 连接超时 | 10s |
| 读超时（`/runs`、`/batches`） | 60s |
| 读超时（`/commit`） | **30 分钟**（单独一档，第 4 节） |
| 读超时（`/abort`） | 30s |
| 自动重试 | **0**（第 5 节） |
| 连接 | HTTP/1.1 + keep-alive 复用，单连接串行 |

写死成常量、不做配置项，与 ADR-0007 的「5000 行 / `fetch_array_size` 100 写死不可配」同一态度。
ADR-0007 已定 fetch 与推送串行，本来就不存在并发请求。

那 30 分钟是**猜的**：真值要等台架 10 万行验收。**把「commit 实际耗时」加进 ADR-0007 已要求的
三个测量数（fetch 累计耗时 / 推送累计耗时 / 全程游标寿命）后面，凑成第四个**，
等真数据回来再谈调值——而不是现在开一个没人知道该填什么的配置项。

**台架跑 HTTP、生产跑 HTTPS，协议报文一个字不变**；TLS 与证书归部署配置，不进本规格。

## 报文定稿

**`POST /v1/runs`**
```json
→ { "run_id": "20260813091530_a3f19c",
    "target_table": "T_POSITION",
    "target_date_col": "D_BIZ",
    "biz_date": "2026-08-13",
    "source_columns": [ … 见 §3.2 … ] }
← 200 { "run_id": "…", "staging_table": "T_POSITION__stg_20260813091530_a3f19c",
        "columns_checked": 70 }
← 422 PRECHECK_FAILED / 409 STAGING_CREATE_FAILED
```

粒度（DAY，不可配）与 `source_date_col` **不进协议**——ADR-0008 的三字段里
只有 `target_date_col` 跨得过这道墙，另两个是源端自己的事。

**`POST /v1/runs/{run_id}/batches`**
```json
→ { "seq": 1, "rows": [ … ] }            // rows 元素形状归 #27
← 200 { "seq": 1, "rows_written": 5000, "next_seq": 2 }
← 404 RUN_UNKNOWN / 409 RUN_SEALED / 409 SEQ_MISMATCH / 413 PAYLOAD_TOO_LARGE
```

`rows_written` = 本批 `INSERT` 的影响行数。它让「推了 5000 行只落 4999」**当场定位到第 n 批**，
而不是等 commit 才发现总数差一。~~**校验口径归 [#29](https://github.com/liumingjian/db-qbs/issues/29)，
本 ADR 只保证这个字段存在。**~~

> **2026-08-13 增补（[ADR-0013](0013-verification-gate-row-counting.md) §3，#29）**：
> 这个字段**不参与 commit 门禁的算式**，它转岗为 **source 侧的逐批硬断言**——
> 每批响应回来立刻比 `rows_written == 本批行数`，不等则当场整 run 失败、错误带 `seq`。
> 「当场定位」只在 source 当场断言时才兑现；不断言的话它只是一个没人看的数字。

**`POST /v1/runs/{run_id}/commit`**（读超时 30 分钟）
```json
→ { "total_batches": 20, "total_rows": 100000 }
← 200 { "source_rows": 100000, "staged_rows": 100000,
        "purged_rows": 82345, "swapped_rows": 100000 }
← 409 VERIFY_FAILED / 500 SWAP_FAILED / 500 INTERNAL_PRECHECK_ESCAPE / 404 RUN_UNKNOWN
```

> **2026-08-13 增补（[ADR-0013](0013-verification-gate-row-counting.md)，#29）——校验口径定稿，三处订正：**
> 1. **门禁是两组数不是一组**：`source_rows`（source 的 fetch 累加器）vs `staged_rows`
>    （切换事务内 `SELECT COUNT(*) FROM stg`），**外加 `total_batches` vs sink 收批计数**——
>    整批丢失在行数上像数据问题，在批数上一眼是传输问题。
> 2. **`VERIFY_FAILED` 报文扩到五个数**：四个门禁数 + 诊断数 `sink_reported_rows`
>    （sink 逐批 `rows_written` 之和），它把失败自动分成「写进去了又没落」与「批次没到齐」两类。
> 3. **明确不承诺定位到批次**：逐批不符已由 §3 的 source 侧断言当场炸掉，
>    走到 commit 的不一致按定义是「每批都自称写对了」，指不出任何一批——
>    **sink 不保留 `seq → rows_written` 明细**。

前两个是校验门禁的两边；`purged_rows` 是 §9 的保险；
`swapped_rows` 看似冗余于 `staged_rows`，但它是**唯一能证明切换事务真的搬完了**的数字——
两者不等意味着 `INSERT ... SELECT` 出了 ADR-0002 增补预言过的那类事
（暂存表无唯一约束，重复键要到切换才炸）。

**`GET /v1/runs/{run_id}`**
```json
← 200 { "run_id": "…", "staging_table": "…", "batches_received": 12,
        "rows_written": 60000, "sealed": false }
← 404 RUN_UNKNOWN
```

**尽力而为的内存快照，不是 run 的权威状态；sink 重启后返回 404 属正常，不是故障。**
commit 同步之后没有任何**程序**需要问 sink「现在怎么样了」，唯一不可替代的是那个布尔：
**sink 到底认不认识这个 `run_id`**。批次报 404 时，人第一个要分辨的是「sink 重启了」
还是「`run_id` 打错了」，这件事只有 sink 能答，MySQL 那边查不出来。进度数字是顺带的。

~~**本 ADR 不定义任何状态名**——状态集合、迁移、谁持有权威状态归
[#28](https://github.com/liumingjian/db-qbs/issues/28)。~~

> **2026-08-13 增补（[ADR-0012](0012-run-lifecycle-and-state-authority.md) §5，
> [#28](https://github.com/liumingjian/db-qbs/issues/28)）——本节两处订正：**
> 1. **已终结的 run 不再回 404。** sink 为终态 run 保留内存墓碑（FIFO 32 条），
>    `GET` 回 **200 带 `terminal`**（`SWAPPED` / `DISCARDED`）+ `purged_rows` / `swapped_rows`；
>    `terminal` 缺席即代表「还活着」。被淘汰或 sink 重启后回 404，退化成本节原行为、不是故障。
>    动机正是本节自己留下的洞：**commit 连接断掉后 source 来问，404 的答案是「不知道」**。
> 2. **§7.1「commit → 已 commit 的 run」由 404 `RUN_UNKNOWN` 改为 409 `RUN_SEALED` + 墓碑。**
>    「批次 → 已 abort 的 run 回 404」不变——那条 404 是故意的语义等价。
>
> 状态集合、迁移、权威归属见 ADR-0012；**协议层不新增任何错误码**。

**`POST /v1/runs/{run_id}/abort`**
```json
→ {}
← 200 { "run_id": "…", "staging_dropped": true }   // false = 本来就没有，同样是 200
```

## 后果

**买到的**：两个二进制之间不再有口头约定。预检那道跨端的裂缝被正式缝上（ADR-0009 §7 的空白）；
ADR-0002 的封口点有了落地的 `RUN_SEALED`；ADR-0009 的 `Note 1265` 哨兵有了
`INTERNAL_PRECHECK_ESCAPE` 这个落脚点，缺陷与故障在协议层分得开。
「重复投递」被正面回答，而且是靠**消除**而不是靠容忍。

**付出的**：

- **一次瞬时网络抖动废掉整个 run**（§5）。这是 `retry = 0` 的直接代价，与 M1 失败语义一致。
- **commit 是一个可能长达数十分钟的 HTTP 请求**（§4）。30 分钟超时是猜的，
  台架验收必须把 commit 实际耗时测回来。
- **空结果集会静默清空目标表当天数据**（§9）。只买了「人看得见」，没买「被拦住」。
- **sink 必须做列序重排**（§3.3）。两个列序混用会造成静默搬错，实现时是个真陷阱。
- **`type` 是自由字符串**（§3.1）。协议不校验它，全靠 sink 的预检把关；
  代价是报文里可能出现任何字符串，sink 必须对未知类型给出好的错误信息而不是 panic。

**对其他 ADR 的影响**：ADR-0009 §7 由本 ADR §3 补全（预检的第 1 步在 source、
第 2~4 步在 sink，`POST /runs` 是缝合处）。ADR-0002、ADR-0007、ADR-0008 不受影响，
本 ADR 是它们在协议层的兑现。

## 时效

M1 报文定稿，**`/v1` 前缀就是它的时效声明**。最可能触发复审的三条：
台架测回的 commit 实际耗时（→ 30 分钟超时）、
[#27](https://github.com/liumingjian/db-qbs/issues/27) 定下的载荷形状（→ 要不要开压缩、64 MiB 上限是否够）、
[#28](https://github.com/liumingjian/db-qbs/issues/28) 的状态机（→ `GET` 的返回是否要扩）。
