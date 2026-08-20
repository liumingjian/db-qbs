# 本地 Oracle 11.2 替身台架

服务于 [#9](https://github.com/liumingjian/db-qbs/issues/9)，供 [#3](https://github.com/liumingjian/db-qbs/issues/3)（类型保真度）与
[#6](https://github.com/liumingjian/db-qbs/issues/6)（dblink 列投影）**开箱即用**。
客户环境短期接触不到（[#2](https://github.com/liumingjian/db-qbs/issues/2) 卡住），这套台架让 ADR-0001 的回退决策不必干等客户放行。

**台架是一次性的，不进主干构建**；本 README 与脚本进主干。

## 起停

```bash
cd docs/spikes/fixtures/local-rig
./scripts/up.sh        # 起库 + 建等价表 + 灌边界值 + 建 dblink + 冒烟
./scripts/smoke.sh     # 只跑冒烟
./scripts/run-dblink-probe.sh [脚本名]   # 跑 #6 的 dblink 探针（默认 dblink-pushdown.sql）
./scripts/run-pagination-boundary-probe.sh # 跑 #21 的分页边界可复现性探针
./scripts/run-canon-gate.sh             # 跑 #43 的 M1 规范形式手工门禁
./scripts/run-m1-acceptance.sh           # 跑 #45 的九类 M1 验收并生成报告
./scripts/run-m2-acceptance.sh           # 跑 #72 的 A1–A14 M2 验收并生成报告
./scripts/run-m3-acceptance.sh           # 跑 #115 的 B1–B6 M3 验收并生成报告
./scripts/run-v1-acceptance.sh           # 跑 #135 的 C1–C6 第一版验收并生成报告
./scripts/run-bulk-probe.sh              # 跑 #5 的内存形状探针（19 组配置矩阵）
REPS=7 ./scripts/run-cpu-probe.sh        # 跑 #5 的客户端每行 CPU 探针（每档取中位数）
./scripts/run-mysql-roundtrip-probe.sh   # 跑 #13 的目标端往返实测（只起 MySQL，不用 Oracle）
./scripts/run-number-shapes-probe.sh     # 跑 #104 的 NUMBER 纯小数/负标度端到端往返实测
./scripts/run-bc-date-probe.sh           # 跑 #98 的公元前日期驱动年份符号取证（只用 Oracle）
./scripts/rehearsal-up.sh                # 起演练台的两台 centos:7「主机」容器（#152）
./scripts/rehearsal-reset.sh             # 把两台主机推倒重建回干净机器态
./scripts/rehearsal-topology-check.sh    # 跑演练台的 R0–R10 拓扑判据
./scripts/test-rehearsal-topology.sh     # 上面三支脚本的静态自检（不碰 docker）（--reset 才判 R9，会推倒重建）
./scripts/sqlplus.sh   # 进 sqlplus
./scripts/down.sh      # 拆掉，连卷一起删
```

首次 `up.sh` 要拉两个镜像、下 84 MB Instant Client、建库，模拟层下需要几分钟。
`up.sh` 结束时冒烟必须全过——冒烟挂了，这台架就不能用来支撑 #3 / #6 的任何结论。

## 平台分配（这套台架成立的关键）

| 层 | 平台 | 说明 |
|---|---|---|
| Oracle XE 11.2.0.2 | `linux/amd64`（Rosetta 模拟） | `gvenzl/oracle-xe:11.2.0.2-slim-faststart` |
| MySQL 8.0 | `linux/arm64` 原生 | 官方镜像有 arm64 |
| spike 程序 + Instant Client 19.32 | `linux/arm64` 原生 | 全速 |

**只模拟数据库，不模拟客户端**：Rust 编译与 ODPI-C 调用都跑在原生架构上，模拟层只压在服务端。

## 为什么是 11.2.0.2 而不是 11.2.0.4

公开渠道拿不到 11.2.0.4 镜像 —— XE 最高只到 11.2.0.2，11.2.0.4 要 EE/SE 介质自己 build（Oracle 账号 + 许可）。
ARM 版 Oracle 11g **不存在，也不会有**。

这不影响结论的性质：19.32 能连 11.2.0.2 是「技术上能连」，Oracle 的认证下限 11.2.0.4 是「出问题他管不管」。
**生产库小版本仍必须由 #2 的 `01-env-facts.sql` 确认。**

## 台架内容

```
oracle/01-grants.sql        spike schema 的权限（含 CREATE DATABASE LINK、看执行计划）
oracle/02-schema.sql        等价表 —— ADR-0003 每一种规范形式至少一列 + #43 门禁表
oracle/03-boundary-rows.sql 边界值 + M0 历史期望 + #43 M1 样本 + 10 万行
oracle/04-dblink.sql        指回自身的 loopback dblink，名字沿用生产的 @FA
mysql/10-target-schema.sql  目标端等价表，utf8mb4
probes/dblink-pushdown*.sql #6 的 dblink 列投影探针（**不是** initdb 脚本，起库后按需跑）
probes/pagination-boundary.sql #21 的本地/dblink × 非全序/全序分页边界探针
probes/mysql-roundtrip.sql  #13 的目标端往返实测（CHAR 尾空格 / DECIMAL 标度 / DATETIME / NULL）
```

`probes/` 里的脚本不参与建库，通过对应的 `scripts/run-*-probe.sh` 手动跑；
它们按需自建/自删所需的表与 dblink，可重复执行。

## #45 的 M1 验收编排

在 ADR-0005 指定的 arm64 mac 上通过 `/rexec` 执行一条命令：

```bash
./scripts/run-m1-acceptance.sh
```

入口会确保台架就绪，构建 source/sink，重建仅供验收使用的 `t_m1_*` / `M1_*` 表，然后依次运行
10 万行宽表、10 万行三列窄表、kill source 后重跑、kill sink 后重跑、commit 响应中断的
`SWAPPED` 与 `DISCARDED` 两种终态、空结果集、两类 `VERIFY_FAILED` 与规范形式手工门禁。破坏性场景只触碰这些 `M1_*` 表；场景间会停止 sink、
清掉孤儿暂存表并重启内存状态，因此同一入口可重复执行。`--list` 只列场景，不启动台架。

每个场景在终端输出 `PASS` / `FAIL`。最终报告默认写到本目录的
`m1-acceptance-<UTC>.md`（可用 `M1_REPORT=/path/report.md` 指定），包含 fetch/push/cursor、
commit 及其 `SELECT COUNT(*)` 子项、`purged_rows`、逐批实际行数和序列化字节分布，外加一节
**载荷记账**（源行宽 bytes/row、批数、批体 p50、载荷总量估算）——客户需求「单次 10 万行、
约 100MB」的兑现点就在那一行上（ADR-0040 §1），**只记不判**。报告按
push/cursor 超过 50%、commit 对 30 分钟读超时、最大批次对 16 MiB 三条口径复审；批次行数
始终从 JSONL 求和和排序，不假设固定值。

commit 场景由 `acceptance/commit-drop-proxy.py` 在 sink 完成事务后切断第一次 commit 响应，
随后转发 source 唯一一次诊断 GET，以稳定复现 `COMMITTING` 的不确定窗口。代理用
`M1_COMMIT_DROP_MODE` 选择断连后留下的终态：`swapped`（默认）原样转发 commit；`discarded`
在转发前把 `total_rows` 加一，令 sink 的暂存/源端行数校验失败，从而丢弃暂存表并落
`DISCARDED` 墓碑——两种模式下 source 都只能靠墓碑 GET 判断目标表是否已被切换。代理只用于本台架。

kill sink 场景在 source 推完部分批次后 `kill -KILL` sink，验证 source 以 1 退出、终止在
`STREAMING` 且不产生 `commit_diagnosed`，目标表保持原样；重启 sink 后重跑必须换用新 `run_id`，
且**哨兵留存**——写入模型换成按主键 upsert 之后（ADR-0035 §1），重跑碰不到主键不在源结果集里
的那一行，目标表哈希回到的是「基线 + 哨兵」而不是基线本身（ADR-0040 §5.1）。同一条理由下，
空结果集那一场的 `purged_rows` 恒 `0`、目标端当日行原封不动。
入口生成的临时日志权限受 `umask 077` 约束，报告权限为 0600。

`oracle/` 与 `mysql/` 分别挂进两个镜像的 initdb 目录，**只在首次建库时执行一次**。
改了 SQL 要重新生效，必须 `./scripts/down.sh && ./scripts/up.sh`（卷不删就不会重跑）。

### M0 历史期望值

`t_canon_expected(row_id, column_name, expected, note)` 存每个单元格按 ADR-0003 应有的规范形式。
**#3 的断言 join 这张表，不要把期望值硬编码进 Rust** —— 期望值是 ADR 的产物，改 ADR 应该只改这张表。

这段只描述 M0 的 `spike-odpi`。ADR-0014 已把 `t_canon_expected` 降级为历史产物；
#43 的新门禁不读它，权威改为仓库内 `../canon-golden.json`。

覆盖到的边界：38 位满精度（正/负）、无精度声明的 `NUMBER`、高标度、尾零与负零、
`DATE` 非零时分秒、`TIMESTAMP` 固定 6 位、中文 `VARCHAR2`/`NVARCHAR2`、`CHAR`/`NCHAR` 尾空格、
全 `NULL` 行。

`RAW`/`CLOB`/`NCLOB`/`BLOB`/`BINARY_FLOAT`/`BINARY_DOUBLE`，以及各自单表的 `LONG` 与 `LONG RAW`
（Oracle 限一表一个），**在 ADR-0003 白名单之外——V1 明确不支持**（[#11](https://github.com/liumingjian/db-qbs/issues/11) 已结，
映射预检遇到即报错拒绝）。它们的 `note` 以 `V1 排除` 开头，探针据此判 **EXCL** 而非 PASS/FAIL：
不做断言，只回报驱动取到了什么，好在 #2 的真实类型清单命中时知道要回炉补什么。
判据在数据里——若日后决定纳入某一类，改 `t_canon_expected` 的 `note` 与 `expected` 即可，不必动 Rust。

## #43 的规范形式手工门禁（`spike-canon/`）

`scripts/run-canon-gate.sh` 连接台架 Oracle，从 `t_canon_m1_probe` 读取原生
`NUMBER` / `DATE` / `VARCHAR2` / NULL，再调用共享库的 `canon_*`，逐条与仓库内
`canon-golden.json` 比较。程序**不查询 `t_canon_expected`**。

当前 fixture 的 36 条 M1 用例全部逐条输出。Oracle 表只放其中 21 条能由原生类型表示的
accept/bypass 样本；15 条 reject 见证直接从 fixture 交给共享校验函数。不能把 `1,23`、
`1E5`、`.5` 或非法年份塞进 `NUMBER` / `DATE` 列冒充源端值：前两种出现表示驱动或
NLS 漂移，`.5` 出现表示链路绕道 `TO_CHAR`。NULL 组只有 `null-bypass` 一条结构性断言，
确认它不进 `canon_*` 并保持 JSON `null`；台架不构造 Oracle 空串这个假用例，因为 Oracle
会把空串存成 NULL，M1 源端不存在空串 `VARCHAR2` 值。

退出码供后续 M1 验收脚本编排：`0` 为总 PASS，`1` 为至少一条断言 FAIL，`2` 为连接、
fixture 或台架结构等程序运行错误。输出包含每条 `[PASS]` / `[FAIL]` 和末尾
`TOTAL PASS: PASS n / FAIL 0`（或 `TOTAL FAIL`）。

这是手工门禁，三条触发条件固定如下：

1. M1 验收时必跑一次。
2. 任何改动 `canon_*`、Oracle 驱动版本、或 NLS / 字符集相关配置的变更，合并前必跑一次。
3. 每次跑完把实际逐条输出贴进 `docs/spikes/0001-oracle-driver.md`，不能只记「通过」。

本门禁**不进 CI**：它依赖 mac 上 amd64 模拟的 Oracle XE 与 arm64 原生 Instant Client，
结构上不适合 GitHub runner；硬塞进 CI 只会得到长期红灯或长期跳过。纯函数层仍由根工作区
CI 的 `cargo test -p db-qbs-shared` 负责。

不要把本程序并进 `spike-odpi`。`spike-odpi` 回答 M0 的「驱动取到了什么」，并继续用
`t_canon_expected` 得出历史 `FAIL 0`；`spike-canon` 回答 M1 的「驱动出口是否符合当前仓库
fixture」。两种失败含义不同，混合会破坏 M0 结论的语义。

## 边界 —— 本台架不能答什么

1. **`NLS_CHARACTERSET` 是 `AL32UTF8`，改不了。** XE 建库时定死，`ALTER DATABASE CHARACTER SET`
   只允许向超集转，AL32UTF8 → ZHS16GBK 是收窄，不放行。
   **#3 的 GBK 中文往返测不了**，只能测 UTF-8 路径。绕开办法是拿 EE 介质自己 build 一个
   ZHS16GBK 的 11.2.0.4 镜像，要 Oracle 账号和许可，**先不做**，记为 M0 的已知缺口。
2. **#5 的服务端吞吐测不了，客户端侧的两条都能测 —— 且都已测完。**
   服务端跑在模拟层上，**墙钟的绝对数字是废数据**。但内存形状与客户端每行 CPU 都是驱动
   **客户端侧**的行为，客户端是 arm64 原生、没有模拟层，所以这两条成立：
   - 内存形状：`t_bulk_probe` 的 10 万行量出「峰值随批次走、与总行数无关」，
     跑法 `./scripts/run-bulk-probe.sh`。
   - 客户端每行 CPU：`getrusage` 的 `ru_utime + ru_stime` **不计等服务端的时间**，
     跑法 `./scripts/run-cpu-probe.sh`。同一条路径走 loopback dblink 墙钟涨 2.6 倍而
     客户端 CPU 纹丝不动 —— 这条对照就是「计量隔离掉了模拟层」的证据。

   结论都在 `docs/spikes/0001-oracle-driver.md` 第 4 节。
   **仍不能答的是服务端吞吐的绝对秒数与真实行宽**，那两条按 ADR-0005 留给上线前复验（#2）。
3. **替代不了 #2 的真实列清单** —— 那是客户表的属性。等价表覆盖的是「类型面」，不是「列清单」。

因此本台架**不能关闭 M0 闸门**。它把 #3 从「完全没跑过」推进到「机制已验证，只差真实类型清单和 GBK 字符集」，
把 #5 从「完全没跑过」推进到「内存前提已证实，只差吞吐绝对值与真实行宽」；#2 仍整个卡在客户环境上。

## 已知坑

- `unzip` 必须带 `-o`：两个 Instant Client zip 都含 `META-INF/MANIFEST.MF`，
  不加会在无 tty 环境下卡在覆盖确认并以 exit 1 失败。
- `debian:12-slim` 拉取偶发 blob 校验失败，重试即可。
- MySQL 检索 `CHAR` 时**默认剥掉尾部空格**，而 ADR-0003 要求 `CHAR` 保留尾空格 ——
  `mysql/10-target-schema.sql` 里的 `t_char_pad_probe` 专门验这条，结论可能是目标端必须用 `VARCHAR`。

## 台架首跑已经抓到的东西

固化过程中真跑了一遍，三条实测结论（`scripts/smoke.sh` 每次起台架都会重跑前两条）：

1. **`TO_CHAR(0.5)` 返回 `.5`，不是 `0.5`** —— 但这只是显示层行为，链路不经过 `TO_CHAR`。
   #3 用 ODPI-C 取同一批值拿到的是 `0.5` / `-0.01`。**[#10](https://github.com/liumingjian/db-qbs/issues/10) 已结**：
   ADR-0003 定成 `|x| < 1` **保留**小数点前的 `0`，且 `NUMBER` 的规范化定位为**校验**（不合规报错，不静默重写）。
   `t_canon_expected` 已与之一致，无需改动。
2. **`BINARY_DOUBLE` 的值域装不进 `NUMBER`。** 字面量不带 `d` 后缀会被当 `NUMBER` 解析并
   `ORA-01426 numeric overflow`。这条连同 #3 实测的「309 位十进制展开」一起送走了
   [#11](https://github.com/liumingjian/db-qbs/issues/11)：**V1 明确不支持二进制浮点**，
   连同 `RAW`/LOB/`LONG` 一并排除，映射预检报错拒绝。
3. **dblink 列投影 Oracle 自己会下推**（#6 已结）。内层 `SELECT *` 与投影写进内层生成的
   远端 SQL 一字不差，`NO_MERGE` 也推不坏；绑定变量能穿过 dblink。详见
   `docs/spikes/0001-oracle-driver.md` 第 5 节。**注意字节计数器的坑**：填充数据同值时
   SQL*Net 会去重重复列值，测出 12 B/行的假数字，必须灌随机值。
4. **Oracle 把空串存成 `NULL`（已实测 `v_ascii` 写 `''` 后 `IS NULL`）。**
   所以「`NULL` 与空串并存」在源端**不可能构造** —— ADR-0003 里 `NULL` 与空串的区分，
   只在目标端 MySQL 侧有意义。`CHAR(10)` 尾空格则确实保留（`LENGTH`=10），
   中文按 UTF-8 存（6 字符 / 18 字节）。

## #5 的内存形状探针（`spike-bulk/`）

`scripts/run-bulk-probe.sh` 跑 19 组配置：地板（只连库）、行数阶梯、全量驻留反证、
批次阶梯、`fetch_array_size` / `prefetch_rows` 阶梯、走 `@fa` 的同一链路。

**一次进程只测一个配置。** `/proc/self/status` 的 `VmHWM` 是进程存续期峰值，
同一进程里连测多个配置，后面的会被前面的峰值污染 —— 矩阵必须由脚本循环拉起进程，
不能在 Rust 里 for 循环。

**全量驻留（`collect` 模式）那三行不是凑数的**：它证明测量手段对「内存随行数涨」
是敏感的，所以流式那三行的「不涨」不是量不出来。任何「某某不增长」的结论都该配一条这样的反证。

## #5 的客户端 CPU 探针（同一个 `spike-bulk/`）

`scripts/run-cpu-probe.sh` 跑四个累进层级 `cpu0`～`cpu3`，相邻两层相减即成本分解：

| 模式 | 每行做什么 | 减去上一层得到 |
|---|---|---|
| `cpu0` | 只迭代行，一个字段都不取 | 驱动的行推进与协议解析 |
| `cpu1` | 取原生类型（i64 / f64 / String / `Timestamp`） | ODPI-C 取值 |
| `cpu2` | 取 ADR-0003 规范形式文本，算完即弃 | 数值与日期 → 文本 |
| `cpu3` | 组 `Vec<String>` + 批次缓冲（= 完整搬运路径） | 我们自己的组装与批次 |

第 7 个参数 `ncols`（1..4）控制取前几列 —— 台架表只有 4 列而生产是 70 列，
必须先知道成本是**按行摊还是按单元格摊**，外推才有依据。

**判据是 CPU 不是墙钟。** `getrusage(RUSAGE_SELF)` 只计进程占用 CPU 的时间，
等服务端那段不计入，所以模拟层影响的是墙钟。同理**这里也不能用墙钟下任何结论**。

## M2 的验收编排

规格见 [ADR-0028](../../adr/0028-m2-acceptance-criteria-and-rig-extension.md)（决策票
[#59](https://github.com/liumingjian/db-qbs/issues/59)）。要点：

- **入口独立**：`scripts/run-m2-acceptance.sh` 与 `run-m1-acceptance.sh` **并列，互不吞并**。
  M2 验收的前置要求是先跑绿 M1 那份。M1 的 9 类场景**不改写成经由 UI 发起**——那会让
  M1 的回归基线依赖 M2 的实现，已 9/9 PASS 的证据链就不再是常量。
- **断言面是 `source` 的 `/api/*`，不是 DOM**。渲染面另立人工走查清单
  [`m2-visual-walkthrough.md`](m2-visual-walkthrough.md)，不给退出码。
- **长驻进程的驱动**：宿主机后台起进程（**不进 compose**——杀进程本身是被测对象），
  轮询 `GET /api/tasks` 判就绪（**不新开健康端点**），`SIGTERM` 优雅收尾、超时兜底 `-KILL`。
- **场景清单** A1–A14 见 ADR-0028 §3.1；纯函数层 F1/F2 进 CI；台架层手工门禁 G1
  （建表 SQL 生成器 ↔ 映射预检不漂移的第二遍）挂 ADR-0014 §8 的触发条件，与
  `run-canon-gate.sh` 共用机制。
- **报告不采性能数**（那是 M1 入口的事），形态是「场景 × PASS/FAIL + 断言实际值」，
  写 `m2-acceptance-<UTC>.md`。唯一新增记录项是**投影六标量 == 历史行落库值**的一致性对照。

先独立跑绿 `scripts/run-m1-acceptance.sh`，再在宿主机设置 Oracle Instant Client 目录并运行：

```bash
M2_ORACLE_CLIENT_LIB_DIR=/path/to/instantclient \
  ./scripts/run-m2-acceptance.sh
```

M2 入口会在宿主机拉起 `db-qbs-source`，并把 A1–A14 的 PASS/FAIL 与每条断言实际值写进报告；
它不会代跑 M1，也不会创建 `/health`。

#### arm64 mac：宿主那半必须是 x86_64 + Rosetta

ADR-0028 要求 source 跑在**宿主机**（杀进程本身是被测对象），而 **Oracle 没有出过 macOS arm64 的
Instant Client**，最后一版是 19.16 x86_64。所以 arm64 mac 上宿主那半只能编成 `x86_64-apple-darwin`
让 Rosetta 跑，否则 `libclntsh.dylib` 根本加载不了。一次性准备：

```bash
rustup target add x86_64-apple-darwin
# 19.8 x64，免登录直链
curl -LO https://download.oracle.com/otn_software/mac/instantclient/198000/instantclient-basic-macos.x64-19.8.0.0.0dbru.zip
unzip instantclient-basic-macos.x64-19.8.0.0.0dbru.zip -d ~/oracle
```

之后每次这样跑（`M2_HOST_CARGO_TARGET` 只影响宿主那半，容器那半仍是 arm64 原生）：

```bash
M2_ORACLE_CLIENT_LIB_DIR="$HOME/oracle/instantclient_19_8" \
M2_HOST_CARGO_TARGET=x86_64-apple-darwin \
  ./scripts/run-m2-acceptance.sh
```

#### 把台架留给渲染走查：`M2_KEEP_RIG=1`

加上 `M2_KEEP_RIG=1`，最后一条场景跑完后台架不拆：容器、sink、宿主 source 与这一轮累积的运行历史
全部留着，入口会打印 web UI 地址与拆台架命令。这样 [`m2-visual-walkthrough.md`](m2-visual-walkthrough.md)
要看的终态**就是 A1–A14 刚造出来的那批**，不必另造一套编排。交接时 source 停在 `hang-streaming`
模式，于是从 UI 发起的 run 会停在 `STREAMING` 不走 —— 走查里 V1 / V16 / V17 这三条要的「进行中」
靠的就是它。

### M2 手工门禁

- **G1**：真在 MySQL 执行生成的 DDL，真 `describe` 目标列，再真跑映射预检；把实际观察贴进本次验收记录。
- **G2**：独立运行既有 `scripts/run-canon-gate.sh`，该入口一字不改。
- **渲染走查**：每次 M2 验收必须跑 [`m2-visual-walkthrough.md`](m2-visual-walkthrough.md)。任何改动
  `docs/design-system/README.md` 或 `docs/design-system/tokens.css` 的变更，合并前也必须跑同一份走查并记录实际观察。

## M3 的验收编排

规格见 [ADR-0032](../../adr/0032-m3-acceptance-criteria-and-rig-extension.md)（决策票
[#103](https://github.com/liumingjian/db-qbs/issues/103)）。M3 是第三个独立入口，前置要求是
先跑绿 M1 与 M2；它不会修改或代跑另外两份验收。

```bash
PATH=/opt/homebrew/opt/node@22/bin:$PATH \
M2_ORACLE_CLIENT_LIB_DIR=/path/to/instantclient \
  ./scripts/run-m3-acceptance.sh
```

arm64 macOS 沿用上面的 `M2_HOST_CARGO_TARGET=x86_64-apple-darwin` 方式编译宿主 source；容器内的
sink 仍使用 arm64 原生构建。Node 22 的 Homebrew 路径按本机实际安装位置调整。

入口依次跑 B1–B6：九行形态逐值往返、全量映射拒绝、`TIMESTAMP(n>6)` 拒绝、裸 `NUMBER` 值域
校核正面、无裸 `NUMBER` 的零扫描反面，以及公元前 `DATE` 的源值失败。B1 的字符族与日期族用
`HEX()` 比对，`DECIMAL` 族按目标列标度补零后的读回值比对。B4 报告唯一的值域校核耗时与扫描行数；
不采 M1 的通用性能数。

M3 使用新建的 `acceptance/oracle-m3.sql` 与 `acceptance/mysql-m3.sql`，两份 fixture 都只触碰
`M3_*` 表；台架仍由 `./scripts/up.sh` 自包含地启动，没有 `up-m3.sh`。source 在宿主机后台运行，
sink 仍运行在 `qbs-client` 容器内。`--list` 只列 B1–B6，不启动台架。

#### 把台架留给 M3 走查：`M3_KEEP_RIG=1`

加上 `M3_KEEP_RIG=1` 后，入口会在 B1–B6 结束时重跑 B2 与 B4，留下两条失败历史，并打印
run ID 与 W1–W6 的对应关系。这样 [`m3-visual-walkthrough.md`](m3-visual-walkthrough.md)
可以直接观察刚生成的 B2/B4 终态；W3/W4/W5 只需使用入口打印的源 SQL 在构建器取列。

报告默认写为 `m3-acceptance-<UTC>.md`（可用 `M3_REPORT=/path/report.md` 指定），内容是
场景 × PASS/FAIL 与每条断言的实际值。报告不会把人工走查写成结果，W1–W6 的实际观察另写为
`m3-visual-walkthrough-<UTC>.md`。

### M3 手工门禁

- **G1**：把生成的 DDL 真正交给 MySQL `describe`，再跑映射预检，覆盖 ADR-0030 §1 的九行形态。
  `NUMBER(38,-30)` 必须在生成侧先拒绝，不能输出 `DECIMAL(68,0)` 后再让 MySQL 报错；把实际列形状、
  预检报告和该拒绝点贴进本次 M3 报告。
- **G2**：独立运行既有 `scripts/run-canon-gate.sh`，入口一字不改。
- **W1–W6**：每次 M3 验收必须跑 [`m3-visual-walkthrough.md`](m3-visual-walkthrough.md)，逐条记录
  1024/1440 布局、五列报告、取列标记、DDL 占位符、白名单外列态和 B4 值域记录的实际观察。

## 第一版的验收编排（第四个入口）

判据见 [ADR-0040](../../adr/0040-v1-acceptance-criteria-and-rig-extension.md)（决策票
[#122](https://github.com/liumingjian/db-qbs/issues/122)，实现票
[#135](https://github.com/liumingjian/db-qbs/issues/135)）。第一版的验收面是**四份**台架
（ADR-0040 §5.4）：M1 / M2 / M3 加上本入口，四份**串行**跑，共用同一套 docker 台架与端口。

```bash
M2_HOST_CARGO_TARGET=x86_64-apple-darwin \
M2_ORACLE_CLIENT_LIB_DIR=/path/to/instantclient \
  ./scripts/run-v1-acceptance.sh
```

入口依次跑 C1–C6：数据源 CRUD 与测试连接、不同名字段映射与目标端列面、用户可填筛选条件、
主键 upsert 的幂等、映射预检三分支、内存形状。**编号字母全局唯一**：M2 是 A、M3 是 B、
第一版是 C，任何情况下不复用、不重编（ADR-0040 §2）。**没有 C7**——「10 万行 / 约 100MB」
的兑现点是 M1 的 `wide-100k` 加 C6，重复设一个只会有两个各自漂移的真源（ADR-0040 §1）。

fixture 是新建的 `acceptance/oracle-v1.sql` 与 `acceptance/mysql-v1.sql`，只触碰 `T_V1_*`
与 `V1_*` 表；**`oracle.sql` / `mysql.sql` 一个字节不动**（M1 基线是常量）。C6 的源表是 M1 的
`t_m1_wide`（ADR-0040 §3.3 字面「同一张宽表」），目标端另起 `V1_WIDE`，免得把 10 万行残留
留给下一份 M1 台架。

### C6 的内存高水位怎么量

判据是 `peak(100k) − baseline ≤ 2 × (peak(10k) − baseline)`，**source 与 sink 各判一次，
两条都绿才算 PASS**。量的是内核维护的单调高水位，**不是轮询采样**——采样会漏掉峰值，
漏掉之后判据假绿，比没有判据更坏：

- **source**（一次性进程）：`acceptance/v1-memory-wrapper.py` 夹在编排进程与真二进制之间，
  用 `wait4()` 取子进程的 `ru_maxrss`。**单位不是跨平台常量**（macOS 字节 / Linux kB），
  wrapper 记原始值、平台与归一到字节的值。
- **sink**（常驻进程）：run 结束后读容器里的 `/proc/<pid>/status` 的 `VmHWM`（kB）。
- **两档之间必须重启 sink**：`VmHWM` 跨 run 只增不减，不重启比值恒等于 1、判据永久假绿。
  报告里记下两档的 sink pid，证明重启这一步确实执行过。
- **基线随档走、一档一测**（ADR-0040 #135 增补 1）：sink 每档是新进程，source 的基线由一趟
  `ROW_ID < 1` 的 0 行真 run 给出——连库、建暂存表、走完切换，唯独没搬第一行。
- 四个绝对数与四个基线原样进报告；**不设绝对上限**，**耗时只记不判**。

### 跑完默认不清场

所有者 2026-08-19 裁定：跑完把台架留着，C1/C2 建出来的两条数据源与那个不同名映射的任务
正是 X1–X8 走查过半条目要用的数据。要清场传 `--clean`（反选，不是默认）。
`--list` 只列 C1–C6，不启动台架；`./scripts/test-v1-acceptance.sh` 是不起台架的静态自检。

### 报告

报告落 `v1-acceptance-<UTC>.md`，**开头是「客户五条需求 → 在哪儿验 → 本次结果」的五行对照表**
（所有者裁定），随后是逐场景结果、C6 的六个内存数、逐条断言证据，以及两节交代边界的正文：
台架能证到哪儿（C1② / C2② / C3③ 有一半在界面上，归 X 走查），以及三份视觉走查在本入口的
触发情况。**不许写「通过」**——贴的是实际观察，没跑的写明「未跑及为什么」。

## 装机演练台（#152 / ADR-0041 增补 1）

第二版要在客户现场装机（ADR-0041），演练台就是那两台客户主机在 mac Docker 上的替身：
**两个 `centos:7` 容器与既有的 Oracle / MySQL 同处一套 compose**，不新开第五个台架字母入口
——ADR-0040 §2 的字母是按**搬运语义的入口**分的，第二版零搬运语义改动。

```
  qbs-host-source ── qbs-src-side ── qbs-oracle11        （源端主机：source + Instant Client）
        │
        └── 宿主 127.0.0.1:15443（扮演客户侧白名单端口）──▶ qbs-host-target:15443
                                                    │
                          qbs-host-target ── qbs-dst-side ── qbs-mysql8   （目标端主机：sink）
```

| 面 | 演练台怎么扮演 |
|---|---|
| 两库之间网络不通 | 两台主机各在一张自己的网上（`qbs-src-side` / `qbs-dst-side`），**切断由台架显式施加**：对面那张网整段黑洞 + 对面那个库的端口出向 DROP（两层，见下） |
| 公网那一跳 | 源端经**宿主上暴露出来的端口**到目标端——容器直连摸不到，只有暴露口能过 |
| 白名单端口 | 目标端只暴露 `15443` 一个口（给 #153 的 stunnel 服务端）；没暴露的端口就是白名单外的端口 |
| 干净机器 | 两台主机**不挂卷、不 build 自定义镜像**，删容器即归零；`rehearsal-reset.sh` 一条命令回到起点 |

- **默认不起，这是对 #152 判据 1 的一次显式收窄**：票面写的是「两个主机容器随既有编排起停」，
  实现只让**停**那一半随既有编排走（`down.sh` 带 `--profile rehearsal`，连它们一起拆），
  **起**那一半没有并进 `up.sh`——两台主机在 `rehearsal` profile 下，要它们跑
  `./scripts/rehearsal-up.sh`（前提是两个库已经起着）。
  取舍的理由是同票判据 3：并进 `up.sh` 就等于给四份既有台架的起停加了两个 centos:7 容器与
  两张网，「既有台架不受影响」当场不成立。**两条判据冲突时按判据 3 让**，收窄记在这里，
  不靠读脚本才看得出来。
- **首次应用这套改动，两个库容器可能被重建**：`oracle` / `mysql` 各多挂了一张「侧」网，
  而 networks 列表进 compose 的配置哈希。2026-08-19 在 Docker 29.3.1 上实测是**原地挂网、
  没有重建**（库里 M1/M2/M3/v1 的表原样还在），但那是一次观察，不是对下一台机器的保证——
  真重建了也不是事故：两个库**本来就没有数据卷**，`up.sh` 会照 initdb 脚本重新灌一遍
  （Oracle 在模拟层下要等几分钟）。真正会丢的是**上一轮验收留在库里、给视觉走查用的那批数据**
  （见「跑完默认不清场」），要用就先跑完走查再动这套改动。
- **刻意不挂仓库**：挂上 `/workspace`，手册就能靠容器里现成的东西蒙混过关。东西必须像现场那样
  `docker cp` 搬进去。
- **刻意用 `linux/amd64`**：客户机是 x86_64 CentOS 7，glibc 2.17 的下界也是在那个架构上兑现的
  （#151）。演练台跟着客户机走，免得演练跑的是一套二进制、带去现场的是另一套。Rosetta 下慢，
  但演练量的是装机步骤，不是吞吐——与本 README「边界」那条一脉相承。
  要装进这两台主机的二进制由 `packaging/centos7/build.sh` 出（#151），产物架构与它们对得上。
- **切断是台架显式施加的，不是 Docker 白送的，而且要两层**：Docker Desktop 在两张 bridge 网之间
  **是转发的**，两台主机各在自己那张网上并不构成隔离（2026-08-20 实测 `172.30.0.3` 直连
  `172.29.0.3:15443` 拿得到令牌）。`rehearsal-up.sh` 起完会借一个一次性 alpine 共享两台主机的
  网络命名空间施加两层：
  1. **路由黑洞**——对面那张网、加上 default 网的网段（两个库在 default 上各还有一个 IP，
     只挡侧网等于没挡）。挡的是「对面那台机器与那个库的所有端口」。
  2. **端口级 DROP**（IPv4 与 IPv6 两张表）——源端封死一切 `3306` 出向，目标端封死一切 `1521` 出向。
     挡的是**绕过路由的那条路**：`host.docker.internal` 在 Desktop 上是个 IPv6 网关地址，宿主上
     `1521:1521` / `3306:3306` 两个发布端口就挂在它后面。**2026-08-20 实测源端经它连 MySQL 3306
     是通的**——只有第 1 层时，「两库之间网络不通」在演练台上从来没成立过（ADR-0041 增补 5）。
     路由黑洞管不了它：那是另一个地址，且必须按端口区分——同一个网关上的 `15443` 正是白名单那一跳。
  
  **被演练的机器本身一个字节没动**（里面连 `ip` 都没有），切断像客户现场的防火墙那样是外部事实。
  删容器即归零，起的时候重打。裁定见 ADR-0041 增补 4 与增补 5。
- **每次 `up` 都要联网**：本机 `centos:7` 这个 tag 指的是 arm64 镜像，而两台主机声明
  `platform: linux/amd64`，compose 每次都得联网解析 amd64 的 manifest——**离线起不来**，
  撞上 registry 超时就复跑一次。
- **拓扑判据 R0–R10** 由 `./scripts/rehearsal-topology-check.sh` 逐条断言并打印实测。
  **通的要通，不通的更要不通**：R3/R5（源端摸不到 MySQL、目标端摸不到 Oracle）、R6（跨容器直达）、
  R8（白名单外的端口）四条负判据一旦悄悄失效，演练就会在一张比客户现场宽松的网上跑完，
  手册里缺的那几步要到现场才炸。**每条负判据都配一条正对照**（R7a/R8a：目标端本机自连，
  确认监听端真的活着）——没有正对照的「不通」不算证据，容器没起、没人监听、DNS 查不到，
  得出的都是「不通」。**负判据一律按 IP 判、且正对照要同址**：R3 的正对照就是 R4（同一个 MySQL IP、
  同一个端口，从目标端连必须通），R5 的正对照是 R2。按容器名判出来的「不通」是假绿——
  `fa2c708` 那一版三条负判据全栽在这上面，实录见
  [`rehearsal-topology-20260820T012000Z.md`](rehearsal-topology-20260820T012000Z.md)；
  宿主网关那条路又漏了一轮，实录见
  [`rehearsal-topology-20260820T014500Z.md`](rehearsal-topology-20260820T014500Z.md)。脚本**默认不重建**（演练进行到一半来复核拓扑是常态，不该顺手把已装好的
  source / stunnel 抹掉），此时 R9 不判；要判干净态就跑 `--reset`，它会先推倒重建、
  再在刚重建出来的干净两台主机上判其余各条。收尾一律回收探针进程并判 R10
  （`15443` 交还给 #153 的 stunnel）。
  **R6 按 IP 判、不按容器名**：按名字连时失败首先发生在名字解析，那正是上面点名的假绿成因之一；
  R6 取目标端在 `qbs-dst-side` 上的 IP 直连，并配 R6a（目标端自己经同一个 IP 连得到令牌）
  把「IP 取错」也排掉，R6b 才是按容器名的那条。
  **R3c/R5c 盖住宿主网关那条路**：两个库在宿主上各发布了一个端口，而「公网一跳」的落点正是宿主，
  所以「两库不通」还欠这一条。它们各自的正对照是同一个网关上的白名单口必须仍然通（R7 / R5d）——
  否则整条网关路断掉时，R3c 会为了错误的理由变绿。
  **总账分两笔**：R0（同架构、同 glibc）是 #151 的构建目标，不是 #152 的拓扑判据，单独记，
  不混进拓扑那一笔。
- **R 不是第五个台架字母**：A/B/C 编的是**搬运语义的验收场景**（ADR-0040 §2），R 编的是演练台
  自己的拓扑自检，一条搬运语义都不碰。第二版的验收判据是过程性的、落在演练记录里
  （ADR-0041 §6），不新开验收入口。

**真机差异**（手册要标出来的地方，见规格 #149 E.17）：容器里 root 是默认的、没装过任何东西、
网络是通的，`host.docker.internal` 更是 Docker Desktop 才有的东西——真机上对应的是客户给的
公网 IP 与白名单端口。ADR-0041 增补 1 明文接受这个代价：装的人就是写手册的人本人。

**yum 源不是差异，是两边都要先做的第一步**：CentOS 7 已 EOL，`mirrorlist.centos.org` 已停服，
`centos:7` 容器与客户那台真机**同样**装不上任何包（`yum install stunnel` 直接失败），
都得先把 repo 指到 `vault.centos.org`（`packaging/centos7/Dockerfile`（#151）里已经有这段改源）。
演练台在这一点上**不比真机宽松**，正好——手册怎么写这一步是 #155/#156 的事，不在本票。

## 三份视觉走查的驱动脚本（`walkthrough/`）

`CLAUDE.md` 的 Visual gates 把 V1–V25 / W1–W6 / X1–X8 定成硬门禁，但驱动它们的桩后端与探针
一直只躺在某台机器未跟踪的 `.playwright/` 下——**门禁是硬的、跑门禁的工具却换台机器就没了**。
2026-08-19 起这套脚本挪进 `walkthrough/`，与四份 acceptance 台架同处一棵树、一起入库。
`.playwright/` 只留 `cli.config.json` 这类本机配置，继续不跟踪。

```bash
cd docs/spikes/fixtures/local-rig
./walkthrough/run-v-walkthrough.sh    # V1–V25：起 v-mock.py  → 跑 v-probe.py（端口 18097）
./walkthrough/run-w-walkthrough.sh    # W1–W6： 起 m3-mock.py → 跑 m3-probe.py（端口 18099）
./walkthrough/run-x-walkthrough.sh    # X1–X8： 起 v1-mock.py → 跑 v1-probe.py（端口 18098）
```

- **桩后端 + 真实构建产物**：三份 mock 各自把对应态原样造出来，喂的是仓库根 `npm run build`
  产出的**真实 `web/dist`**（mock 沿父链找它，找不到就明说要先构建）。造态是假的，渲染面是真的。
- **只观察、不断言**（ADR-0028 §1）：探针把 JSON 打到 stdout 给人抄进走查记录，
  一行 DOM 断言都不进验收套件；**判据已退役的条目照实报 `retired`**，不许报「通过」、不许跳过。
- **playwright 装在仓库外**：探针默认用 `~/pwvenv/bin/python`，换个位置传 `PW_PYTHON=...`；
  端口传 `PORT=...`。三支 runner 都会先查这个解释器在不在，不在就直说，不会跑到一半才炸。
- `*-summary.py` 是把探针输出挑重点打出来的小工具，读 `/tmp/{v,w,x}-obs.json`，
  换路径传 `OBS_JSON=...`。
- `switch-child-mode.sh` 是 M2 走查专用：`M2_KEEP_RIG` 交出来的台架固定停在 `hang-streaming`，
  V2/V6/V7/V10–V12/V15/V22 要 `real`，用它按 `start_source` 同一套参数重起 source。
  **work_root 必须传**（或设 `M2_WORK_ROOT`）——那是每次都不同的临时目录。
- `wt-runs.py` / `walkthrough-history.py` / `walkthrough-tasks.py` / `wt-extra.py` 是对着
  **真台架**（18088）取观察的几支，不配桩后端。
