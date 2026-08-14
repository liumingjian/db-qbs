# #2 上线前客户环境复验

服务于 [#2](https://github.com/liumingjian/db-qbs/issues/2)。这里的 SQL 是**给人在客户环境里跑**的；
它们只采事实，不把任何一项判成通过。七项结论都要在 M4 收尾前写进
[`docs/spikes/0001-oracle-driver.md`](../0001-oracle-driver.md) 第 1 节。

## 目标表

`htbr45.t_r_fr_aststat@FA` —— 经 dblink 指向更远端的库。
**因此所有字典查询都走 `ALL_TAB_COLUMNS@FA`，本地数据字典里没有这张表。**

## 执行顺序

| # | 脚本 | 命令 | 产出 |
|---|---|---|---|
| 0 | `01-env-facts.sql` | `sqlplus user/pass@src @01-env-facts.sql` | 版本、字符集、对象类型、`undo_retention`，整段贴回 #2 |
| 1 | `02-columns.sql` | `sqlplus -S user/pass@src @02-columns.sql > columns-t_r_fr_aststat.csv` | 提交 CSV |
| 2 | `03-type-census.sql` | `sqlplus user/pass@src @03-type-census.sql` | 贴回 #2 |
| 3a | `04-gen-boundary-samples.sql` | `sqlplus -S user/pass@src @04-gen-boundary-samples.sql > 05-boundary-samples.sql` | 中间脚本，先审再跑 |
| 3b | `05-boundary-samples.sql` | `sqlplus -S user/pass@src @05-boundary-samples.sql > samples-t_r_fr_aststat.csv` | 提交 CSV（**脱敏后**） |

若步骤 0 查询远端 `V$PARAMETER` 报权限错误，不得把空输出当结论；请 DBA 在 `@FA` 指向的库执行
`SELECT name, value FROM v$parameter WHERE name = 'undo_retention'`，把结果并入同一份记录。

## 七项复验与判定

| # | 怎么验 | 必须记录的结果 | 不符时的动作 |
|---|---|---|---|
| 1 | 用步骤 1/2 的列清单和类型普查逐列比对 ADR-0003 白名单 | 列数、类型组合、白名单外命中 | 映射预检拒绝；按命中类型重开 #11，不能绕过 |
| 2 | 用真实 10 万行 run 的 `batch_pushed.rows` / `bytes` 统计序列化后行宽 | `sum(bytes)/sum(rows)`、最大单批 `bytes/rows`、5000 行外推字节数 | 复审 ADR-0011 的 5000 行/16 MiB 配比，并用真实列数复算 spike §4.7 的 CPU 外推 |
| 3 | 选步骤 3b 中含中文的真实行，走待上线同版本的完整 Oracle → MySQL 链路 | source 在序列化前的 UTF-8 字节与目标端 `HEX()` 读回值；必须逐值逐字节相同 | 乱码或截断即复审 ADR-0011 字符集边界，不能只凭行数通过 |
| 4 | 在真实服务端和网络上跑 10 万行完整链路 | fetch / 推送 / commit 分段耗时与总时长 | 总时长远超“几十秒”即复审 ADR-0001；先按分段耗时定位服务端或网络 |
| 5 | 读步骤 0 的远端版本 | 完整 11g 小版本 | `< 11.2.0.4` 时下调 Instant Client 版本 |
| 6 | 读步骤 0 的 `ALL_OBJECTS@FA` / `ALL_SYNONYMS@FA` | `TABLE` / `VIEW` / `SYNONYM` 及最终去向 | 非表时按 spike §5.1 复验 `Remote SQL Information` 的投影下推 |
| 7 | 在并发写入压力下跑真实 run，并与步骤 0 的远端 `undo_retention` 对照 | 全程游标寿命、fetch / 推送累计耗时、`undo_retention`、是否出现 `ORA-01555` | run 时长逼近或超过保留时间时，把 DBA 保证提升为 M4 硬前置；推送过半则另开流水线复审票 |

第 2、4、7 项使用待上线版本的 JSON Lines 日志：逐批行提供 `rows` / `bytes` / `ms`，`run_end`
提供 fetch / 推送 / commit 分段耗时。第 3 项必须保存 source 规范形式的 UTF-8 字节作为期望值；
只看 SQL 执行成功、行数相等或肉眼显示正常都不构成通过。

## 脱敏规则

样本是客户生产数据。可以脱敏，但 **必须保留精度特征**：

- `NUMBER`：可以改数字，**不能改位数和标度**。`123456789.12345` → `987654321.98765` 可以；
  → `100.0` 不行。精度正是 spike 要验的东西。
- 字符串：可以改内容，**必须保留字符数与字节数关系**（中文仍是中文，长度不变）。
- `DATE`：可以平移，**不能抹掉时分秒**。原本非零的时分秒必须仍非零。
- `NULL` / 空串：**原样保留**，不许互换——ADR-0003 要求两者可区分。

## 若客户库不可直连

退路：用 `columns-t_r_fr_aststat.csv` 在本地 Oracle 建结构等价的空表，灌入样本值。
CSV 落地后由 agent 生成 `create-equivalent-table.sql`，不用手写 70+ 列。
注意本地库要建成与源端相同的 `NLS_CHARACTERSET`，否则中文往返测试测的是假环境。

这个退路只能准备类型与样本测试，**不能**替代七项客户环境复验：服务端配置、真实吞吐、
真实网络和并发写入下的 undo 压力仍然未知。

## 验收

- [ ] 七项都有实测结论并写进 spike 第 1 节；没有把权限错误或空输出当成通过
- [ ] 类型清单已与 ADR-0003 白名单比对；白名单外命中已开回炉票
- [ ] 真实行宽已用于复核 5000 行/16 MiB 配比，并复算 spike §4.7 的 CPU 外推
- [ ] 中文样本已经过 Oracle → MySQL 完整链路的逐值逐字节比对
- [ ] 任一项不符时，受影响的 ADR 已复审而不是绕过
