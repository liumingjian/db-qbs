# ADR-0029: 失败分类是一个闭集字段，在出错那一步定下；目标端写批次的三个成因各占一个码

**状态**: 已接受
**日期**: 2026-08-16
**决策票**: [#91](https://github.com/liumingjian/db-qbs/issues/91)
**关联**: [ADR-0010](0010-http-protocol-contract.md)（错误码闭集，本 ADR 增三码 12 → 15）、
[ADR-0017](0017-run-log-format-and-contract.md)（§5 字段只增不删，本 ADR 增 `failure_kind`）、
[ADR-0020](0020-m2-run-history-store.md)（历史行由日志折出，本 ADR 增一列）、
[ADR-0003](0003-numeric-as-string.md)（规范形式拒绝源值 → `SOURCE_VALUE`）、
[ADR-0009](0009-m1-mapping-precheck-rules.md)（映射预检 → `MAPPING_PRECHECK`）、
[ADR-0028](0028-m2-acceptance-criteria-and-rig-extension.md)（§1 断言只打在 `/api` 面，不打 DOM）

## 背景

`docs/STRATEGY-V1.md` 的成功标准第 4 条：**「任务失败时，错误信息能直接指向原因
（哪一列、哪个类型、哪个批次），不需要翻日志猜」**。用户已拍板：这一条达成之后才继续 M3。

这一条有两半，此前只做了一半：

- **措辞那一半到位**。ADR-0017 §4 要求错误行带 `column` / `value`，sink 的人话点名到列、值、
  第几批（#35），源端 `ORA-01555` 翻成人话。
- **分类那一半缺席**。`SinkErrorKind` 只有 `Transport` / `Response` 两档，
  而 STRATEGY-V1 M4 要求区分的是六类：Oracle 连接失败 / dblink 不可用 / 类型映射错 /
  网络中断 / MySQL 写入失败 / 校验不通过。这六类全部塌进 `Response`，
  唯一的分辨手段是读 `sink_code` 加读人话——**读人话就是「翻日志猜」**。

更硬的一处在协议本身。`crates/sink/src/service.rs` 的 `write_batch_api_error` 把三个成因
塞进了两个别处的码：

| 成因 | 原来的码 | 那个码的本义 |
|---|---|---|
| MySQL 逐值拒绝业务数据（1264 / 1292 / 1366） | `BAD_REQUEST` | 请求不合法（我们发错了） |
| `max_allowed_packet` 一类环境配置错 | `SWAP_FAILED` | 切换失败 |
| 写暂存表失败、整批回滚 | `SWAP_FAILED` | 切换失败 |

**从码上分不清是切换坏了还是写批次坏了、是我们发错了请求还是数据本身被拒。**
分类字段若建在这样的码上，第一步就分错。

## 决策

### 1. sink 码闭集增三个码，把写批次的三个成因拆开

`DATA_REJECTED`、`SINK_ENVIRONMENT`、`BATCH_WRITE_FAILED`，闭集 12 → 15（写回 ADR-0010）。

**HTTP 状态码与报文形状一字不改**：`DATA_REJECTED` 仍是 400，另两个仍是 500。
状态码本可以顺手改得更贴切（数据被拒更像 422），但那会动 M1 已定的时序与断言面，
而本 ADR 要解的是**分类可判定**，不是状态码好看。付的代价写在明面上：
`DATA_REJECTED` 的 400 与「请求不合法」共用一个状态，**分辨靠码不靠状态**。

### 2. 分类字段 `failure_kind` 是闭集，且在出错的那一步定下

不做事后推导：不匹配人话文字，也不靠「`sink_code` 为空就是网络问题」这类反推。
`SourceReadError` 与 `TransferFailure` 各带一个 `kind` 字段，**每个构造点必须显式给出**，
没有默认档。

闭集 16 值：

| kind | 触发 |
|---|---|
| `CONFIG` | CLI 参数、`source.toml`、任务定义、业务日期不合法 |
| `ORCHESTRATOR` | 父进程没能把这次运行拉起来（物化任务文件失败、子进程 spawn 失败） |
| `SHAPE_PRECHECK` | 源端 SQL 形状预检未通过（ADR-0016 §4） |
| `SOURCE_CONNECT` | 建本地会话失败：Instant Client 初始化、监听器、登录 |
| `SOURCE_DBLINK` | 会话已建立后撞上远端库 |
| `SOURCE_QUERY` | 其余 Oracle 读取失败，含 `ORA-01555` |
| `SOURCE_VALUE` | 源值过不了规范形式（ADR-0003），带列名与值 |
| `MAPPING_PRECHECK` | `PRECHECK_FAILED` |
| `NETWORK` | `SinkErrorKind::Transport` |
| `SINK_WRITE` | `STAGING_CREATE_FAILED` / `BATCH_WRITE_FAILED` / `SWAP_FAILED` |
| `DATA_REJECTED` | `DATA_REJECTED` |
| `SINK_ENVIRONMENT` | `SINK_ENVIRONMENT` |
| `TARGET_BUSY` | `SWAP_TARGET_BUSY`（ADR-0022：另一个 run 占着目标表） |
| `VERIFY_FAILED` | `VERIFY_FAILED`，以及本地对 commit 响应的行数断言 |
| `DEFECT` | `INTERNAL_*` / `SEQ_MISMATCH` / `RUN_SEALED` / `RUN_UNKNOWN` / `PAYLOAD_TOO_LARGE` / `BAD_REQUEST` / 协议断言，**以及闭集外的任何码** |
| `UNKNOWN` | 进程消失、服务重启、commit 断连后仍判不出 |

**闭集外的码落 `DEFECT` 而不是新造一档**：ADR-0010 的码闭集只增不删，
出现闭集外的码本身就说明有一端偷偷改了协议，那是缺陷。

#### 2.1 同一个 Oracle 码在两步上不是同一件事

`ORA-12541`（监听器没起）、`ORA-12154`（TNS 解析不出）这些码，在**建本地会话**那一步指的是
本地库；会话已经建起来之后再撞上，撞的只能是 **dblink 那一头**。
因此判据是**在哪一步撞上的**，不是码本身：建连接 → `SOURCE_CONNECT`；
取数/prepare 且码在远端链路码表里 → `SOURCE_DBLINK`；其余 → `SOURCE_QUERY`。

远端链路码表：`2019` / `2020` / `2068` / `12154` / `12170` / `12203` / `12514` / `12541`。
`ORA-01555` **不在表里**——快照过旧是本地游标寿命问题，混进 dblink 会把排障引去查网络。

### 3. 落在三个面上：日志、历史、界面

- **日志**：`run_finished` 与四条早失败事件（`cli_failed` / `business_date_invalid` /
  `source_config_failed` / `task_config_failed` / `sql_shape_precheck_failed`）各增一个
  `failure_kind` 字段。成功的 `run_finished` 显式为 `null`——与 ADR-0017 §2 用显式 `null`
  表达「这个东西还不存在」同一种手法。既有字段一字未动（ADR-0017 §5 只增不删）。
- **历史**：SQLite `run_history` 增一列可空 TEXT，走既有 `ALTER TABLE ADD COLUMN` 补列路子。
  **老历史行读出 `NULL`**，消费者不得报错——「当时没记」不是「没有分类」。
- **界面**：失败结论条前面加一个方括号类目名（`[MySQL 写入] 目标端：第 3 批写入暂存表失败……`）。
  **不新增视觉元素、不动 `docs/design-system/`**——那会触发整份 V1–V25 走查，
  而这次要解的是分类，不是视觉。类目词表在 web 侧成文；闭集外的值原样显示，不吞掉。

### 4. 验收断言打在 `/api` 面

M2 台架的 A4/A5/A6/A7/A8/A9/A12/A13 各加一条 `failure_kind` 断言，
覆盖「源端查询 / 成功无分类 / 形状预检 / 映射预检 / 校验 / 缺陷 / 结局未知」。
**不许把 DOM 断言写进验收套件**（ADR-0028 §1），界面那一格靠人工走查看。

## 后果

- **六类可单独判出**，排障第一步从「读完整句人话」变成「读一个字段」。
- **`BAD_REQUEST` 与 `SWAP_FAILED` 的语义被收回本义**。代价：任何按旧码匹配写批次失败的
  外部消费者会失配——本仓库内没有这样的消费者，台架断言也没有依赖这两个码。
- **日志与历史各多一个字段/一列**，`run_finished` 的必带字段集合随之扩大（ADR-0017 §6.2）。
- **界面上的分类是文字，不是元素**。它因此不占设计系统的账，也不会在改视觉时被顺手动掉；
  反过来说，它也**没有视觉权重**——如果将来发现用户看不见它，那是另一次要走整份走查的改动。

## 时效

- **增新 sink 码时必须同时写回 ADR-0010 的表与本 ADR §2 的对照表**；漏一处，
  新码就会落进 `DEFECT`，把一次真实故障说成程序缺陷。
- **`SOURCE_DBLINK` 的码表是经验表**，客户环境跑起来后若出现判错（把本地库的问题说成 dblink，
  或反过来），按实测回炉——判据仍是「在哪一步撞上的」，改的只是码表。
- **M4 做「错误分类可诊断」那条时不再重开分类形态**，只补重试与延迟重推（ADR-0018）。
