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
./scripts/run-bulk-probe.sh              # 跑 #5 的内存形状探针（19 组配置矩阵）
REPS=7 ./scripts/run-cpu-probe.sh        # 跑 #5 的客户端每行 CPU 探针（每档取中位数）
./scripts/run-mysql-roundtrip-probe.sh   # 跑 #13 的目标端往返实测（只起 MySQL，不用 Oracle）
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
commit 及其 `SELECT COUNT(*)` 子项、`purged_rows`、逐批实际行数和序列化字节分布。报告按
push/cursor 超过 50%、commit 对 30 分钟读超时、最大批次对 16 MiB 三条口径复审；批次行数
始终从 JSONL 求和和排序，不假设固定值。

commit 场景由 `acceptance/commit-drop-proxy.py` 在 sink 完成事务后切断第一次 commit 响应，
随后转发 source 唯一一次诊断 GET，以稳定复现 `COMMITTING` 的不确定窗口。代理用
`M1_COMMIT_DROP_MODE` 选择断连后留下的终态：`swapped`（默认）原样转发 commit；`discarded`
在转发前把 `total_rows` 加一，令 sink 的暂存/源端行数校验失败，从而丢弃暂存表并落
`DISCARDED` 墓碑——两种模式下 source 都只能靠墓碑 GET 判断目标表是否已被切换。代理只用于本台架。

kill sink 场景在 source 推完部分批次后 `kill -KILL` sink，验证 source 以 1 退出、终止在
`STREAMING` 且不产生 `commit_diagnosed`，目标表保持原样；重启 sink 后重跑必须换用新 `run_id`
并复现直连基线的目标表哈希。
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
