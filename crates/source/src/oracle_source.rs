use db_qbs_shared::{canon_date, canon_number, canon_text, canon_timestamp};
use oracle::sql_type::ToSql;
use oracle::sql_type::{OracleType, Timestamp};
use oracle::{Connection, InitParams, ResultSet, Row};

use crate::{
    builder_column_query, builder_table_query, classify_column, column_support, BuilderColumn,
    BuilderTable, FailureKind, OracleAccess, RangeCheckColumn, RangeCheckResult, RowSource,
    SourceColumn, SourceReadError, TaskConfig, TaskSpec, FETCH_ARRAY_SIZE,
};

/// 一次查询的绑定变量取值：参数名 → 值。全部值都走绑定（ADR-0011 §2「不发明第二套转义」），
/// 常量条件也不例外，所以它同时承载「写死的常量」与「运行时填的参数」。
type Bindings = Vec<(String, String)>;

fn named_params(bindings: &Bindings) -> Vec<(&str, &dyn ToSql)> {
    bindings
        .iter()
        .map(|(name, value)| (name.as_str(), value as &dyn ToSql))
        .collect()
}

pub struct OracleRowSource {
    rows: ResultSet<'static, Row>,
    columns: Vec<SourceColumn>,
    value_kinds: Vec<ValueKind>,
    access: OracleAccess,
    source_sql: String,
    bindings: Bindings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    Number,
    Date,
    Timestamp,
    Text,
}

impl OracleRowSource {
    pub fn connect(task: &TaskConfig) -> Result<Self, SourceReadError> {
        let source_sql = task.source_sql();
        let bindings = task
            .bindings()
            .map_err(|message| SourceReadError::with_kind(&message, None, FailureKind::Config))?;
        let rows = open_result_set(&task.oracle, &source_sql, &bindings)?;
        let (columns, value_kinds) = describe_columns(rows.column_info());

        Ok(Self {
            rows,
            columns,
            value_kinds,
            access: task.oracle.clone(),
            source_sql,
            bindings,
        })
    }

    /// 取列面：只为把游标开起来拿列信息，绑定变量喂哑值（见 [`TaskSpec::describe_bindings`]）。
    ///
    /// 投影里**结构性只有真列**（`a.C AS C`），所以旧那段「表达式列要抹掉精度再重分类」
    /// 的归一化随生成器一起退役了——构建器根本产不出表达式列（ADR-0036 §5 第 5/6 条）。
    pub fn describe(
        access: &OracleAccess,
        spec: &TaskSpec,
    ) -> Result<Vec<SourceColumn>, SourceReadError> {
        let rows = open_result_set(access, &spec.source_sql(), &spec.describe_bindings())?;
        let (columns, _) = describe_columns(rows.column_info());
        Ok(columns)
    }

    /// 开跑前的一次 `COUNT(*)`：迁移进度那一列的分母（ADR-0043 §7、裁定 6）。
    ///
    /// **把当次真正要执行的语句整个套进子查询**（`SELECT COUNT(*) FROM (<source_sql>)`），
    /// 而不是照 `spec` 另拼一条 `SELECT COUNT(*) FROM 表 WHERE 条件`：
    /// 另拼的那条迟早与生成器漂开，分母就会算的是另一批行。绑定变量用**同一组**，
    /// 常量条件也走绑定（ADR-0011 §2「不发明第二套转义」）。
    ///
    /// 单独开一条连接、用完即关：主游标那条连接上挂着的是取数结果集，
    /// 在它上面再跑一条语句要么排队要么打断取数。代价明码标价——每次发起多一次源端全表计数。
    ///
    /// 失败**不抛给运行**：调用方把它记成「未取到总行数」就继续跑。
    pub fn precount(task: &TaskConfig) -> Result<u64, SourceReadError> {
        let bindings = task
            .bindings()
            .map_err(|message| SourceReadError::with_kind(&message, None, FailureKind::Config))?;
        let query = format!("SELECT COUNT(*) FROM (\n{}\n)", task.source_sql());
        let connection = open_connection(&task.oracle)?;
        let row = connection
            .query_row_named(&query, &named_params(&bindings))
            .map_err(oracle_error)?;
        // Oracle 的 COUNT(*) 回的是 NUMBER，取成 i64 再夹到 0：负数不可能出现，
        // 但夹一下比在 `as u64` 上把 -1 变成天文数字安全。
        let total: i64 = row.get(0).map_err(oracle_error)?;
        Ok(total.max(0) as u64)
    }

