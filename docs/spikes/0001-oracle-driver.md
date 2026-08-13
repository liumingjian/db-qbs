# Spike 0001 — Oracle 驱动选型

闸门 issue：[#1 M0: Oracle 驱动 spike（风险闸门）](https://github.com/liumingjian/db-qbs/issues/1)

> 本文档由 M0 各子任务分节填写，最终由 #8 汇总出闸门决策。
> spike 代码一次性，不进主干；本文档进主干。

## 1. 环境与样本（#2）

_待填_

## 2. `oracle` crate / ODPI-C 类型保真度（#3）

**结论：通过。** `NUMBER` 能以字符串取到 38 位完整精度，ADR-0003 的硬前提成立。
台架覆盖到的每一种类型都能保真读出，**FAIL 0 项**。

> 跑法：`docs/spikes/fixtures/local-rig/scripts/run-odpi-probe.sh`（在 #9 台架上，
> arm64 原生 Instant Client 19.32 → amd64 模拟的 XE 11.2.0.2）。
> 探针源码 `docs/spikes/fixtures/local-rig/spike-odpi/`，一次性，不进主干。
> 期望值全部来自台架的 `t_canon_expected`，**没有硬编码在 Rust 里**。

### 2.1 闸门判据：`NUMBER` 完整精度

```
[PASS] n_int38 取成 String = "12345678901234567890123456789012345678"（38 位有效数字）
[PASS] n_bare  取成 String = "12345678901234567890123456789012345678"（38 位有效数字）
[对照] 同一值取成 f64     = 12345678901234568000000000000000000000  ← 精度已丢
```

`row.get::<_, String>()` 走的是 ODPI-C 的原生 bytes 路径，**不经过 `f64`、也不经过
`rust_decimal`**；同一行取成 `f64` 的对照数据就在上面，两者放一起即为 ADR-0003 的论据。

### 2.2 逐类型判定

| 类型 | 边界 | 结果 |
|---|---|---|
| `NUMBER`（无精度声明 / 38,0 / 38,10 / 18,2） | 38 位满精度正负、零与负零、尾零、高标度、小于 1 的小数 | PASS |
| `DATE` | 非零时分秒 | PASS（`2026-08-13 14:35:09`） |
| `TIMESTAMP(6)` | 固定 6 位补零 | PASS（`2026-08-13 14:35:09.120000`） |
| `VARCHAR2` / `NVARCHAR2` 中文 | UTF-8 往返 | PASS |
| `CHAR(10)` / `NCHAR(10)` | 尾部空格保留 | PASS（`"AB        "`、`"甲乙        "`） |
| `NULL` | 与非 NULL 可区分 | PASS |
| `RAW` / `BLOB` | 字节无损 | 读出正常（`DEADBEEF00` / `0001020304FF`），但**规范形式 ADR 未定义** |
| `CLOB` / `NCLOB` | 中文混合内容 | 读出正常，**规范形式 ADR 未定义** |
| `LONG` / `LONG RAW` | 11g 遗留类型，ODPI-C 已知弱项 | **读出正常** —— 预想的坑没踩到 |
| `BINARY_FLOAT` / `BINARY_DOUBLE` | 值域 | 读出正常，但**规范形式 ADR 未定义**，见 2.4 |

汇总：`PASS 20 / FAIL 0 / UNSPEC 3`（`t_canon_expected` 的 23 个单元格全部判定）。

### 2.3 意外发现：驱动的 `NUMBER` 字符串已经就是规范形式

`0.5` / `-0.01` / `1.23`（源值 `1.2300000000`）/ `0`（源值 `-0`）——
**驱动给出的原始字符串与 ADR-0003 规范形式逐字节相同**，规范化函数在这些用例上是恒等变换。

这直接回答 [#10](https://github.com/liumingjian/db-qbs/issues/10)：
`sqlplus` 的 `TO_CHAR(0.5)` 返回 `.5`，但 **ODPI-C 返回 `0.5`（保留小数点前的 0）**。
两者不一致，说明「`TO_CHAR` 的行为」不能当成规范形式的依据。
按 ODPI-C 的输出定 ADR，规范化这一步在 `NUMBER` 上就退化成校验，实现风险最低。

### 2.4 需要 ADR 补课的缺口

`BINARY_DOUBLE` 的 `1.7976931348623157E+308` 取成字符串是 **309 位的十进制展开**
（`17976931348623157` 后跟 292 个 `0`），不是科学计数法。二进制浮点本身就不是十进制精确值，
「一律走字符串」对这两个类型**不构成无损往返的保证**。
`RAW` / `BLOB` / `CLOB` / `NCLOB` 同样没有规范形式定义（本报告暂按十六进制大写 / 原样文本表示）。
这不是驱动的问题，是 ADR-0003 的覆盖缺口 —— 已开 [#11](https://github.com/liumingjian/db-qbs/issues/11)。

### 2.5 本条**没有**回答什么

1. **GBK 中文往返未测。** 台架 `NLS_CHARACTERSET` 是 `AL32UTF8` 且改不了（见台架 README 边界一节），
   本条测的是 UTF-8 路径。生产库若为 `ZHS16GBK`，仍需在客户环境复测 —— **M0 的已知缺口**。
2. **真实类型清单仍缺。** 本条覆盖的是 ADR-0003 的「类型面」，不是客户表的列清单（#2 的产出）。
   若 #2 扫出 `XMLType`、`INTERVAL`、`TIMESTAMP WITH TIME ZONE` 等台架未覆盖的类型，需补测。
3. **吞吐与内存不在此列**（#5，且本台架测不了）。

因此本条**通过，但不单独关闭闸门** —— 闸门仍需 #5，以及在客户环境上复验 1 与 2。

## 3. `oracle-rs` 纯 Rust 类型保真度（#4）

_待填_

## 4. 流式 fetch 吞吐与内存（#5）

_待填_

## 5. dblink 列投影下推（#6）

**结论：Oracle 自己会把列投影推到远端。ADR-0004 的「把投影写进内层子查询」是防御性建议，不是必需的。**
**绑定变量能穿过 dblink，ADR-0004 不需要改方案。**

在本地替身台架（`docs/spikes/fixtures/local-rig/`，loopback dblink `@fa` 指回同一 XE 11.2.0.2）上，
建了一张 70 列的 `t_wide_probe`（贴近生产 `t_r_fr_aststat` 的形状）灌 5000 行随机值实测。
探针脚本 `probes/dblink-pushdown{,-2,-3}.sql`，跑法 `./scripts/run-dblink-probe.sh [脚本名]`。

### 5.1 远端 SQL 里带没带列裁剪

关键证据是执行计划的 **Remote SQL Information** —— 那才是真正发到远端的 SQL。
要看到它，查询必须同时碰本地对象（纯远程查询会被判成 `fully remote statement`，整条语句发过去，
连 `REMOTE` 行源都没有）。因此对比在「远程宽表 join 一张本地小表」的混合形状上做：

| 形状 | 内层写法 | Plan hash | 发到远端的 SQL |
|---|---|---|---|
| A（生产原样） | `SELECT *` | 414720286 | `SELECT "ROW_ID","D_ASTSTAT","C01","C02" FROM "T_WIDE_PROBE" "A" WHERE "D_ASTSTAT"=TRUNC(:1-1)` |
| B（ADR-0004 建议） | 投影写进内层 | 414720286 | **完全相同** |
| A + `NO_MERGE` | `SELECT *`，禁止子查询合并 | 3814347444 | **完全相同** |

三点：

1. **70 列里只有 4 列过网络** —— 外层引用的 3 列，加上 WHERE 用到的 `D_ASTSTAT`。列投影下推成立。
2. **A 与 B 的计划完全一致**（同 plan hash、同远端 SQL）。改写不带来任何收益。
3. **连 `NO_MERGE` 都推不坏它** —— 加了 hint 后本地多一层 `VIEW`，但远端 SQL 一字不差。
   投影裁剪不依赖子查询合并，所以它不是「碰巧生效」。
4. 顺带：**WHERE 谓词也一并下推**了，不是拉回本地再过滤。

### 5.2 网络传输量实测

`v$mystat` 的 `bytes received via SQL*Net from dblink`，5000 行：

| 取法 | 收字节 | B/行 |
|---|---|---|
| 内层 `SELECT *`，外层 3 列 | 656,992 | 131.4 |
| 内层已投影，外层 3 列 | 656,595 | 131.3 |
| 混合（远程 join 本地），外层 3 列 + tag | 667,448 | 133.5 |
| **对照：真取全 70 列** | **20,830,956** | **4166.2** |

前两行差 0.06%，是噪声。对照行说明计数器对列宽完全敏感（32 倍差距），
所以前两行的「没差别」不是计数器不灵，是真没差别。

> 坑（留档）：第一轮测出来只有 12 B/行，因为填充数据每行同值，**SQL\*Net 会去重重复列值**。
> 第三轮改灌随机值才得到上面的数字。以后拿字节计数器做对比，填充数据不能同值。

### 5.3 绑定变量能否穿过 dblink（这条影响 ADR-0004）

**能。** `:biz_date` 到了远端仍是绑定变量，没有被拼成字面量：

```
Remote SQL Information:
   3 - SELECT "ROW_ID","D_ASTSTAT","C01" FROM "T_WIDE_PROBE" "A" WHERE "D_ASTSTAT"=:1
```

实跑 `WHERE a.d_aststat = TO_DATE(:biz_date,'YYYY-MM-DD')` 命中 5000 行。
（顺带印证反面：不参数化时 `TRUNC(SYSDATE-1)` 在远端变成 `TRUNC(:1-1)` ——
`SYSDATE` 在**本地**求值后当绑定值送过去，这正是 ADR-0004 要消灭的「运行时才求值」。）

### 5.4 dblink 故障的错误特征（M4 要能区分本地 / dblink 问题）

| 故障 | SQLCODE | 错误文本 |
|---|---|---|
| 监听端口不通 | `-12541` | `ORA-12541: TNS:no listener` |
| 主机名解析不了 | `-12154` | `ORA-12154: TNS:could not resolve the connect identifier specified` |
| 远端口令错 | `-1017` | `ORA-01017: invalid username/password; logon denied`<br>`ORA-02063: preceding line from FA_BAD_CRED` |
| 远端表不存在 | `-942` | `ORA-00942: table or view does not exist`<br>`ORA-02063: preceding line from FA` |
| 本地表不存在（对照） | `-942` | `ORA-00942: table or view does not exist` |

**判别规则给 M4：`ORA-02063: preceding line from <LINK_NAME>` 是 dblink 的签名。**
它带着 link 名，且只在远端报错时出现 —— 上表最后两行错误码完全相同，
唯一的区别就是有没有这一行。M4 的错误分类应当扫 `ORA-02063` 并抓出 link 名，
而不是去枚举 `ORA-125xx`（TNS 层错误压根不带 `ORA-02063`，要单独归一类「连不上远端」）。

另有 `v$dblink` 可查当前会话打开着哪些 dblink（`DB_LINK` / `IN_TRANSACTION`），排障时有用。

### 5.5 本条**没有**回答什么

- **台架是 loopback dblink**（指回同一实例），复现的是远程优化器路径的**形状**，不是跨机网络。
  字节数真实（走了本地 listener 的 TNS），**RTT 与吞吐不真实** —— 那是 #5 的事，且 #5 也测不了。
- **生产的 `@FA` 指向的远端库版本未知**。远端 SQL 的生成是本地 CBO 的行为，与远端版本无关，
  但若 `htbr45.t_r_fr_aststat` 在远端其实是**视图或同义词**，下推行为可能不同 ——
  这条要在 #2 拿到客户环境后复验一次。
- 生产查询是否也 join 本地对象未知。若它像台架的纯远程形状一样只碰 `@FA`，
  Oracle 会走 `fully remote statement`（整条语句发过去执行），那是更好的情况，投影问题不存在。

## 6. 客户源端机器部署 Instant Client 可行性（#7）

**结论：可行。** 源端机器由我方提需求、客户方申请交付，安装第三方软件与获取
离线包都在可满足范围内。ODPI-C 路线（#3）的部署风险解除。

### 6.1 源端机器环境

机器为**新申请交付**，非既有机器，因此环境是我方提需求时**指定**的，不是探测出来的。
提资源需求时按下表写死：

| 项 | 要求 | 理由 |
|---|---|---|
| 操作系统 | 主流 Linux 发行版，glibc ≥ 2.14 | Instant Client 19c 的下限（RHEL/CentOS 7+、Ubuntu 18.04+ 均满足） |
| CPU 架构 | x86_64 | Oracle 只为 x86_64 提供完整的 Instant Client 版本矩阵；ARM 上 19c 无 Linux 版 |
| 权限 | 应用账号可写自己的安装目录即可，**不强制要 root** | 库路径走 `LD_LIBRARY_PATH`，不动 `/etc/ld.so.conf.d/` |
| 出网 | 不要求 | 按离线包带入处理，不依赖机器能访问 download.oracle.com |
| 到目标端 | 允许 HTTPS 单向出站 | `source` → `sink` 的唯一通道，见 CONTEXT.md |

### 6.2 Instant Client 获取与安装

| 项 | 结论 |
|---|---|
| 版本 | **19c**（不是 21c）—— 见 6.3 的版本兼容性 |
| 包 | **完整 Basic**（约 80 MB），**不用 Basic Light** |
| 获取途径 | 离线包随应用制品一起带入，不依赖源端出网 |
| 安装方式 | 解压到应用目录，`LD_LIBRARY_PATH` 指向 `instantclient_19_x`；不需要 root，不需要 `ldconfig` |
| 许可 | OTN 免费许可，随应用分发需接受 OTN License |

**为什么是完整 Basic 而不是 Basic Light**：Basic Light 只内置 US7ASCII / WE8DEC /
WE8MSWIN1252 等少数字符集。国内 11g 生产库大量是 `ZHS16GBK`，Light 包读中文会直接失败。
源端 Oracle 的 `NLS_CHARACTERSET` 是客户既有生产库的属性、不由我们选，
所以按最坏情况取完整包 —— 多 50 MB，换掉一轮「装错了再走一次审批」的往返。

### 6.3 Oracle 服务端

| 项 | 值 | 影响 |
|---|---|---|
| 服务端版本 | **Oracle 11g**（V1 范围） | 决定 Instant Client 选 19c |
| 是否 ≥ 12.1 | **否** | **`oracle-rs`（#4）前置条件不满足 → 出局** |
| 目标端 | **MySQL 8.0** | 目标端统一 `utf8mb4` |
| `NLS_CHARACTERSET` | 待确认（不阻塞） | 已按最坏情况取完整 Basic 包，故不再是决策输入；但 #3 的中文往返测试需要知道它 |

**版本兼容性 —— 必须用 19c 客户端**：Oracle 的客户端/服务端互操作矩阵里，
21c 客户端的服务端下限是 12.1，**连不了 11g**；19c 客户端才向下支持到 **11.2.0.4**。

> **残留待确认（不阻塞本条，落到 #2/#3）**：11g 的具体小版本。
> 19c 客户端对 11g 的认证下限是 **11.2.0.4**；若客户库是 11.2.0.1–0.3 或 11.1，
> 需退到 18c/12.2 客户端，或走 11.2 版本的 Instant Client。

### 6.4 对 M0 闸门的影响

1. **#7 的原始风险解除** —— ODPI-C 能装，候选 A 在部署上成立。
2. **候选 B 出局** —— 11g < 12.1，`oracle-rs` 不用测（#4 已关闭）。
3. **因此 M0 从「两个候选择优」变成「ODPI-C 单点验证」**：#3 不通过就没有备胎，
   直接触发 ADR-0001 复审、回退 Java（JDBC thin 对 11g 无版本门槛）。#1 的通过判据已据此改写。
4. **11g 缩小了类型面** —— 无 SQL `JSON`、无 SQL `BOOLEAN`；反过来
   `LONG` / `LONG RAW` / `XMLType` 这类遗留类型在 11g 库里出现的概率更高，#2 的类型清单要重点扫。

## 7. 闸门决策（#8）

_待填_
