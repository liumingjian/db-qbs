# M0-1 fixtures — 生产表元数据与样本

服务于 [#2](https://github.com/liumingjian/db-qbs/issues/2)。这里的 SQL 是**给人在客户环境里跑**的，
不进主干构建；跑出来的 CSV 才是 #3 / #5 / #6 依赖的产出。

## 目标表

`htbr45.t_r_fr_aststat@FA` —— 经 dblink 指向更远端的库。
**因此所有字典查询都走 `ALL_TAB_COLUMNS@FA`，本地数据字典里没有这张表。**

## 执行顺序

| # | 脚本 | 命令 | 产出 |
|---|---|---|---|
| 0 | `01-env-facts.sql` | `sqlplus user/pass@src @01-env-facts.sql` | 贴回 #2 |
| 1 | `02-columns.sql` | `sqlplus -S user/pass@src @02-columns.sql > columns-t_r_fr_aststat.csv` | 提交 CSV |
| 2 | `03-type-census.sql` | `sqlplus user/pass@src @03-type-census.sql` | 贴回 #2 |
| 3a | `04-gen-boundary-samples.sql` | `sqlplus -S user/pass@src @04-gen-boundary-samples.sql > 05-boundary-samples.sql` | 中间脚本，先审再跑 |
| 3b | `05-boundary-samples.sql` | `sqlplus -S user/pass@src @05-boundary-samples.sql > samples-t_r_fr_aststat.csv` | 提交 CSV（**脱敏后**） |

步骤 0 的两个事实（11g 小版本、`NLS_CHARACTERSET`）是 #7 遗留的待确认项，是 #3 的直接输入：
**若小版本 < 11.2.0.4，19c Instant Client 不在认证范围，#3 的客户端版本要往下退。**

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

## 验收（对应 #2 的判据）

- [ ] `columns-t_r_fr_aststat.csv` 已提交
- [ ] 类型清单已写进 `docs/spikes/0001-oracle-driver.md` 第 1 节，#3 据此确定测试范围
- [ ] 每种类型 ≥3 个边界样本，含 ADR-0003 的预期规范形式
- [ ] #3 / #5 / #6 能在这套环境上跑起来
