//! 任务定义的**结构化规格**——ADR-0036 §1 判定的唯一真相源。
//!
//! SQL 不进任务定义（ADR-0036 §2），由本模块从规格现算；界面上只读，v1 没有编辑入口。
//! 派生面恰好三样（ADR-0036 §6）：源端 SQL、运行参数清单与取值、报文里来自任务定义的字段。
//! **目标表 DDL 生成与 `OpenRunRequest` 不在其中**——它们吃 describe 回来的源列。
//!
//! 「业务日期」这个一等概念已随 ADR-0035 §3 退役：过滤是普通条件（字段 + 比较符 + 值），
//! 值来源二选一（常量 / 运行时填）。**两种来源都走绑定变量**，理由不是防注入而是转义正确性
//! （ADR-0011 §2「不发明第二套转义」），所以常量也有自己的参数名。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// 运行时逐条填的参数：参数名 → 值。
///
/// 这也是并发互斥键里「本次运行参数集」的规范形式（ADR-0036 §7）：
/// 按参数名排序的 map，值原样取字符串。`BTreeMap` 的序即规范序。
pub type RunParams = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Gt,
    Lt,
    Eq,
}

impl Comparison {
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Gt => ">",
            Self::Lt => "<",
            Self::Eq => "=",
        }
    }
}

/// 条件值怎么绑进 SQL。**不是源列的 Oracle 类型**——它是用户在构建器里就该列做的声明，
/// 建构建器时按所选列的 `DATA_TYPE` 预填，用户可改。
///
/// 存它是必须的：`DATE` 列拿字符串裸比会走 Oracle 的隐式转换、吃 `NLS_DATE_FORMAT`，
/// 换个会话就换个语义。而按 ADR-0036 §6，describe 回来的类型**不许进任务定义**，
/// 所以这个声明只能是用户的选择，不能是缓存下来的元数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    Text,
    Number,
    Date,
}

impl ValueType {
    /// 绑定变量在 SQL 里的写法。
    fn render(self, parameter: &str) -> String {
        match self {
            Self::Text => format!(":{parameter}"),
            Self::Number => format!("TO_NUMBER(:{parameter})"),
            Self::Date => format!("TO_DATE(:{parameter},'YYYY-MM-DD')"),
        }
    }

    /// describe 时要执行一次查询，给每个绑定变量喂一个类型上说得通的哑值。
    fn describe_placeholder(self) -> &'static str {
        match self {
            Self::Text => "",
            Self::Number => "0",
            Self::Date => "1970-01-01",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueSource {
    /// 建任务时写死。
    Constant,
    /// 发起运行时逐条填。
    Runtime,
}

/// 一条过滤条件：字段 + 比较符 + 值。
///
/// 第一版只做这一种最简形态：不做 `IN` / `BETWEEN` / `LIKE` / 表达式，
/// 也不做 `sysdate-1` 这类相对表达式（ADR-0004 的禁令由「值是绑定变量」结构性保证）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub column: String,
    pub operator: Comparison,
    pub value_type: ValueType,
    /// 绑定变量名，规格内唯一。运行时填的条件用它做运行参数的键，
    /// 所以它**必须由用户拥有并且稳定**——按序号自动编号会让增删一条参数
    /// 把此前所有历史里的键都对不上（ADR-0036 §7 否掉顺序串接是同一个理由）。
    pub parameter: String,
    pub value_source: ValueSource,
    /// 仅 [`ValueSource::Constant`] 时有意义；运行时填的条件必须留空。
    #[serde(default)]
    pub constant: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Asc,
    Desc,
}

impl Direction {
    fn as_sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderTerm {
    pub column: String,
    pub direction: Direction,
}

/// 一列的映射：源列名 → 目标列名（ADR-0038 §2）。
///
/// 「默认同名映射」就是 `target` 预填成 `source`——那不是将就的默认值，它就是恒等映射的
/// 正确表达（ADR-0009 增补第 5 条：别名即目标列名），与改形状之前的语义逐字一致。
///
/// 落到 SQL 上是投影的别名 `a.{source} AS {target}`，**不引入映射表**：目标列名本来就是
/// `SELECT` 投影的别名，在协议层再表达一遍等于让两端各留一份判定式（ADR-0038 §1）。
/// 因此报文里仍然只有一份列名——目标名，协议不增字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnMapping {
    pub source: String,
    pub target: String,
}

/// 任务定义的全部内容。旧的四字段形态（ADR-0016 §2）已由 ADR-0036 §4 判定直接丢弃。
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
    /// upsert 去重用的主键列，必选（所有者 2026-08-18 裁定）。存的是**目标列名**
    /// （ADR-0038 §2/§6）——与 [`ColumnMapping::target`]、过线报文、sink 侧比对同一个名字空间。
    /// 目标端是否真有对应约束由 sink 侧预检核对（ADR-0035 §2）——这里只记用户选了什么。
    pub primary_key: Vec<String>,
    // 下面三个是 TOML 里的 array-of-tables，**必须排在所有标量之后**：
    // 临时任务定义走 TOML 落盘，值排在表之后会直接序列化失败。
    // `columns` 自 ADR-0038 §2 换成结构之后也归这一类，所以它排到了 `primary_key` 之后。
    /// 选中的列及其目标字段（ADR-0038 §2）。
    pub columns: Vec<ColumnMapping>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub order_by: Vec<OrderTerm>,
}

