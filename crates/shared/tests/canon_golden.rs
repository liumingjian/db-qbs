use db_qbs_shared::{canon_date, canon_number, canon_text, canon_timestamp, CanonError};
use serde::Deserialize;
use serde_json::Value;

const CANON_FIXTURE: &str = include_str!("../../../docs/spikes/fixtures/canon-golden.json");

#[derive(Debug, Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    #[serde(rename = "type")]
    data_type: String,
    tier: String,
    kind: String,
    verdict: Option<String>,
    input: Value,
    expect: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct TimeParts {
    #[serde(rename = "y")]
    year: i32,
    #[serde(rename = "mo")]
    month: u32,
    #[serde(rename = "d")]
    day: u32,
    #[serde(rename = "h")]
    hour: u32,
    #[serde(rename = "mi")]
    minute: u32,
    #[serde(rename = "s")]
    second: u32,
    #[serde(default, rename = "ns")]
    nanosecond: u32,
}

#[test]
fn all_canonical_cases_match_the_authoritative_fixture() {
    let fixture: Fixture = serde_json::from_str(CANON_FIXTURE).unwrap();

    assert_eq!(fixture.cases.len(), 66);
    assert_eq!(
        fixture
            .cases
            .iter()
            .filter(|case| case.tier == "m1")
            .count(),
        36
    );
    assert_eq!(
        fixture
            .cases
            .iter()
            .filter(|case| case.tier == "m3")
            .count(),
        30
    );

    for case in &fixture.cases {
        check_case(case);
    }
}

fn check_case(case: &Case) {
    let should_reject = case.verdict.as_deref() == Some("reject");

    match (case.data_type.as_str(), case.kind.as_str()) {
        ("NUMBER", "validate") => {
            let input = case.input.as_str().unwrap();
            let result = canon_number(input);
            assert_eq!(result.is_err(), should_reject, "{}", case.id);
            if let Ok(actual) = result {
                assert_eq!(
                    Some(actual),
                    case.expect.as_ref().and_then(Value::as_str),
                    "{}",
                    case.id
                );
            }
        }
        ("DATE", "format") => {
            let parts: TimeParts = serde_json::from_value(case.input.clone()).unwrap();
            let result = canon_date(
                parts.year,
                parts.month,
                parts.day,
                parts.hour,
                parts.minute,
                parts.second,
            );
            assert_eq!(result.is_err(), should_reject, "{}", case.id);
            if let Ok(actual) = result {
                assert_eq!(
                    Some(actual.as_str()),
                    case.expect.as_ref().and_then(Value::as_str),
                    "{}",
                    case.id
                );
            }
        }
        ("TIMESTAMP", "format") => {
            let parts: TimeParts = serde_json::from_value(case.input.clone()).unwrap();
            let result = canon_timestamp(
                parts.year,
                parts.month,
                parts.day,
                parts.hour,
                parts.minute,
                parts.second,
                parts.nanosecond,
            );
            assert_eq!(result.is_err(), should_reject, "{}", case.id);
            if let Ok(actual) = result {
                assert_eq!(
                    Some(actual.as_str()),
                    case.expect.as_ref().and_then(Value::as_str),
                    "{}",
                    case.id
                );
            }
        }
        ("VARCHAR2" | "NVARCHAR2" | "CHAR" | "NCHAR", "passthrough") => {
            let input = case.input.as_str().unwrap();
            assert_eq!(
                Some(canon_text(input)),
                case.expect.as_ref().and_then(Value::as_str),
                "{}",
                case.id
            );
        }
        ("*", "bypass") => assert!(case.input.is_null(), "{}", case.id),
        _ => panic!("unhandled fixture case: {}", case.id),
    }
}

#[test]
fn canon_date_rejects_invalid_calendar_and_time_components() {
    assert!(canon_date(2023, 2, 29, 0, 0, 0).is_err());
    assert!(canon_date(2024, 1, 1, 24, 0, 0).is_err());
}

#[test]
fn canon_timestamp_rejects_invalid_components_with_its_own_error() {
    assert_eq!(
        canon_timestamp(0, 1, 1, 0, 0, 0, 0),
        Err(CanonError::InvalidTimestamp)
    );
    assert_eq!(
        canon_timestamp(2024, 2, 29, 0, 0, 0, 1_000_000_000),
        Err(CanonError::InvalidTimestamp)
    );
    assert_eq!(
        canon_timestamp(2024, 2, 29, 0, 0, 0, 1_500),
        Err(CanonError::InvalidTimestamp)
    );
    assert_eq!(
        canon_timestamp(2023, 2, 29, 0, 0, 0, 0),
        Err(CanonError::InvalidTimestamp)
    );
    assert_eq!(
        canon_timestamp(2024, 1, 1, 24, 0, 0, 0),
        Err(CanonError::InvalidTimestamp)
    );
}