    /// 「测试连接」（ADR-0037 §9）：开一条连接、跑一条最便宜的查询，用完即关。
    ///
    /// `SELECT 1 FROM DUAL` 而不是只 `Connection::connect`：登录成功之后仍可能因为
    /// 会话级的东西（NLS、权限）在第一条语句上才炸，那时用户已经以为「连通了」。
    pub fn test_connection(access: &OracleAccess) -> Result<(), SourceReadError> {
        let connection = open_connection(access)?;
        connection
            .query_row("SELECT 1 FROM DUAL", &[])
            .map_err(oracle_error)?;
        Ok(())
    }

    pub fn list_builder_tables(
        access: &OracleAccess,
        dblink: Option<&str>,
    ) -> Result<Vec<BuilderTable>, SourceReadError> {
        let query = builder_table_query(dblink)
            .map_err(|error| SourceReadError::with_kind(error, None, FailureKind::Config))?;
        let connection = open_connection(access)?;
        let rows = connection.query(&query, &[]).map_err(oracle_error)?;
        let mut tables = Vec::new();
        for row in rows {
            let row = row.map_err(oracle_error)?;
            tables.push(BuilderTable {
                owner: row.get(0).map_err(oracle_error)?,
                name: row.get(1).map_err(oracle_error)?,
            });
        }
        Ok(tables)
    }

    pub fn list_builder_columns(
        access: &OracleAccess,
        dblink: Option<&str>,
        owner: &str,
        table: &str,
    ) -> Result<Vec<BuilderColumn>, SourceReadError> {
        let query = builder_column_query(dblink)
            .map_err(|error| SourceReadError::with_kind(error, None, FailureKind::Config))?;
        let connection = open_connection(access)?;
        let rows = connection
            .query(&query, &[&owner, &table])
            .map_err(oracle_error)?;
        let mut columns = Vec::new();
        for row in rows {
            let row = row.map_err(oracle_error)?;
            let length = row
                .get::<usize, Option<i64>>(4)
                .map_err(oracle_error)?
                .and_then(|value| u64::try_from(value).ok());
            let nullable: String = row.get(5).map_err(oracle_error)?;
            columns.push(BuilderColumn {
                name: row.get(0).map_err(oracle_error)?,
                data_type: row.get(1).map_err(oracle_error)?,
                precision: row.get(2).map_err(oracle_error)?,
                scale: row.get(3).map_err(oracle_error)?,
                length,
                nullable: nullable == "Y",
            });
        }
        Ok(columns)
    }

    fn execute_range_check(
        &self,
        columns: &[RangeCheckColumn],
    ) -> Result<(Vec<RangeCheckResult>, u64), SourceReadError> {
        let query = build_range_check_query(&self.source_sql, columns);
        let connection = open_connection(&self.access)?;
        let statement = connection.statement(&query).build().map_err(oracle_error)?;
        let mut rows = statement
            .into_result_set_named(&named_params(&self.bindings))
            .map_err(oracle_error)?;
        let row = rows
            .next()
            .ok_or_else(|| {
                SourceReadError::with_kind(
                    "range check aggregate returned no row",
                    None,
                    FailureKind::Defect,
                )
            })?
            .map_err(oracle_error)?;

        let scanned_rows = aggregate_count(&row, 0, "scanned_rows")?;
        let results = columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                Ok(RangeCheckResult {
                    column: column.column.clone(),
                    invalid_rows: aggregate_count(&row, index + 1, &column.column)?,
                })
            })
            .collect::<Result<Vec<_>, SourceReadError>>()?;
        Ok((results, scanned_rows))
    }
}

fn build_range_check_query(source_sql: &str, columns: &[RangeCheckColumn]) -> String {
    let checks = columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let name = quote_oracle_identifier(&column.column);
            let integer_digits = column.precision.saturating_sub(column.scale);
            format!(
                "NVL(SUM(CASE WHEN {name} IS NOT NULL AND ({name} <> TRUNC({name}, {}) OR ABS({name}) >= POWER(10, {integer_digits})) THEN 1 ELSE 0 END), 0) AS INVALID_{index}",
                column.scale
            )
        })
        .collect::<Vec<_>>();
    let source_sql = source_sql.trim().trim_end_matches(';').trim();
    let checks = if checks.is_empty() {
        String::new()
    } else {
        format!(", {}", checks.join(", "))
    };
    format!("SELECT COUNT(*) AS SCANNED_ROWS{checks} FROM ({source_sql}) RANGE_CHECK_SOURCE")
}

