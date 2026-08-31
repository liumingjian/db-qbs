//! 任务定义的**结构化规格**——它是唯一真相源。
//!
//! SQL 不进任务定义，由本模块从规格现算；界面上只读，没有编辑入口。
//! 派生面恰好两样：源端 SQL、报文里来自任务定义的字段。
//! **目标表 DDL 生成与 `OpenRunRequest` 不在其中**——它们吃 describe 回来的源列。
//!
//! 过滤是**一段自由 WHERE 文本**：用户写什么就原样拼进 `WHERE` 后面。用这门平台的人本来
//! 就写 SQL，「字段 + 三个比较符 + 值」那种四格表单表达不了他们真正要的条件，
//! 反而逼人绕道自定义 SQL。随之退役的还有整条运行参数链——运行时填的值只是
//! 结构化条件的一个属性，条件没了它就无所依附。发起一次运行因此不再需要任何输入。

use std::collections::BTreeSet;

use db_qbs_shared::{WriteMode, WriteStatement};
use serde::{Deserialize, Serialize};

use crate::cron::CronSchedule;

/// 一列的映射：源列名 → 目标列名。
///
/// 「默认同名映射」就是 `target` 预填成 `source`——那不是将就的默认值，它就是恒等映射的
/// 正确表达（别名即目标列名）。
///
/// 落到 SQL 上是投影的别名 `a.{source} AS {target}`，**不引入映射表**：目标列名本来就是
/// `SELECT` 投影的别名，在协议层再表达一遍等于让两端各留一份判定式。
/// 因此报文里仍然只有一份列名——目标名，协议不增字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnMapping {
    pub source: String,
    pub target: String,
}

/// 任务定义的全部内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sql: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dblink: Option<String>,
    pub owner: String,
    pub table: String,
    pub target_table: String,
    /// 过滤条件：**原样拼进 `WHERE` 后面的一段片段**，不含 `WHERE` 这个词本身。
    ///
    /// 不解析、不改写、不反解——真相源是这段文本，能不能跑由 Oracle 说了算。
    /// 空白（或缺席）就是不加 `WHERE`，即整表取数。
    ///
    /// 它是标量，**必须排在 `columns` 这个 array-of-tables 之前**：临时任务定义走 TOML
    /// 落盘，值排在表之后会直接序列化失败。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub where_clause: Option<String>,
    /// 写入模式（#261/#264）：「追加写」与「先清空再导入」两档，定义在
    /// [`db_qbs_shared::WriteMode`]。**不给默认值**——任务定义里必须写明白这一次要怎么写。
    ///
    /// 它是标量，和 [`Self::where_clause`] 一样**必须排在 `columns` 之前**。
    pub write_mode: WriteMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_sql: Option<String>,
    /// 调度用的**五字段 cron 表达式**（#265），按**服务器本地时区**解读。
    ///
    /// `None` 或空白 = 这个任务没有周期，只能手动发起。语法与语义只有一份定义，
    /// [`crate::CronSchedule`]；这里存的是**原文**，不是解析结果——人写的那一行是真相源，
    /// 存下解析结果等于让任务定义里多一份会和原文漂开的派生物。
    ///
    /// 它是标量，和 [`Self::where_clause`] 一样**必须排在 `columns` 之前**。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_cron: Option<String>,
    /// 调度的启停开关（#265）。
    ///
    /// 它和 [`Self::schedule_cron`] 是两件事，因此是两个字段：把表达式清空来「暂停」，
    /// 等于让人为了停一次而丢掉自己写好的那一行。开着但没有表达式是自相矛盾的，
    /// [`Self::validate`] 拒绝它。
    ///
    /// 它是标量，同样**必须排在 `columns` 之前**。
    pub schedule_enabled: bool,
    /// 去重用的主键列，**可以为空**（#261）。存的是**目标列名**——与
    /// [`ColumnMapping::target`]、过线报文、sink 侧比对同一个名字空间。
    ///
    /// 这个字段同时是**任务定义记下的写入语义**：非空 = 目标表有主键，按主键 upsert；
    /// 空 = 目标表没有可合并的唯一约束，本任务是纯追加写，重跑会产生重复数据。
    /// 派生只有一处，[`db_qbs_shared::WriteStatement::for_primary_key`]，两端读同一份。
    ///
    /// 目标端**现在**是否真是这个样子由 sink 侧预检核对——两边不符就拒跑，
    /// 写法绝不静默切换。这里只记当时解析出来的那一份。
    pub primary_key: Vec<String>,
    // 下面这个是 TOML 里的 array-of-tables，**必须排在所有标量之后**。
    /// 选中的列及其目标字段。
    pub columns: Vec<ColumnMapping>,
}

impl TaskSpec {
    /// 去掉首尾空白之后的 WHERE 片段；空白等同于没写。
    fn where_fragment(&self) -> Option<&str> {
        self.where_clause
            .as_deref()
            .map(str::trim)
            .filter(|clause| !clause.is_empty())
    }

