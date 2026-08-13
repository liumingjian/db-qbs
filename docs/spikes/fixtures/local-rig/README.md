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
oracle/02-schema.sql        等价表 —— ADR-0003 每一种规范形式至少一列
oracle/03-boundary-rows.sql 边界值 + t_canon_expected（期望规范形式）+ 10 万行
oracle/04-dblink.sql        指回自身的 loopback dblink，名字沿用生产的 @FA
mysql/10-target-schema.sql  目标端等价表，utf8mb4
probes/dblink-pushdown*.sql #6 的 dblink 列投影探针（**不是** initdb 脚本，起库后按需跑）
```

`probes/` 里的脚本不参与建库，靠 `scripts/run-dblink-probe.sh` 手动跑；
它们自建/自删所需的表与 dblink，可重复执行。

`oracle/` 与 `mysql/` 分别挂进两个镜像的 initdb 目录，**只在首次建库时执行一次**。
改了 SQL 要重新生效，必须 `./scripts/down.sh && ./scripts/up.sh`（卷不删就不会重跑）。

### 期望值是数据，不是代码

`t_canon_expected(row_id, column_name, expected, note)` 存每个单元格按 ADR-0003 应有的规范形式。
**#3 的断言 join 这张表，不要把期望值硬编码进 Rust** —— 期望值是 ADR 的产物，改 ADR 应该只改这张表。

覆盖到的边界：38 位满精度（正/负）、无精度声明的 `NUMBER`、高标度、尾零与负零、
`DATE` 非零时分秒、`TIMESTAMP` 固定 6 位、中文 `VARCHAR2`/`NVARCHAR2`、`CHAR`/`NCHAR` 尾空格、
全 `NULL` 行、`RAW`/`CLOB`/`NCLOB`/`BLOB`/`BINARY_FLOAT`/`BINARY_DOUBLE`，
以及各自单表的 `LONG` 与 `LONG RAW`（Oracle 限一表一个）。

## 边界 —— 本台架不能答什么

1. **`NLS_CHARACTERSET` 是 `AL32UTF8`，改不了。** XE 建库时定死，`ALTER DATABASE CHARACTER SET`
   只允许向超集转，AL32UTF8 → ZHS16GBK 是收窄，不放行。
   **#3 的 GBK 中文往返测不了**，只能测 UTF-8 路径。绕开办法是拿 EE 介质自己 build 一个
   ZHS16GBK 的 11.2.0.4 镜像，要 Oracle 账号和许可，**先不做**，记为 M0 的已知缺口。
2. **#5 的吞吐与内存不能在此测。** 服务端跑在模拟层上，绝对数字是废数据。
   `t_bulk_probe` 的 10 万行至多能看「流式 fetch 的内存是否随行数增长」这个**形状**，测不出吞吐。
3. **替代不了 #2 的真实列清单** —— 那是客户表的属性。等价表覆盖的是「类型面」，不是「列清单」。

因此本台架**不能关闭 M0 闸门**。它把 #3 从「完全没跑过」推进到「机制已验证，只差真实类型清单和 GBK 字符集」；
#5 和 #2 仍卡在客户环境上。

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
   `ORA-01426 numeric overflow`。ADR-0003 没有为 `BINARY_FLOAT/DOUBLE` 定规范形式，
   而「一律 `TO_CHAR` 走字符串」对这两个类型的往返精度需要 #3 单独确认。
3. **dblink 列投影 Oracle 自己会下推**（#6 已结）。内层 `SELECT *` 与投影写进内层生成的
   远端 SQL 一字不差，`NO_MERGE` 也推不坏；绑定变量能穿过 dblink。详见
   `docs/spikes/0001-oracle-driver.md` 第 5 节。**注意字节计数器的坑**：填充数据同值时
   SQL*Net 会去重重复列值，测出 12 B/行的假数字，必须灌随机值。
4. **Oracle 把空串存成 `NULL`（已实测 `v_ascii` 写 `''` 后 `IS NULL`）。**
   所以「`NULL` 与空串并存」在源端**不可能构造** —— ADR-0003 里 `NULL` 与空串的区分，
   只在目标端 MySQL 侧有意义。`CHAR(10)` 尾空格则确实保留（`LENGTH`=10），
   中文按 UTF-8 存（6 字符 / 18 字节）。