fn quote_oracle_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn aggregate_count(row: &Row, index: usize, label: &str) -> Result<u64, SourceReadError> {
    let value: i64 = row.get(index).map_err(oracle_error)?;
    u64::try_from(value).map_err(|_| {
        SourceReadError::with_kind(
            format!("range check aggregate {label} returned a negative count"),
            None,
            FailureKind::Defect,
        )
    })
}

fn open_result_set(
    access: &OracleAccess,
    source_sql: &str,
    bindings: &Bindings,
) -> Result<ResultSet<'static, Row>, SourceReadError> {
    let connection = open_connection(access)?;
    let statement = connection
        .statement(source_sql)
        .fetch_array_size(FETCH_ARRAY_SIZE)
        .build()
        .map_err(oracle_error)?;
    let rows = statement
        .into_result_set_named(&named_params(bindings))
        .map_err(oracle_error)?;
    Ok(rows)
}

/// **`client_lib_dir` 只有第一次调用生效**：ODPI-C 的 `dpiContext` 全进程唯一，
/// 已初始化后再调 `init()` 返回 `Ok(false)` 且参数被**静默忽略**（ADR-0037 §6 的查证）。
/// 这也正是它不能做成数据源级字段的原因。
fn open_connection(access: &OracleAccess) -> Result<Connection, SourceReadError> {
    std::env::set_var("NLS_LANG", ".AL32UTF8");
    let mut init = InitParams::new();
    init.oracle_client_lib_dir(&access.client_lib_dir)
        .and_then(|params| params.default_driver_name("db-qbs-source : 0.1.0"))
        .and_then(|params| params.init())
        .map_err(oracle_connect_error)?;

    Connection::connect(&access.username, &access.password, &access.connect_string)
        .map_err(oracle_connect_error)
}

fn describe_columns(infos: &[oracle::ColumnInfo]) -> (Vec<SourceColumn>, Vec<ValueKind>) {
    infos
        .iter()
        .map(|info| describe_column(info.name(), info.oracle_type()))
        .unzip()
}

impl RowSource for OracleRowSource {
    fn columns(&self) -> &[SourceColumn] {
        &self.columns
    }

    fn range_check(
        &mut self,
        columns: &[RangeCheckColumn],
    ) -> Result<(Vec<RangeCheckResult>, u64), SourceReadError> {
        self.execute_range_check(columns)
    }

    fn next_row(&mut self) -> Result<Option<Vec<Option<String>>>, SourceReadError> {
        let Some(row) = self.rows.next() else {
            return Ok(None);
        };
        let row = row.map_err(oracle_error)?;
        let mut values = Vec::with_capacity(self.columns.len());

        for (index, kind) in self.value_kinds.iter().enumerate() {
            let column = &self.columns[index].name;
            let value = match kind {
                ValueKind::Number => read_number(&row, index, column)?,
                ValueKind::Date => read_date(&row, index, column)?,
                ValueKind::Timestamp => read_timestamp(&row, index, column)?,
                ValueKind::Text => read_text(&row, index)?,
            };
            values.push(value);
        }

        Ok(Some(values))
    }
}

fn read_number(row: &Row, index: usize, column: &str) -> Result<Option<String>, SourceReadError> {
    let Some(raw) = row
        .get::<usize, Option<String>>(index)
        .map_err(oracle_error)?
    else {
        return Ok(None);
    };

    canon_number(&raw)
        .map(|value| Some(value.to_owned()))
        .map_err(|error| invalid_value(error.to_string(), column, raw))
}

fn read_date(row: &Row, index: usize, column: &str) -> Result<Option<String>, SourceReadError> {
    let Some(timestamp) = row
        .get::<usize, Option<Timestamp>>(index)
        .map_err(oracle_error)?
    else {
        return Ok(None);
    };

    canon_date(
        timestamp.year(),
        timestamp.month(),
        timestamp.day(),
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second(),
    )
    .map(Some)
    .map_err(|error| invalid_value(error.to_string(), column, timestamp.to_string()))
}

fn read_timestamp(
    row: &Row,
    index: usize,
    column: &str,
) -> Result<Option<String>, SourceReadError> {
    let Some(timestamp) = row
        .get::<usize, Option<Timestamp>>(index)
        .map_err(oracle_error)?
    else {
        return Ok(None);
    };

    canon_timestamp(
        timestamp.year(),
        timestamp.month(),
        timestamp.day(),
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second(),
        timestamp.nanosecond(),
    )
    .map(Some)
    .map_err(|error| invalid_value(error.to_string(), column, timestamp.to_string()))
}