    /// 本任务写的是哪一种语句。见 [`Self::primary_key`]。
    pub fn write_statement(&self) -> WriteStatement {
        WriteStatement::for_primary_key(&self.primary_key)
    }

    /// 源端 SQL。没有 WHERE 片段时就是整表取数——量级风险归台架去证，不在这里挡。
    ///
    /// 自定义 SQL **不原样执行**：外层再套一次投影。搬运链路只认结果列名——`transfer.rs`
    /// 把 `source.columns()`（执行语句的结果列）原样交给 sink，所以「结果列名就是目标列名」。
    /// 不套这一层，勾选落不了地（没勾的列照样过线），目标字段改名也会被静默忽略。
    pub fn source_sql(&self) -> String {
        if let Some(source_sql) = self.source_sql.as_deref() {
            let inner = normalize_source_sql(source_sql)
                .lines()
                .map(|line| format!("         {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            return format!(
                "SELECT {}\n  FROM (\n{inner}\n       ) q",
                self.projection("q")
            );
        }
        let projection = self.projection("a");
        let dblink_suffix = self
            .dblink
            .as_deref()
            .map(|link| format!("@{link}"))
            .unwrap_or_default();
        let mut sql = format!(
            "SELECT {projection}\n  FROM {}.{}{dblink_suffix} a",
            self.owner, self.table
        );
        if let Some(clause) = self.where_fragment() {
            // **一个字符不加不改**，多行片段的续行也不重排缩进。
            // 缩进看着更齐，但要做对就得知道哪个换行在字符串字面量里面——
            // `'a\nb'` 里插进去的空格会改掉那个字面量的值，也就改掉了搬的数据。
            // 认这件事需要一个词法器，而「不解析这段文本」正是本字段的立身之本。
            sql.push_str(&format!("\n WHERE {clause}"));
        }
        sql
    }

    /// 投影就是映射：`别名.源列 AS 目标字段`。按表选择与自定义 SQL 共用这一份，
    /// 两条路径生成的形状因此不会漂。
    fn projection(&self, alias: &str) -> String {
        self.columns
            .iter()
            .map(|mapping| {
                format!(
                    "{alias}.{} AS {}",
                    quote_if_folded(&mapping.source),
                    mapping.target
                )
            })
            .collect::<Vec<_>>()
            .join(",\n       ")
    }

    pub fn validate(&self) -> Result<(), String> {
        let custom_sql = match self.source_sql.as_deref().map(str::trim) {
            Some(sql) if !sql.is_empty() => Some(sql),
            Some(_) => return Err("source_sql 不能为空".to_owned()),
            None => None,
        };
        if let Some(source_sql) = custom_sql {
            validate_source_sql(source_sql)?;
            if self.dblink.is_some() {
                return Err("自定义 SQL 已包含源端查询路径，不能同时设置 dblink".to_owned());
            }
            if self.where_fragment().is_some() {
                return Err("自定义 SQL 模式不能再单独配置过滤条件，请直接写进 SQL".to_owned());
            }
        } else {
            validate_identifier(&self.owner, "owner")?;
            validate_identifier(&self.table, "table")?;
            if let Some(dblink) = &self.dblink {
                validate_identifier(dblink, "dblink")?;
            }
            if let Some(clause) = self.where_fragment() {
                validate_where_clause(clause)?;
            }
        }
        if self.target_table.trim().is_empty() {
            return Err("target_table 不能为空".to_owned());
        }
        if self.columns.is_empty() {
            return Err("至少要选一列".to_owned());
        }
        // 两个名字空间各查一次重复：源列重复是「勾了两遍」，目标字段重复会生成两个同名别名，
        // Oracle 不拒、sink 侧按名字对齐时后一列静默盖掉前一列——那是最难排的错，
        // 在源端就能判，所以在源端判。
        let mut selected = BTreeSet::new();
        let mut targets = BTreeSet::new();
        for mapping in &self.columns {
            validate_identifier(&mapping.source, "column")?;
            validate_identifier(&mapping.target, "target column")?;
            if !selected.insert(mapping.source.to_ascii_uppercase()) {
                return Err(format!("选中的列 {} 重复", mapping.source));
            }
            if !targets.insert(mapping.target.to_ascii_uppercase()) {
                return Err(format!("目标字段 {} 重复", mapping.target));
            }
        }
        // 主键不再必选（#261）。空的主键不是「还没填」，它是一个**有含义的值**：
        // 目标表没有可合并的唯一约束，这个任务写纯 `INSERT ... SELECT`。
        // 因此这里没有可判的东西——「目标表到底有没有主键」不是源端能回答的问题，
        // 它归 sink 侧的映射预检，那里仍是唯一的拦截点、仍是硬门。
        let mut key_columns = BTreeSet::new();
        for column in &self.primary_key {
            validate_identifier(column, "primary_key")?;
            let normalized = column.to_ascii_uppercase();
            if !key_columns.insert(normalized.clone()) {
                return Err(format!("主键列 {column} 重复"));
            }
            // 主键列必须落在选中的列里。比的是**目标字段**——`primary_key` 存目标名，
            // 拿它去比源列名会在改过名的列上误判。
            // 目标端约束是否真的存在由 sink 侧核对，这一条在源端就能判，所以在源端判。
            if !targets.contains(&normalized) {
                return Err(format!("主键列 {column} 不在选中的列里"));
            }
        }
        // 调度（#265）。语法由 `CronSchedule::parse` 说了算，它的 `Err` 就是给人看的那句话，
        // 原样带出去——在这里另写一句「cron 表达式不合法」会把真正的原因盖掉。
        // 拒绝发生在**保存**这一刻，不是等到该跑的那一刻：一个永远不会响的闹钟不该被存下来。
        if let Some(expression) = self.schedule_expression() {
            CronSchedule::parse(expression)?;
        } else if self.schedule_enabled {
            return Err("启用了周期调度就必须写一条 cron 表达式".to_owned());
        }
        Ok(())
    }

    /// 去掉首尾空白之后的 cron 表达式；空白等同于没配。与 [`Self::where_fragment`] 同一套口径。
    pub fn schedule_expression(&self) -> Option<&str> {
        self.schedule_cron
            .as_deref()
            .map(str::trim)
            .filter(|expression| !expression.is_empty())
    }

    /// 解析好的周期，只在**开关开着且表达式配了**的时候才有。
    ///
    /// 这是「这个任务此刻会不会被自动发起」唯一的判据——#266 的调度器读它，界面上的
    /// 「下次触发」也读它。开关与表达式各判各的会让两边在「开着但没表达式」上分叉。
    pub fn active_schedule(&self) -> Option<CronSchedule> {
        if !self.schedule_enabled {
            return None;
        }
        CronSchedule::parse(self.schedule_expression()?).ok()
    }
}

/// WHERE 片段的**唯一**校验：不许出现 `;`。
///
/// 这一条不是「防注入」——片段本来就是用户自己写的 SQL，他能写的这台机器上他本来就能跑。
/// 挡的是**语句拼接**：`;` 之后那一段会被生成器缝进一条本该只有一个 `SELECT` 的语句里，
/// 于是「预览的是这条、执行的是那条」。同一条口径也管着自定义 SQL（见 [`validate_source_sql`]）。
///
/// 除此之外一律放行：括号配不配对、列名存不存在、函数认不认识，都由 Oracle 当场说了算，
/// 在这里再判一遍等于自己养一个会漂的解析器。
fn validate_where_clause(clause: &str) -> Result<(), String> {
    if clause.contains(';') {
        return Err("过滤条件里不能出现分号：它只是拼进 WHERE 的一段条件，不是一条语句".to_owned());
    }
    Ok(())
}

pub fn validate_source_sql(source_sql: &str) -> Result<(), String> {
    let normalized = normalize_source_sql(source_sql);
    if normalized.is_empty() {
        return Err("自定义 SQL 不能为空".to_owned());
    }
    if normalized.contains(';') {
        return Err("自定义 SQL 只能包含一条 SELECT 语句".to_owned());
    }
    let first_keyword = normalized.split_whitespace().next().unwrap_or_default();
    if !first_keyword.eq_ignore_ascii_case("SELECT") {
        return Err("自定义 SQL 目前只允许 SELECT 语句".to_owned());
    }
    Ok(())
}

fn normalize_source_sql(source_sql: &str) -> String {
    source_sql
        .trim()
        .strip_suffix(';')
        .map(str::trim_end)
        .unwrap_or_else(|| source_sql.trim())
        .to_owned()
}

/// 列引用的写法。Oracle 把**未加引号**的标识符一律折成大写，于是内层查询若把结果列
/// 别名成了带引号的小写（`SELECT ID AS "id"`），`q.id` 会折成 `Q.ID`——打不中，
/// ORA-00904，且只在真跑的时候才炸。
///
/// **不是全大写才加引号**。全大写是绝大多数情况（Oracle 的默认折叠结果），保持不加引号
/// 意味着既有任务生成的 SQL 文本一字不变；「一律加引号」的正确性与本式完全相同，
/// 却会改动每一条既有任务的生成文本，连带动到运行历史里那份快照的可比性。
///
/// 安全性不靠这里：名字已经过 [`validate_identifier`]，里面不可能有引号或空白。
fn quote_if_folded(identifier: &str) -> String {
    if identifier == identifier.to_ascii_uppercase() {
        identifier.to_owned()
    } else {
        format!("\"{identifier}\"")
    }
}

/// 只收未加引号的 Oracle 标识符。表名、列名这一侧只能靠白名单式校验挡住拼串——
/// **WHERE 片段不在此列**：它按设计就是用户写的一段 SQL，见 [`validate_where_clause`]。
pub(crate) fn validate_identifier(identifier: &str, what: &str) -> Result<(), String> {
    let mut bytes = identifier.bytes();
    let first_is_valid = bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic());
    if first_is_valid
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'#'))
    {
        Ok(())
    } else {
        Err(format!(
            "{what} {identifier:?} 必须是未加引号的 Oracle 标识符"
        ))
    }
}
