use std::collections::{HashMap, HashSet};

use db_qbs_shared::{canon_date, canon_number, canon_text};
use serde::Deserialize;
use serde_json::Value;

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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DateParts {
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
}

impl DateParts {
    pub fn new(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Self {
        Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sample {
    Number(String),
    Date(DateParts),
    Text(String),
    Null,
}

pub struct Gate {
    cases: Vec<Case>,
}

impl Gate {
    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        let fixture: Fixture = serde_json::from_str(json)?;
        Ok(Self {
            cases: fixture.cases,
        })
    }

    pub fn m1_case_count(&self) -> usize {
        self.m1_cases().count()
    }

    pub fn required_sample_ids(&self) -> Vec<&str> {
        self.m1_cases()
            .filter(|case| case.verdict.as_deref() != Some("reject"))
            .map(|case| case.id.as_str())
            .collect()
    }

    pub fn evaluate(&self, samples: &HashMap<String, Sample>) -> GateReport {
        let mut results: Vec<CaseResult> = self
            .m1_cases()
            .map(|case| evaluate_case(case, samples.get(&case.id)))
            .collect();

        let required: HashSet<&str> = self.required_sample_ids().into_iter().collect();
        let mut extra_ids: Vec<&str> = samples
            .keys()
            .map(String::as_str)
            .filter(|id| !required.contains(id))
            .collect();
        extra_ids.sort_unstable();
        results.extend(
            extra_ids
                .into_iter()
                .map(|id| CaseResult::fail(id, "Oracle sample has no non-reject M1 fixture case")),
        );

        GateReport { results }
    }

    fn m1_cases(&self) -> impl Iterator<Item = &Case> {
        self.cases.iter().filter(|case| case.tier == "m1")
    }
}

fn evaluate_case(case: &Case, sample: Option<&Sample>) -> CaseResult {
    if case.verdict.as_deref() == Some("reject") {
        return evaluate_reject_witness(case);
    }

    let Some(sample) = sample else {
        return CaseResult::fail(&case.id, "required Oracle sample is missing");
    };

    match (case.data_type.as_str(), case.kind.as_str(), sample) {
        ("NUMBER", "validate", Sample::Number(actual)) => match canon_number(actual) {
            Ok(canonical) => compare_string(case, canonical, "driver NUMBER"),
            Err(error) => CaseResult::fail(
                &case.id,
                format!("driver NUMBER {actual:?} rejected: {error}"),
            ),
        },
        ("DATE", "format", Sample::Date(parts)) => match canon_date(
            parts.year,
            parts.month,
            parts.day,
            parts.hour,
            parts.minute,
            parts.second,
        ) {
            Ok(canonical) => compare_string(case, &canonical, "driver DATE"),
            Err(error) => CaseResult::fail(&case.id, format!("driver DATE rejected: {error}")),
        },
        ("VARCHAR2", "passthrough", Sample::Text(actual)) => {
            compare_string(case, canon_text(actual), "driver VARCHAR2")
        }
        ("*", "bypass", Sample::Null) => CaseResult::pass(
            &case.id,
            "Oracle NULL bypassed canon_* and remains JSON null",
        ),
        _ => CaseResult::fail(
            &case.id,
            format!(
                "fixture expects {}/{} but Oracle returned {sample:?}",
                case.data_type, case.kind
            ),
        ),
    }
}

fn evaluate_reject_witness(case: &Case) -> CaseResult {
    let rejected = match (case.data_type.as_str(), case.kind.as_str()) {
        ("NUMBER", "validate") => case
            .input
            .as_str()
            .map(canon_number)
            .map(|result| result.is_err()),
        ("DATE", "format") => serde_json::from_value::<DateParts>(case.input.clone())
            .ok()
            .map(|parts| {
                canon_date(
                    parts.year,
                    parts.month,
                    parts.day,
                    parts.hour,
                    parts.minute,
                    parts.second,
                )
                .is_err()
            }),
        _ => None,
    };

    match rejected {
        Some(true) => CaseResult::pass(
            &case.id,
            "repository reject witness was rejected by the shared canonical function",
        ),
        Some(false) => CaseResult::fail(
            &case.id,
            "repository reject witness was unexpectedly accepted",
        ),
        None => CaseResult::fail(&case.id, "unsupported reject witness shape in fixture"),
    }
}

fn compare_string(case: &Case, actual: &str, source: &str) -> CaseResult {
    match case.expect.as_ref().and_then(Value::as_str) {
        Some(expected) if actual == expected => {
            CaseResult::pass(&case.id, format!("{source} = {actual:?}"))
        }
        Some(expected) => CaseResult::fail(
            &case.id,
            format!("{source} = {actual:?}; fixture expects {expected:?}"),
        ),
        None => CaseResult::fail(&case.id, "fixture has no string expectation"),
    }
}

pub struct GateReport {
    results: Vec<CaseResult>,
}

impl GateReport {
    pub fn results(&self) -> &[CaseResult] {
        &self.results
    }

    pub fn pass_count(&self) -> usize {
        self.results.iter().filter(|result| result.passed).count()
    }

    pub fn fail_count(&self) -> usize {
        self.results.len() - self.pass_count()
    }

    pub fn passed(&self) -> bool {
        self.fail_count() == 0
    }
}

pub struct CaseResult {
    id: String,
    passed: bool,
    detail: String,
}

impl CaseResult {
    fn pass(id: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            passed: true,
            detail: detail.into(),
        }
    }

    fn fail(id: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            passed: false,
            detail: detail.into(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn passed(&self) -> bool {
        self.passed
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}
