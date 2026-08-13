# ADR-0004: 业务日期参数化，禁止 SQL 中的相对时间表达式

**状态**: 已接受
**日期**: 2026-08-13

## 背景

用户当前在跑的生产查询是这个形状：

```sql
SELECT t."ID_R_FR_ASTSTAT", t."D_ASTSTAT", ... /* 70+ 列 */
FROM (
    SELECT * FROM htbr45.t_r_fr_aststat@FA a
    WHERE a.d_aststat = TRUNC(SYSDATE - 1)
) t
```

`TRUNC(SYSDATE - 1)` 是相对时间表达式，带来两个问题：

1. **不可重放。** 同一个任务今天跑和明天跑取到的是不同数据集。「重跑一次」这个操作
   失去意义，也就谈不上最终一致性。
2. **两端会错位。** 目标端的清除条件必须圈定与源端 WHERE 相同的范围（ADR-0002）。
   如果源端 WHERE 在运行时才求值，而清除条件在另一个时刻求值，跨零点时两者指向不同的日期。

## 决策

**SQL 里禁止出现 `SYSDATE`、`CURRENT_DATE`、`SYSTIMESTAMP` 等相对时间表达式。**
业务日期提升为运行的显式参数，绑定到命名参数 `:biz_date`：

```sql
SELECT t."ID_R_FR_ASTSTAT", t."D_ASTSTAT", ... /* 70+ 列 */
FROM (
    SELECT "ID_R_FR_ASTSTAT", "D_ASTSTAT", ... /* 投影下推，见下 */
    FROM htbr45.t_r_fr_aststat@FA a
    WHERE a.d_aststat = :biz_date
) t
```

- 任务启动时钉死一个业务日期，写进 run 记录，全程只读这一个值。
- UI 默认填「昨天」，用户可改——**补数（重跑历史某天）由此免费获得**。
- 目标端清除条件由同一个业务日期推导：`DELETE FROM target WHERE d_aststat = :biz_date`。
- SQL 构建器生成的 SQL 天然满足此约束；用户手改 SQL 后需通过静态检查（扫描相对时间函数）
  才允许保存。

## 附带发现：dblink 与投影下推

`htbr45.t_r_fr_aststat@FA` 中的 `@FA` 是 database link——目标表不在直连的 Oracle 上，
而在更远端的库。两点影响：

- **多一个故障点**：dblink 不可用时任务失败，错误信息需要能区分「本地库问题」和「dblink 问题」。
- **投影可能不下推**：原查询内层 `SELECT *`、外层才选 70+ 列。Oracle 未必把投影推到远端，
  最坏情况是整表所有列过网络后再丢弃。SQL 构建器应**把列投影写进内层子查询**。
  实际下推行为需在 M0 spike 中用执行计划确认。

## 代价

用户不能再直接粘贴他原来那条 SQL——需要改写。这是有意的：可重放性是最终一致性的前提，
不能为了省一次改写而放弃。构建器会替用户生成正确形状，手写路径给出明确的报错提示。
