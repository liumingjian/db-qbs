# ADR-0017: 运行日志的形态与最小契约——JSON Lines、公共五字段、逐批常态打、定位到列与值

**状态**: 已接受
**日期**: 2026-08-14
**来源**: [#36](https://github.com/liumingjian/db-qbs/issues/36)
**关联**: [ADR-0007](0007-single-query-streaming-fetch.md)（`ORA-01555` 须翻人话、批次序号须进日志）、
[ADR-0009](0009-m1-mapping-precheck-rules.md)（一次报全部列、`ERROR 1118`/`1366`、§10 域外日期缺口）、
[ADR-0010](0010-http-protocol-contract.md)（`code` 十码闭集、人话在 sink 成文由 source 透传）、
[ADR-0011](0011-batch-payload-wire-format.md)（批次字节预算、`PAYLOAD_TOO_LARGE` 是缺陷不是故障）、
[ADR-0012](0012-run-lifecycle-and-state-authority.md)（状态不落盘——日志是唯一事后取证手段）、
[ADR-0013](0013-verification-gate-row-counting.md)（校验失败即 `DROP` 暂存表、不留现场）、
[ADR-0015](0015-staging-table-write-path.md)（开连接仪式失败是 sink 级、点名第 `seq` 批）、
[ADR-0016](0016-task-definition-form.md)（source 一次性进程、任务路径是任务身份；**§9 由本 ADR 订正**）

## 背景

「日志与可观测性的最小形态」是 M1 规格图收图时剩下的三块雾之一，而它迟迟定不下来的原因
不是难，是**没有消费者**：十来条 ADR 陆续往里钉了措辞要求与错误码，却没有一个人能说清
「这些字将来被谁、以什么方式读」。没有消费者，「记什么」就只能靠猜。

两条已定决策同时把这块雾的分量抬到了 M1 的取证底线：

- **ADR-0012 §2**：run 状态不落盘 —— 日志是 M1 唯一的事后取证手段。
- **ADR-0016 §3**：source 是一次性进程 —— 进程退出即什么都不剩，**stdout 就是全部取证面**。

[#36](https://github.com/liumingjian/db-qbs/issues/36) 用三张低保真线框做了一次定向取样，
把 M2 Web UI 的观察面反推成一份数据项清单。取样最实的产出不是清单本身，是**认出了消费者**：
M1 日志的读者不是坐在终端前的人，是**排障时的 Agent 与 M2 的采集侧**。
本 ADR 把取样定下的五条约束落成规格。

本 ADR 的第 1～5 节先定形态与契约规则；**第 6 节随 M1 实现定稿 `event` 闭集、必带字段与
日志去向**，并把已有 ADR 反推出的八条日志行下界逐条落到事件上。

## 决策

### 1. 载体是 JSON Lines：stdout 每行一个 JSON 对象，source 与 sink 同一套

人话不消失，它是 `message` 字段的内容（措辞归属仍按 ADR-0010 §7：sink 成文、source 透传，
SQL 形状预检是唯一例外，在 source 本地成文——ADR-0016 §4）。变的是**结构的位置**：

> **结构在字段上，不在排版上。**

判据是消费者：JSON 之外的任何形态，都要求下游为「排版」写一个正则解析器，
而排版是最容易在改一句措辞时被无意破坏的东西。ADR-0016 §3 之后 stdout 是全部取证面，
把取证面的结构留在排版里，等于让 M1 的每一次措辞微调都成为下游的破坏性变更。

代价明写：**人裸眼看 stdout 变难，要配 `jq`**。这是有意识付的——把可读性放在人这一侧，
比把可解析性放在机器那一侧更容易补救（`jq` 一行即可还原人话，反向则要写解析器）。

#### 1.1 订正 ADR-0016 §9

ADR-0016 §9 定的是「`POST /runs` 成功后立刻把 `run_id` **单独打一行**到 stdout，
让验收脚本**无解析**地抓到它」。stdout 全量 JSON Lines 之后，这一行会成为该流上
**唯一的非 JSON 行**——契约刚立就有例外，而例外正是解析器最先撞上的东西。

**订正**：取消裸行，`run_id` 由 `POST /runs` 成功后立刻打出的那条 JSON 行承载
（公共字段里本就有 `run_id`，见第 2 节）。ADR-0016 §9 的诉求（脚本能抓到它）不变，
兑现方式从「无解析」变成「一次 `jq`」。**其余不变**：退出码仍只有 `0` / `1`。

代价：验收脚本依赖 `jq`。相对于「stdout 上有两种行形态」，这个代价便宜得多。

### 2. 公共字段固定五个，`run_id` 可以是 `null`

| 字段 | 类型 | 说明 |
|---|---|---|
| `ts` | string | UTC、RFC3339。与业务日期无关（业务日期是无时区日历日，ADR-0008） |
| `level` | string | `info` / `warn` / `error` |
| `event` | string | 事件名。闭集与逐事件必带字段见第 6 节 |
| `run_id` | string \| **null** | ADR-0002 增补钉死的单一形态；**可为 `null`** |
| `task` | string \| null | 任务定义的**绝对路径**。sink 侧恒为 `null`（它对任务定义一无所知，ADR-0016 §5） |

**`run_id` 可以是 `null` 是本节唯一需要论证的一条**：SQL 形状预检失败时 run 根本没发起
（ADR-0016 §4：不过则不发 HTTP 请求，sink 不知道存在这个 run），此时没有 `run_id` 可打。
若用「字段缺席」表示，「这条日志没有 run_id」与「这条日志忘了打 run_id」在下游不可区分；
显式 `null` 让「这个 run 还不存在」成为一个**可断言的事实**。
这与 ADR-0011 §2 用 JSON `null` 而非带内哨兵区分 NULL 与空串，是同一种手法。

`task` 是任务的**唯一身份**——ADR-0016 §7 已证明 `run_id` 里塞不进任务信息，
而 M1 没有任务注册表，路径就是身份。它从「日志第一行」升为**公共字段**：
一次运行的每一行都带着它，采集侧才能按任务聚合（清单第 17 项）。

### 3. 逐批一行、全字段、常态打，不挂开关

`batch_pushed` 每批一行，必带 `seq` / `rows` / `bytes` / `written` / `ms`。

**这不是「为了好看」，是三条已有验收要求的最省实现**：

| 已有要求 | 出处 | 由逐批日志兑现 |
|---|---|---|
| 批次实际序列化字节数的分布 | ADR-0011（16 MiB 与 5000 行的配比是按 3 kB/行估的） | `bytes` |
| fetch 累计耗时 / 推送累计耗时占比 | ADR-0007（推送占比过半则「串行不做流水线」要复审） | `ms` 累加 |
| `sum(affected_rows) != rows.len()` 时**点名第 `seq` 批** | ADR-0015 | `seq` + `written` |

常态打之后，这三条**不需要另立一套「只在验收时打开」的埋点**。
不挂开关的理由是负面的：**开关是会忘记打开的东西**，而这三个数只有在真出问题的那一次
才有价值——那一次恰恰是没人提前打开开关的一次。

量级不构成理由：10 万行按 ADR-0011 的切分约 20–37 批，即 20–37 行。
`rows` 与 `bytes` 都必须打，因为**批次不恒为 5000 行**（16 MiB 先到先切），
两个数缺一个就无法判断这一批是被行数切的还是被字节切的。

### 4. 失败定位打到列与值

错误行在已有的 `code` / `message` 之外，带 `column` 与 `value`（不适用时为 `null`）。

这直接兑现 [#35](https://github.com/liumingjian/db-qbs/issues/35) 给 `ERROR 1292` 定的
「措辞要点名**是哪一列、哪个值**」——**该要求不需要订正**。
同一形状适用于 `ERROR 1264`（整数位溢出）、`ERROR 1366`（字符集）等逐值失败路径。

**代价必须写在明处，并作为负面条款进规格**：

> **M1 的日志文件含业务数据值。** 其权限口径与 ADR-0016 §8 的凭据同档（**0600**），
> 且**不得**因为「日志」这个名字就被按普通运维文件对待——不 `chmod 644`、
> 不在未经确认的情况下采集到目标端之外。

不写这一条，部署时它会被随手放开：凭据文件长得像秘密，日志文件不像，
而在本决策之后它们装的是同一档东西。

**边界**：打的是**引发错误的那个值**，不是「顺手把行内容 dump 出来」。
`batch_pushed` 这类成功路径的日志行**不含任何业务数据值**。

### 5. 格式契约：字段集合稳定，排版不保证

- **进契约**：`event` 的取值，以及每个 `event` **必带的字段集合**。
- **不进契约**：`message` 的文字（随时可改）、字段顺序、JSON 的空白排版。
- **演进规则**：字段**只增不删、不改义**。删字段或改字段含义是破坏性变更，
  须与 M2 的消费者一并处理。

这条几乎是白拿的——JSON 之后它只是一句承诺——但它决定了一件真东西：
**M2 能不能靠采集 M1 的日志把「运行历史」拼出来**（见第 7 节 C1）。
没有这条承诺，那条路线在技术上可行、在工程上不可依赖。

### 6. M1 定稿：stdout、`event` 闭集、必带字段与措辞归属

#### 6.1 日志只写 stdout

`source` 与 `sink` 都只向 **stdout** 写 JSON Lines；程序自身不创建日志文件、不轮转、不外采，
stderr 也不另开一套排版。是否把 stdout 重定向到文件由部署者决定。一旦落盘，文件就含失败值：
创建前须 `umask 077`，最终权限必须为 **0600**，且不得采集到目标端之外。

一次 `jq` 即可取证，不依赖字段顺序或空白：

```sh
jq -c 'select(.run_id == $run_id and (.event == "run_opened" or .event == "run_finished"))' \
  --arg run_id "$run_id" run.jsonl
```

#### 6.2 `event` 闭集与每个事件的必带字段

下表是 **M1 完整闭集**。每行都先带第 2 节公共五字段；「必带字段」是在公共字段之外追加的字段。
字段存在不等于值一定非空：`run_finished` 的门禁数在失败发生于门禁之前时为 JSON `null`；
错误不适用于某一列或无法从数据库错误定位时，`column` / `value` 同样显式为 `null`。

| `event` | 产生端 | 必带字段 |
|---|---|---|
| `cli_failed` | source | `message` |
| `source_started` | source | `biz_date`, `message` |
| `business_date_invalid` | source | `value`, `message` |
| `source_config_failed` | source | `message` |
| `task_config_failed` | source | `message` |
| `sql_shape_precheck_failed` | source | `problems`, `message` |
| `sql_shape_precheck_passed` | source | `message` |
| `stage_changed` | source | `stage`, `message` |
| `mapping_precheck_failed` | source（sink 成文后透传） | `column`, `source`, `target`, `rule`, `message` |
| `run_opened` | source（`POST /runs` 成功后） | `staging_table`, `columns_checked`, `message` |
| `batch_pushed` | source | `seq`, `rows`, `source_rows`, `bytes`, `written`, `ms` |
| `commit_diagnosed` | source | `terminal`, `message` |
| `abort_failed` | source | `message` |
| `run_finished` | source | `terminal`, `stage`, `message`, `source_code`, `sink_code`, `column`, `value`, `source_rows`, `source_batches`, `staged_rows`, `received_batches`, `sink_reported_rows`, `purged_rows`, `fetch_ms`, `push_ms`, `commit_ms`, `cursor_ms` |
| `sink_unavailable` | sink | `message` |
| `http_response_failed` | sink | `message` |

`mapping_precheck_failed` **每个不合格列一行**，sink 仍在一次响应里返回全部列，source 再逐项写行；
不会退回「改一列、重跑、再发现下一列」。`batch_pushed.source_rows` 是 fetch 累加器到本批为止的
累计值，`rows` 仍只表示本批。逐批全字段常态写，不挂开关。

`run_finished` 每个已发起的 run **恰好一行**。成功时门禁四数、`sink_reported_rows`、
`purged_rows` 与三个分段耗时都有数值；失败时也保留已经产生的数值，未知项才为 `null`。
`cursor_ms` 是 Oracle 游标从 describe 完成到 run 终结的寿命，不拿进程启动耗时冒充。

#### 6.3 八条日志下界逐条落点

| 下界 | 事件与字段 | 出处 |
|---|---|---|
| run 起点与业务日期 | `source_started.task` / `biz_date` | ADR-0016 §7 / ADR-0008 |
| 阶段迁移 | `stage_changed.stage`，取五状态之一 | ADR-0012 |
| 累计行数 | `batch_pushed.source_rows`，终态再由 `run_finished.source_rows` 封口 | ADR-0013 |
| 逐批行数、字节、耗时 | `batch_pushed.rows` / `bytes` / `ms`，并带 `seq` / `written` | 第 3 节 / ADR-0015 |
| 分段耗时 | `run_finished.fetch_ms` / `push_ms` / `commit_ms` / `cursor_ms` | ADR-0007 |
| 失败列与值 | `run_finished.column` / `value` | 第 4 节 / #35 |
| `POST /runs` 后的诊断锚点 | `run_opened.run_id` | 第 1.1 节 |
| 终态取证行 | `run_finished`：终态、门禁四数、诊断数、`purged_rows`、分段耗时一次打齐 | ADR-0010 / ADR-0013 |

commit 断连后的唯一一次 `GET` 另落 `commit_diagnosed`：`SWAPPED` 明说目标表已是新数据，
`DISCARDED` 明说目标表未动；其余结果原样说「无法确定目标表是否已被切换」。开连接仪式失败落
`sink_unavailable`，`run_id = null`；人话必须逐项含变量名、期望值、实际值。

#### 6.4 错误码人话与成文归属

| 诊断 | 定稿人话要点 | 归属 |
|---|---|---|
| `ORA-01555` | 「源端结果集在读取过程中失效……请缩小业务日期范围或联系 DBA 调大 undo 保留」 | source（唯一的运行时数据库措辞例外） |
| `ERROR 1118` | 「**目标表建表失败**：列宽合计超出 MySQL 单行上限，需缩窄字符列或拆表」；不得说成暂存表失败 | 生成目标表建表 SQL 的 sink 侧能力；M1 暂存表路径不可能触发 |
| `ERROR 1264` | 数据问题；点名列和值，说明值超出目标数值范围 | sink 成文，source 只加「目标端：」 |
| `ERROR 1292` | 数据问题；点名列和值，说明不是目标日期列可接受的日期时间 | 同上 |
| `ERROR 1366` | 点名列和值，说明该值无法按目标列字符集写入 | 同上 |
| `Note 1265` | `INTERNAL_PRECHECK_ESCAPE`；明写「P0 程序缺陷，不是运行故障，请报 issue」 | 同上 |

sink 通过 MySQL 报出的列名与行号回指原批次，取出**恰好一个失败值**放入 `column` / `value`，
不 dump 整行。`ERROR 1153` 只让人修 `max_allowed_packet` 环境配置，明确「不要排查业务数据」；
`PAYLOAD_TOO_LARGE` 与 `INTERNAL_PRECHECK_ESCAPE` 都是缺陷口吻。开连接仪式的字符集、`sql_mode`、
`max_allowed_packet` 三类失败都是环境口吻。SQL 形状预检仍是唯一由 source 本地成文的预检错误；
其余 MySQL 人话一律 sink 成文，source 只加前缀透传，不二次改写。

### 7. 四条冲突不在本 ADR 解，留给 M2

[#36](https://github.com/liumingjian/db-qbs/issues/36) 的线框撞出四条与已定决策直接冲突的观察需求，
**M1 一条都不解**：

| 冲突 | 撞上 | M1 的处置 |
|---|---|---|
| C1 历史运行列表、按任务/日期查 | ADR-0012 §2 状态不落盘；墓碑 32 条且非状态存储 | **照实认零**，M1 不引入任何运行历史存储。本 ADR 的第 1、2、5 节把「采集日志重建历史」变成一条**可依赖的可选路线**——M2 在「引入历史存储」与「消费日志」之间选一次 |
| C2 进行中的实时观察 | ADR-0016 §3 source 一次性、无入站接口 | M1 只有 stdout 尾随 |
| C3 校验失败的取证（差了哪几行） | ADR-0013 暂存表失败即 `DROP`、不留现场 | M1 不留；日志是唯一可能留下线索的地方，而它只留得下第 6 节那些数 |
| C4 进度百分比 | ADR-0007 不预先 `COUNT` | 结构性不能给：总行数未知，且批次不恒为 5000 行 |

## 后果

- **「记什么、叫什么、写到哪里」已经收口。** 十来条散在各 ADR 里的措辞要求第一次有了统一的
  承载形态：它们都是闭集内某个 `event` 的 `message` 加定稿字段，而不是各自一种排版。
- **M1 的日志成了一个对外接口。** 第 5 节的契约是 M1 第一次对进程外的消费者做出稳定性承诺
  （此前只有 HTTP 协议）。代价：改日志字段从此不是纯内部改动。
- **日志文件的敏感级别被抬到与凭据同档**（第 4 节），部署口径随之变化。
  这是 M1 里第二个「明知的缺口靠文件权限兜底」的地方，与 ADR-0016 §8 一起，
  在引入鉴权时一并重开。
- **ADR-0016 §9 的「无解析抓取」被换成「一次 `jq`」**，验收脚本多一个依赖。
- **成功路径也开始有日志要求**（第 6 节的 run 结束行、ADR-0012 §7 的两句人话），
  这块雾不再只覆盖失败路径。

## 时效

- **第 7 节 C1 在 M2 起图时第一件重开**：选历史存储还是选消费日志，选后者则第 5 节的契约
  从「承诺」变成「被依赖的接口」，演进规则要相应收紧。
- **第 1 节的载体在 source 变长驻（M2 引入 Web UI）时复审**：届时进度有了推/拉通道，
  stdout 不再是唯一观察面，JSON Lines 是否仍是主载体要重新判。
- **第 4 节的「值可打」绑在「日志文件权限 0600 且不外采」上**。一旦日志要采集到目标端之外
  或接入集中式日志系统，这条必须先重开——那时它就不再是文件权限能兜住的事。
- **第 6 节闭集若要新增事件或字段**，须同时更新实现、契约测试与 M2 消费者；已有字段仍只增不删不改义。
