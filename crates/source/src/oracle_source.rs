use db_qbs_shared::{canon_date, canon_number, canon_text, canon_timestamp};
use oracle::sql_type::{OracleType, Timestamp};
use oracle::{Connection, InitParams, ResultSet, Row};

use crate::{
    builder_column_query, builder_table_query, BuilderColumn, BuilderTable, ColumnSupport,
    FailureKind, RowSource, SourceColumn, SourceConfig, SourceReadError, TaskConfig,
    FETCH_ARRAY_SIZE,
};

const DESCRIBE_BIZ_DATE: &str = "0001-01-01";

pub struct OracleRowSource {
    rows: ResultSet<'static, Row>,
    columns: Vec<SourceColumn>,
    value_kinds: Vec<ValueKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    Number,
    Date,
    Timestamp,
    Text,
}

impl OracleRowSource {
    pub fn connect(
        config: &SourceConfig,
        task: &TaskConfig,
        biz_date: &str,
    ) -> Result<Self, SourceReadError> {
        let rows = open_result_set(config, task, biz_date)?;
        let (columns, value_kinds) = describe_columns(rows.column_info());

        Ok(Self {
            rows,
            columns,
            value_kinds,
        })
    }

    pub fn describe(
        config: &SourceConfig,
        task: &TaskConfig,
    ) -> Result<Vec<SourceColumn>, SourceReadError> {
        let rows = open_result_set(config, task, DESCRIBE_BIZ_DATE)?;
        let (columns, _) = describe_columns(rows.column_info());
        Ok(columns)
    }

    pub fn list_builder_tables(
        config: &SourceConfig,
        dblink: Option<&str>,
    ) -> Result<Vec<BuilderTable>, SourceReadError> {
        let query = builder_table_query(dblink)
            .map_err(|error| SourceReadError::with_kind(error, None, FailureKind::Config))?;
        let connection = open_connection(config)?;
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
        config: &SourceConfig,
        dblink: Option<&str>,
        owner: &str,
        table: &str,
    ) -> Result<Vec<BuilderColumn>, SourceReadError> {
        let query = builder_column_query(dblink)
            .map_err(|error| SourceReadError::with_kind(error, None, FailureKind::Config))?;
        let connection = open_connection(config)?;
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
}

fn open_result_set(
    config: &SourceConfig,
    task: &TaskConfig,
    biz_date: &str,
) -> Result<ResultSet<'static, Row>, SourceReadError> {
    let connection = open_connection(config)?;
    let statement = connection
        .statement(&task.source_sql)
        .fetch_array_size(FETCH_ARRAY_SIZE)
        .build()
        .map_err(oracle_error)?;
    let rows = statement
        .into_result_set_named(&[("biz_date", &biz_date)])
        .map_err(oracle_error)?;
    Ok(rows)
}

fn open_connection(config: &SourceConfig) -> Result<Connection, SourceReadError> {
    std::env::set_var("NLS_LANG", ".AL32UTF8");
    let mut init = InitParams::new();
    init.oracle_client_lib_dir(&config.oracle_client_lib_dir)
        .and_then(|params| params.default_driver_name("db-qbs-source : 0.1.0"))
        .and_then(|params| params.init())
        .map_err(oracle_connect_error)?;

    Connection::connect(
        &config.oracle_username,
        &config.oracle_password,
        &config.oracle_connect_string,
    )
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
    let (data_type, precision, scale, length, fsp, support, value_kind) = match oracle_type {
        OracleType::Number(0, _) => (
            "NUMBER".to_owned(),
            None,
            None,
            None,
            None,
            Some(ColumnSupport::NeedsPrecision),
            ValueKind::Number,
        ),
        OracleType::Number(precision, scale) => (
            "NUMBER".to_owned(),
            Some(i64::from(*precision)),
            Some(i64::from(*scale)),
            None,
            None,
            Some(number_support(*precision, *scale)),
            ValueKind::Number,
        ),
        OracleType::Varchar2(length) => (
            "VARCHAR2".to_owned(),
            None,
            None,
            Some(u64::from(*length)),
            None,
            Some(ColumnSupport::Ok),
            ValueKind::Text,
        ),
        OracleType::NVarchar2(length) => (
            "NVARCHAR2".to_owned(),
            None,
            None,
            Some(u64::from(*length)),
            None,
            Some(ColumnSupport::Ok),
            ValueKind::Text,
        ),
        OracleType::Char(length) => (
            "CHAR".to_owned(),
            None,
            None,
            Some(u64::from(*length)),
            None,
            Some(ColumnSupport::Ok),
            ValueKind::Text,
        ),
        OracleType::NChar(length) => (
            "NCHAR".to_owned(),
            None,
            None,
            Some(u64::from(*length)),
            None,
            Some(ColumnSupport::Ok),
            ValueKind::Text,
        ),
        OracleType::Timestamp(fsp) => (
            "TIMESTAMP".to_owned(),
            None,
            None,
            None,
            Some(u32::from(*fsp)),
            Some(if *fsp <= 6 {
                ColumnSupport::Ok
            } else {
                ColumnSupport::Unsupported
            }),
            ValueKind::Timestamp,
        ),
        OracleType::Date => (
            "DATE".to_owned(),
            None,
            None,
            None,
            None,
            Some(ColumnSupport::Ok),
            ValueKind::Date,
        ),
        other => (
            other.to_string(),
            None,
            None,
            None,
            None,
            Some(ColumnSupport::Unsupported),
            ValueKind::Text,
        ),
    };

    (
        SourceColumn {
            name: name.to_owned(),
            data_type,
            precision,
            scale,
            length,
            fsp,
            support,
        },
        value_kind,
    )
}

fn number_support(precision: u8, scale: i8) -> ColumnSupport {
    let precision = i64::from(precision);
    let scale = i64::from(scale);
    let (target_precision, target_scale) = if scale < 0 {
        (precision + scale.abs(), 0)
    } else if scale > precision {
        (scale, scale)
    } else {
        (precision, scale)
    };

    if target_precision <= 65 && target_scale <= 30 {
        ColumnSupport::Ok
    } else {
        ColumnSupport::Unsupported
    }
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

    #[test]
    fn timestamp_values_use_fixed_six_digit_canonical_form() {
        assert_eq!(
            canon_timestamp(2026, 8, 17, 14, 35, 9, 120_000_000).unwrap(),
            "2026-08-17 14:35:09.120000"
        );
        assert!(canon_timestamp(2026, 8, 17, 14, 35, 9, 120_000_001).is_err());
    }
}