impl TaskSpec {
    /// 运行时要逐条填的条件，按参数名排序——发起面照这个顺序列，历史里也照这个顺序展示。
    pub fn runtime_parameters(&self) -> Vec<&Condition> {
        let mut parameters = self
            .conditions
            .iter()
            .filter(|condition| condition.value_source == ValueSource::Runtime)
            .collect::<Vec<_>>();
        parameters.sort_by(|left, right| left.parameter.cmp(&right.parameter));
        parameters
    }

    /// 全部绑定变量的取值：常量取规格里写死的那份，运行时填的取本次运行参数。
    pub fn bindings(&self, run_params: &RunParams) -> Result<Vec<(String, String)>, String> {
        self.conditions
            .iter()
            .map(|condition| {
                let value = match condition.value_source {
                    ValueSource::Constant => condition.constant.clone(),
                    ValueSource::Runtime => run_params
                        .get(&condition.parameter)
                        .cloned()
                        .ok_or_else(|| format!("运行参数 {} 未取值", condition.parameter))?,
                };
                Ok((condition.parameter.clone(), value))
            })
            .collect()
    }

    /// describe 用的绑定：只为把游标开起来拿列信息，值本身没有意义。
    pub fn describe_bindings(&self) -> Vec<(String, String)> {
        self.conditions
            .iter()
            .map(|condition| {
                (
                    condition.parameter.clone(),
                    condition.value_type.describe_placeholder().to_owned(),
                )
            })
            .collect()
    }

    /// 源端 SQL。一条条件都没有时就是整表取数——量级风险归 #122 去证，不在这里挡。
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
        for (index, condition) in self.conditions.iter().enumerate() {
            let keyword = if index == 0 { " WHERE" } else { "   AND" };
            sql.push_str(&format!(
                "\n{keyword} a.{} {} {}",
                condition.column,
                condition.operator.as_sql(),
                condition.value_type.render(&condition.parameter)
            ));
        }
        if !self.order_by.is_empty() {
            let terms = self
                .order_by
                .iter()
                .map(|term| format!("a.{} {}", term.column, term.direction.as_sql()))
                .collect::<Vec<_>>()
                .join(", ");
            sql.push_str(&format!("\n ORDER BY {terms}"));
        }
        sql
    }

    /// 投影就是映射：`别名.源列 AS 目标字段`。按表选择与自定义 SQL 共用这一份，
    /// 两条路径生成的形状因此不会漂。
    fn projection(&self, alias: &str) -> String {
        self.columns
            .iter()
            .map(|mapping| format!("{alias}.{} AS {}", mapping.source, mapping.target))
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
            if !self.conditions.is_empty() || !self.order_by.is_empty() {
                return Err("自定义 SQL 模式不能再配置过滤条件或排序，请直接写入 SQL".to_owned());
            }
        } else {
            validate_identifier(&self.owner, "owner")?;
            validate_identifier(&self.table, "table")?;
            if let Some(dblink) = &self.dblink {
                validate_identifier(dblink, "dblink")?;
            }
        }
        if self.target_table.trim().is_empty() {
            return Err("target_table 不能为空".to_owned());
        }
        if self.columns.is_empty() {
            return Err("至少要选一列".to_owned());
        }
        // 两个名字空间各查一次重复：源列重复是「勾了两遍」，目标字段重复会生成两个同名别名，
        // Oracle 不拒、sink 侧按名字对齐时后一列静默盖掉前一列——那正是 ADR-0038 §1 说的
        // 「最难排的错」，在源端就能判，所以在源端判。
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
        if self.primary_key.is_empty() {
            return Err("主键必选：至少要勾一列作为 upsert 的去重键".to_owned());
        }
        let mut key_columns = BTreeSet::new();
        for column in &self.primary_key {
            validate_identifier(column, "primary_key")?;
            let normalized = column.to_ascii_uppercase();
            if !key_columns.insert(normalized.clone()) {
                return Err(format!("主键列 {column} 重复"));
            }
            // 主键列必须落在选中的列里（ADR-0035 §2 第 2 条）。比的是**目标字段**——
            // `primary_key` 存目标名（ADR-0038 §6），拿它去比源列名会在改过名的列上误判。
            // 目标端约束是否真的存在由 sink 侧核对，这一条在源端就能判，所以在源端判。
            if !targets.contains(&normalized) {
                return Err(format!("主键列 {column} 不在选中的列里"));
            }
        }
        let mut parameters = BTreeSet::new();
        for condition in &self.conditions {
            validate_identifier(&condition.column, "condition column")?;
            validate_identifier(&condition.parameter, "parameter")?;
            if !parameters.insert(condition.parameter.to_ascii_lowercase()) {
                return Err(format!("参数名 {} 重复", condition.parameter));
            }
            match condition.value_source {
                ValueSource::Constant if condition.constant.is_empty() => {
                    return Err(format!("条件 {} 的常量值不能为空", condition.parameter));
                }
                ValueSource::Runtime if !condition.constant.is_empty() => {
                    return Err(format!(
                        "条件 {} 标了运行时填，不能同时写死常量值",
                        condition.parameter
                    ));
                }
                _ => {}
            }
        }
        for term in &self.order_by {
            validate_identifier(&term.column, "order by column")?;
        }
        Ok(())
    }
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

/// 只收未加引号的 Oracle 标识符。这是**唯一**挡住把标识符拼进 SQL 的东西——
/// 值走绑定变量，标识符不能，所以标识符这一侧只能靠白名单式校验。
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