fn read_text(row: &Row, index: usize) -> Result<Option<String>, SourceReadError> {
    row.get::<usize, Option<String>>(index)
        .map(|value| value.map(|value| canon_text(&value).to_owned()))
        .map_err(oracle_error)
}

fn invalid_value(message: String, column: &str, value: String) -> SourceReadError {
    SourceReadError {
        message,
        oracle_code: None,
        column: Some(column.to_owned()),
        value: Some(value),
        kind: FailureKind::SourceValue,
    }
}

fn describe_column(name: &str, oracle_type: &OracleType) -> (SourceColumn, ValueKind) {
    // 三档标记不再逐类型手写：形状推导只有一份定义（#125），标记是它的三条出路。
    let (data_type, precision, scale, length, fsp, value_kind) = match oracle_type {
        OracleType::Number(0, _) => (
            "NUMBER".to_owned(),
            None,
            None,
            None,
            None,
            ValueKind::Number,
        ),
        OracleType::Number(precision, scale) => (
            "NUMBER".to_owned(),
            Some(i64::from(*precision)),
            Some(i64::from(*scale)),
            None,
            None,
            ValueKind::Number,
        ),
        OracleType::Varchar2(length) => (
            "VARCHAR2".to_owned(),
            None,
            None,
            Some(u64::from(*length)),
            None,
            ValueKind::Text,
        ),
        OracleType::NVarchar2(length) => (
            "NVARCHAR2".to_owned(),
            None,
            None,
            Some(u64::from(*length)),
            None,
            ValueKind::Text,
        ),
        OracleType::Char(length) => (
            "CHAR".to_owned(),
            None,
            None,
            Some(u64::from(*length)),
            None,
            ValueKind::Text,
        ),
        OracleType::NChar(length) => (
            "NCHAR".to_owned(),
            None,
            None,
            Some(u64::from(*length)),
            None,
            ValueKind::Text,
        ),
        OracleType::Timestamp(fsp) => (
            "TIMESTAMP".to_owned(),
            None,
            None,
            None,
            Some(u32::from(*fsp)),
            ValueKind::Timestamp,
        ),
        OracleType::Date => ("DATE".to_owned(), None, None, None, None, ValueKind::Date),
        other => (other.to_string(), None, None, None, None, ValueKind::Text),
    };

    let mut column = SourceColumn {
        name: name.to_owned(),
        data_type,
        precision,
        scale,
        length,
        fsp,
        support: None,
    };
    column.support = Some(column_support(classify_column(&column)));

    (column, value_kind)
}

/// 会话已经建起来之后撞上的 Oracle 错误：可能是本地查询，也可能是 dblink 那一头。
fn oracle_error(error: oracle::Error) -> SourceReadError {
    SourceReadError::new(error.to_string(), oracle_code(&error))
}

/// 建连接那一步撞上的 Oracle 错误——同样的码在这一步指的是**本地**库。
fn oracle_connect_error(error: oracle::Error) -> SourceReadError {
    SourceReadError::connecting(error.to_string(), oracle_code(&error))
}

