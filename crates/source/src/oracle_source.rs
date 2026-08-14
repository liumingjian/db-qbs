use db_qbs_shared::{canon_date, canon_number, canon_text};
use oracle::sql_type::{OracleType, Timestamp};
use oracle::{Connection, InitParams, ResultSet, Row};

use crate::{RowSource, SourceColumn, SourceConfig, SourceReadError, TaskConfig, FETCH_ARRAY_SIZE};

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
        std::env::set_var("NLS_LANG", ".AL32UTF8");
        let mut init = InitParams::new();
        init.oracle_client_lib_dir(&config.oracle_client_lib_dir)
            .and_then(|params| params.default_driver_name("db-qbs-source : 0.1.0"))
            .and_then(|params| params.init())
            .map_err(oracle_error)?;

        let connection = Connection::connect(
            &config.oracle_username,
            &config.oracle_password,
            &config.oracle_connect_string,
        )
        .map_err(oracle_error)?;
        let statement = connection
            .statement(&task.source_sql)
            .fetch_array_size(FETCH_ARRAY_SIZE)
            .build()
            .map_err(oracle_error)?;
        let rows = statement
            .into_result_set_named(&[("biz_date", &biz_date)])
            .map_err(oracle_error)?;

        let mut columns = Vec::with_capacity(rows.column_info().len());
        let mut value_kinds = Vec::with_capacity(rows.column_info().len());
        for info in rows.column_info() {
            let (column, value_kind) = describe_column(info.name(), info.oracle_type());
            columns.push(column);
            value_kinds.push(value_kind);
        }

        Ok(Self {
            rows,
            columns,
            value_kinds,
        })
    }
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
