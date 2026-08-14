use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::io::{Error as IoError, ErrorKind};

use oracle::sql_type::Timestamp;
use oracle::Connection;
use spike_canon::{DateParts, Gate, GateReport, Sample};

const CANON_FIXTURE: &str = include_str!("../../../canon-golden.json");

fn main() {
    match run() {
        Ok(report) => {
            print_report(&report);
            if !report.passed() {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("TOTAL ERROR: canonical gate could not run: {error}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<GateReport, Box<dyn Error>> {
    let gate = Gate::from_json(CANON_FIXTURE)?;
    let user = env_or("ORACLE_USER", "spike");
    let password = env_or("ORACLE_PASSWORD", "spike123");
    let dsn = env_or("ORACLE_DSN", "//oracle:1521/XE");

    println!("== M1 canonical-form manual gate (issue #43) ==");
    println!("DSN: {dsn}  user: {user}");

    let connection = Connection::connect(&user, &password, &dsn)?;
    print_session_facts(&connection)?;
    let samples = load_samples(&connection)?;

    Ok(gate.evaluate(&samples))
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn print_session_facts(connection: &Connection) -> oracle::Result<()> {
    let numeric_characters: String = connection.query_row_as(
        "SELECT value FROM nls_session_parameters WHERE parameter = 'NLS_NUMERIC_CHARACTERS'",
        &[],
    )?;
    let client_charset: String = connection.query_row_as(
        "SELECT client_charset FROM v$session_connect_info WHERE sid = SYS_CONTEXT('USERENV', 'SID') AND ROWNUM = 1",
        &[],
    )?;
    println!("NLS_NUMERIC_CHARACTERS: {numeric_characters:?}");
    println!("client charset: {client_charset}\n");
    Ok(())
}

fn load_samples(connection: &Connection) -> Result<HashMap<String, Sample>, Box<dyn Error>> {
    let rows = connection.query(
        "SELECT case_id, sample_type, n_value, d_value, v_value \
           FROM t_canon_m1_probe ORDER BY case_id",
        &[],
    )?;
    let mut samples = HashMap::new();

    for row in rows {
        let row = row?;
        let case_id: String = row.get(0)?;
        let sample_type: String = row.get(1)?;
        let number: Option<String> = row.get(2)?;
        let date: Option<Timestamp> = row.get(3)?;
        let text: Option<String> = row.get(4)?;

        let sample = sample_from_row(&case_id, &sample_type, number, date, text)?;
        if samples.insert(case_id.clone(), sample).is_some() {
            return Err(invalid_data(format!(
                "duplicate Oracle sample id {case_id:?}"
            )));
        }
    }

    Ok(samples)
}

fn sample_from_row(
    case_id: &str,
    sample_type: &str,
    number: Option<String>,
    date: Option<Timestamp>,
    text: Option<String>,
) -> Result<Sample, Box<dyn Error>> {
    match (sample_type, number, date, text) {
        ("NUMBER", Some(value), None, None) => Ok(Sample::Number(value)),
        ("DATE", None, Some(value), None) => Ok(Sample::Date(DateParts::new(
            value.year(),
            value.month(),
            value.day(),
            value.hour(),
            value.minute(),
            value.second(),
        ))),
        ("VARCHAR2", None, None, Some(value)) => Ok(Sample::Text(value)),
        ("NULL", None, None, None) => Ok(Sample::Null),
        _ => Err(invalid_data(format!(
            "Oracle sample {case_id:?} has values inconsistent with sample_type {sample_type:?}"
        ))),
    }
}

fn invalid_data(message: String) -> Box<dyn Error> {
    Box::new(IoError::new(ErrorKind::InvalidData, message))
}

fn print_report(report: &GateReport) {
    for result in report.results() {
        println!(
            "[{}] {}: {}",
            if result.passed() { "PASS" } else { "FAIL" },
            result.id(),
            result.detail()
        );
    }

    println!(
        "\nTOTAL {}: PASS {} / FAIL {}",
        if report.passed() { "PASS" } else { "FAIL" },
        report.pass_count(),
        report.fail_count()
    );
}