fn oracle_code(error: &oracle::Error) -> Option<i32> {
    error.db_error().map(|database_error| database_error.code())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ColumnSupport;

    fn assert_describe(
        oracle_type: OracleType,
        expected_column: SourceColumn,
        expected_kind: ValueKind,
    ) {
        let (column, value_kind) = describe_column("VALUE", &oracle_type);
        assert_eq!(column, expected_column);
        assert_eq!(value_kind, expected_kind);
    }

    fn expected_column(
        data_type: &str,
        precision: Option<i64>,
        scale: Option<i64>,
        length: Option<u64>,
        fsp: Option<u32>,
        support: ColumnSupport,
    ) -> SourceColumn {
        SourceColumn {
            name: "VALUE".to_owned(),
            data_type: data_type.to_owned(),
            precision,
            scale,
            length,
            fsp,
            support: Some(support),
        }
    }

    #[test]
    fn describe_column_reports_all_number_shapes_and_support() {
        for (oracle_type, precision, scale, support) in [
            (
                OracleType::Number(18, 4),
                Some(18),
                Some(4),
                ColumnSupport::Ok,
            ),
            (
                OracleType::Number(4, 6),
                Some(4),
                Some(6),
                ColumnSupport::Ok,
            ),
            (
                OracleType::Number(8, -2),
                Some(8),
                Some(-2),
                ColumnSupport::Ok,
            ),
            (
                OracleType::Number(0, -127),
                None,
                None,
                ColumnSupport::NeedsPrecision,
            ),
            (
                OracleType::Number(38, -27),
                Some(38),
                Some(-27),
                ColumnSupport::Ok,
            ),
            (
                OracleType::Number(38, -28),
                Some(38),
                Some(-28),
                ColumnSupport::Unsupported,
            ),
            (
                OracleType::Number(4, 35),
                Some(4),
                Some(35),
                ColumnSupport::Unsupported,
            ),
        ] {
            assert_describe(
                oracle_type,
                expected_column("NUMBER", precision, scale, None, None, support),
                ValueKind::Number,
            );
        }
    }

    #[test]
    fn describe_column_reports_character_date_and_timestamp_shapes() {
        for (oracle_type, data_type, length) in [
            (OracleType::Varchar2(32), "VARCHAR2", 32),
            (OracleType::NVarchar2(32), "NVARCHAR2", 32),
            (OracleType::Char(32), "CHAR", 32),
            (OracleType::NChar(32), "NCHAR", 32),
        ] {
            assert_describe(
                oracle_type,
                expected_column(data_type, None, None, Some(length), None, ColumnSupport::Ok),
                ValueKind::Text,
            );
        }

        assert_describe(
            OracleType::Date,
            expected_column("DATE", None, None, None, None, ColumnSupport::Ok),
            ValueKind::Date,
        );

        for (fsp, support) in [
            (0, ColumnSupport::Ok),
            (6, ColumnSupport::Ok),
            (7, ColumnSupport::Unsupported),
            (9, ColumnSupport::Unsupported),
        ] {
            assert_describe(
                OracleType::Timestamp(fsp),
                expected_column("TIMESTAMP", None, None, None, Some(u32::from(fsp)), support),
                ValueKind::Timestamp,
            );
        }
    }

    #[test]
    fn describe_column_marks_out_of_scope_types_unsupported() {
        for oracle_type in [
            OracleType::Float(126),
            OracleType::BinaryFloat,
            OracleType::BinaryDouble,
            OracleType::TimestampTZ(6),
            OracleType::TimestampLTZ(6),
        ] {
            let data_type = oracle_type.to_string();
            assert_describe(
                oracle_type,
                expected_column(
                    &data_type,
                    None,
                    None,
                    None,
                    None,
                    ColumnSupport::Unsupported,
                ),
                ValueKind::Text,
            );
        }
    }

    #[test]
    fn timestamp_metadata_serializes_with_fsp_and_support() {
        let (column, _) = describe_column("CREATED_AT", &OracleType::Timestamp(3));
        let wire = serde_json::to_value(column).unwrap();

        assert_eq!(wire["type"], "TIMESTAMP");
        assert_eq!(wire["fsp"], 3);
        assert_eq!(wire["support"], "ok");
    }

    // 表达式列的元数据修正（原 `normalize_expression_metadata`）已随 ADR-0036 §5 删除：
    // 生成器结构性只产 `a.C AS C`，SQL 又不可手改，表达式列在 v1 根本进不来。

    #[test]
    fn timestamp_values_use_fixed_six_digit_canonical_form() {
        assert_eq!(
            canon_timestamp(2026, 8, 17, 14, 35, 9, 120_000_000).unwrap(),
            "2026-08-17 14:35:09.120000"
        );
        assert!(canon_timestamp(2026, 8, 17, 14, 35, 9, 120_000_001).is_err());
    }

    #[test]
    fn range_check_query_scans_once_and_applies_the_full_domain_predicate() {
        let query = build_range_check_query(
            "SELECT N_RAW, N_EXPR FROM source_table WHERE D_BIZ = :biz_date;",
            &[
                RangeCheckColumn {
                    column: "N_RAW".to_owned(),
                    precision: 10,
                    scale: 2,
                },
                RangeCheckColumn {
                    column: "N_EXPR".to_owned(),
                    precision: 18,
                    scale: 4,
                },
            ],
        );

        assert_eq!(query.matches("SUM(CASE").count(), 2);
        assert!(query.contains("COUNT(*) AS SCANNED_ROWS"));
        assert!(query.contains("\"N_RAW\" <> TRUNC(\"N_RAW\", 2)"));
        assert!(query.contains("ABS(\"N_RAW\") >= POWER(10, 8)"));
        assert!(query.contains("\"N_EXPR\" <> TRUNC(\"N_EXPR\", 4)"));
        assert!(query.contains("ABS(\"N_EXPR\") >= POWER(10, 14)"));
        assert!(
            query.contains("FROM (SELECT N_RAW, N_EXPR FROM source_table WHERE D_BIZ = :biz_date)")
        );
        assert!(!query.contains("; )"));
    }
}
