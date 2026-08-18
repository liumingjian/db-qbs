# ADR-0033: 字符长度按驱动的字节口径判——第一版认下这份过严，真字符数留待后续

**状态**: 已接受
**日期**: 2026-08-18
**关联**: [ADR-0030](0030-m3-type-whitelist.md)（**订正其 §1 形态 5/7 与 §7 的「`n` 已是字符数」**）、
[ADR-0032](0032-m3-acceptance-criteria-and-rig-extension.md)（**订正其 §4.1 矩阵形态 5/7 的目标列宽**）、
[ADR-0009](0009-m1-mapping-precheck-rules.md) §2（字符族下界式 `n' >= n`，**本 ADR 不动它**）、
[ADR-0014](0014-canonical-form-boundary-test-suite.md) §12（驱动版本钉死，**本 ADR 不动它**）、
[#106](https://github.com/liumingjian/db-qbs/issues/106)（M3 规格，B1 在此暴露）

## 背景

M3 验收 B1「九行形态正面往返」FAIL：源 `VARCHAR2(10 CHAR)` 与 `CHAR(10 CHAR)` 两列整个
`DISCARDED`，报「目标 VARCHAR 长度不足」。

根因在驱动边界。`oracle` 0.6.3 `src/sql_type/oracle_type.rs:339-342`：

- `DPI_ORACLE_TYPE_VARCHAR` / `DPI_ORACLE_TYPE_CHAR` → `info.dbSizeInBytes`（**字节**）
- `DPI_ORACLE_TYPE_NVARCHAR` / `DPI_ORACLE_TYPE_NCHAR` → `info.sizeInChars`（**字符**）

于是 AL32UTF8 下 `VARCHAR2(10 CHAR)` 的 `length` 是 **40**，撞上按字符比的下界式
`n' >= n`（`crates/sink/src/precheck.rs` `validate_varchar`），把一张本来正确的目标表判为不合规。
`NVARCHAR2(10)` / `NCHAR(10)` 因为拿到的是字符数，同一轮里通过——
这与 ADR-0030 §7「`NVARCHAR2` 判定式与 `VARCHAR2` 逐字相同」直接矛盾。

**ADR-0030 §1 形态 5/7 与 §7 里「`n` 已是字符数，直接对齐」这句话，对 `VARCHAR2` / `CHAR` 是错的。**
它引自 spike-0001 §7.3，但 §7.3 量的是 N 族。M1/M2 从没暴露，是因为那批 fixture 全是字节语义
（`oracle/02-schema.sql` 的 `v_cn VARCHAR2(400)` 对 `mysql/10-target-schema.sql` 的 `VARCHAR(400)`）。

## 决策

### 1. 判定式一个字不改，认下过严

`n' >= n` 保留，`n` 就是 describe 给什么算什么。**这是只知道字节数时的紧下界，不是可以随手放宽的保守值**：
`n_bytes` 字节最多装 `n_bytes` 个字符（每字符至少 1 字节），而 MySQL `VARCHAR(n')` 的 `n'` 按字符计，
所以 `n' >= n_bytes` 恰好是安全边界。**任何按字节对字节的放宽都是不安全的**——
`VARCHAR2(10 BYTE)` 对 `VARCHAR(3)` 会满足「10 字节 ≤ 12 字节」而放行，
但 10 个 ASCII 字符进 3 字符的列必炸 `ERROR 1406`。此路已封，勿再走。

### 2. 代价写明：`CHAR` 语义的源列，目标列要建 4 倍宽

本系统**不自动建表**，目标表由用户手工预建（见 §4）。因此这条代价是用户直接承担的：

| 源列 | describe `length` | 目标列必须 | 说明 |
|---|---|---|---|
| `VARCHAR2(n BYTE)` | `n` | `VARCHAR(n)` | 正好，无浪费 |
| `VARCHAR2(n CHAR)` | `4n`（AL32UTF8） | `VARCHAR(4n)` | **4 倍宽，用不满** |
| `CHAR(n CHAR)` | `4n` | `VARCHAR(4n)` | 同上 |
| `NVARCHAR2(n)` / `NCHAR(n)` | `n` | `VARCHAR(n)` | 正好 |

**统一表述：目标列宽照抄 `generate_target_ddl` 吐出的 `VARCHAR(length)`。**
不要写成「字符语义列建 4 倍宽」——线上区分不出 `VARCHAR2(10 BYTE)` 与 `VARCHAR2(10 CHAR)`
（它们的 `length` 分别是 10 和 40），照抄生成侧是唯一无需判断的规则，且生成侧与判定侧本来就已满足它。

### 3. ADR-0030 §7 的对称性表述订正

`NVARCHAR2` 与 `VARCHAR2` **目标端往返面仍然同构、失效模式仍然相同**，
但**推导出的目标列宽不同**，因为两者的 describe 单位不同。§7 那句「判定式与 `VARCHAR2` 逐字相同」
按「判定式的形相同（`n' >= n`）、代入的 `n` 口径不同」理解。
**不为了这句话的好看去把 N 族 `length` 乘 4**——那是拿已知精确的一端去劣化到未知的那一端，
还会让 M1/M2 既有 fixture 的 N 族列全部回归失败。

### 4. 不做自动建表（本 ADR 顺带把既成事实立成明文）

目标表、索引等对象**一律由用户手工提前创建**，系统不执行任何针对用户目标表的 DDL。
`generate_target_ddl` 只在 `crates/source/src/server_main.rs:901` 产出 **DDL 文本**放进 JSON 响应供用户抄，
从不执行。全仓唯一真执行 `CREATE TABLE` 的是 `crates/sink/src/service.rs:757`，
建的是内部暂存表 `__stg_<ts>_<hash>`，属于「先落暂存再原子换」的搬运机制，不是用户对象。

**这是禁令，不只是描述**：后续不得新增「一键建表」之类的能力。

## 后果

- ADR-0032 §4.1 矩阵形态 5/7 的「目标列」一格由 `VARCHAR(10)` 改为 `VARCHAR(40)`，
  形态 6 保持 `VARCHAR(10)`。**边界值与期望值一格不动**——加宽目标列不改变存进去的字节，
  §4.3 的 `HEX()` 逐字节比对结论不变。
- `docs/spikes/fixtures/local-rig/acceptance/mysql-m3.sql` 的 `M3_B1` 表 `V_TEXT` / `C_TEXT` 建 `VARCHAR(40)`。
- 用户侧的实际影响：`VARCHAR2(n CHAR)` 源列会收到一条「目标 VARCHAR 长度不足，改为 `VARCHAR(4n)`」的
  预检提示。**这条提示是准确的下界，不是误报**——虽然那 4 倍宽物理上用不满。

## 时效

**这是第一版的取舍，不是终局。** 真字符数的取法已经调研清楚，留待后续版本：

- **可行路径（推荐）**：describe 时对字符族列回查 `ALL_TAB_COLUMNS.CHAR_LENGTH`。
  该字典查询**仓库里已经存在**——`crates/source/src/sql_builder.rs:111` 的 `builder_column_query`
  已在 SELECT `CHAR_LENGTH`，`BuilderColumn.length`（`sql_builder.rs:29`）装的就是真字符数。
  也就是说现在**两套 `length` 并存、单位不同**：builder（字符）走字典喂 UI，
  describe（字节）走驱动喂预检。查不到的列（表达式列、多表 join、`CAST`）退回字节紧下界，安全性不降。
- **已封路，勿重走**：Oracle 11.2 的 `DBMS_SQL.DESC_REC3` **没有** `col_char_len`（台架上 `PLS-00302` 实证），
  只有 `col_max_len` / `col_charsetform`；`oracle` 0.6.3 已是 crates.io 最新版，
  `ColumnInfo` 只暴露 `name()` / `oracle_type()` / `nullable()`，`oci_attr` 只支持
  SvcCtx/Server/Session/Stmt 四种 handle，够不到 column param。不 vendor 驱动就拿不到 `sizeInChars`。
- **放宽是单向门**，与 ADR-0031 同性质：一旦按真字符数放宽，退回字节口径会让已经在跑的任务成批被拒，
  属破坏性变更，要退必须当新决策重走。
