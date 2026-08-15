use db_qbs_shared::{canon_date, canon_number, canon_text};
use oracle::sql_type::{OracleType, Timestamp};
use oracle::{Connection, InitParams, ResultSet, Row};

use crate::{
    builder_column_query, builder_table_query, BuilderColumn, BuilderTable, RowSource,
    SourceColumn, SourceConfig, SourceReadError, TaskConfig, FETCH_ARRAY_SIZE,
};

const DESCRIBE_BIZ_DATE: &str = "0001-01-01";

pub struct OracleRowSource {
    rows: ResultSet<'static, Row>,
    columns: Vec<SourceColumn>,
    value_kinds: Vec<ValueKind>,
}

#[derive(Clone, Copy)]
enum ValueKind {
    Number,
    Date,
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
        let query =
            builder_table_query(dblink).map_err(|error| SourceReadError::new(error, None))?;
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
        let query =
            builder_column_query(dblink).map_err(|error| SourceReadError::new(error, None))?;
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
        .map_err(oracle_error)?;

    Connection::connect(
        &config.oracle_username,
        &config.oracle_password,
        &config.oracle_connect_string,
    )
    .map_err(oracle_error)
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
    }
}

fn describe_column(name: &str, oracle_type: &OracleType) -> (SourceColumn, ValueKind) {
    let (data_type, precision, scale, length, value_kind) = match oracle_type {
        OracleType::Number(0, _) => ("NUMBER".to_owned(), None, None, None, ValueKind::Number),
        OracleType::Number(precision, scale) => (
            "NUMBER".to_owned(),
            Some(i64::from(*precision)),
            Some(i64::from(*scale)),
            None,
            ValueKind::Number,
        ),
        OracleType::Varchar2(length) => (
            "VARCHAR2".to_owned(),
            None,
            None,
            Some(u64::from(*length)),
            ValueKind::Text,
        ),
        OracleType::Date => ("DATE".to_owned(), None, None, None, ValueKind::Date),
        other => (other.to_string(), None, None, None, ValueKind::Text),
    };

    (
        SourceColumn {
            name: name.to_owned(),
            data_type,
            precision,
            scale,
            length,
        },
        value_kind,
    )
}

fn oracle_error(error: oracle::Error) -> SourceReadError {
    SourceReadError::new(
        error.to_string(),
        error.db_error().map(|database_error| database_error.code()),
    )
}
