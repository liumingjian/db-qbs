use std::collections::HashMap;

use serde_json::Value;
use spike_canon::{DateParts, Gate, Sample};

const CANON_FIXTURE: &str = include_str!("../../../canon-golden.json");

#[test]
fn authoritative_fixture_defines_every_required_oracle_sample() {
    let gate = Gate::from_json(CANON_FIXTURE).unwrap();
    let ids = gate.required_sample_ids();

    assert_eq!(gate.m1_case_count(), 36);
    assert_eq!(ids.len(), 21);
    assert!(ids.contains(&"num-frac"));
    assert!(ids.contains(&"date-leap-century"));
    assert!(ids.contains(&"vc-escape-chars"));
    assert!(ids.contains(&"null-bypass"));
    assert!(!ids.contains(&"num-drift-nls-comma"));
    assert!(!ids.contains(&"date-reject-year-zero"));
}

#[test]
fn all_current_m1_cases_are_evaluated() {
    let gate = Gate::from_json(CANON_FIXTURE).unwrap();
    let fixture: Value = serde_json::from_str(CANON_FIXTURE).unwrap();
    let mut samples = HashMap::new();

    for case in fixture["cases"].as_array().unwrap() {
        if case["tier"] != "m1" || case["verdict"] == "reject" {
            continue;
        }

        let id = case["id"].as_str().unwrap().to_string();
        let sample = match case["type"].as_str().unwrap() {
            "NUMBER" => Sample::Number(case["input"].as_str().unwrap().to_string()),
            "DATE" => {
                let input = &case["input"];
                Sample::Date(DateParts::new(
                    input["y"].as_i64().unwrap() as i32,
                    input["mo"].as_u64().unwrap() as u32,
                    input["d"].as_u64().unwrap() as u32,
                    input["h"].as_u64().unwrap() as u32,
                    input["mi"].as_u64().unwrap() as u32,
                    input["s"].as_u64().unwrap() as u32,
                ))
            }
            "VARCHAR2" => Sample::Text(case["input"].as_str().unwrap().to_string()),
            "*" => Sample::Null,
            data_type => panic!("unexpected M1 fixture type {data_type}"),
        };
        samples.insert(id, sample);
    }

    let report = gate.evaluate(&samples);

    assert_eq!(report.pass_count(), 36);
    assert_eq!(report.fail_count(), 0);
}

#[test]
fn driver_drift_fails_the_named_fixture_case() {
    let gate = Gate::from_json(
        r#"{
          "cases": [{
            "id": "num-frac",
            "type": "NUMBER",
            "tier": "m1",
            "kind": "validate",
            "verdict": "accept",
            "input": "1.23",
            "expect": "1.23"
          }]
        }"#,
    )
    .unwrap();
    let samples = HashMap::from([("num-frac".to_string(), Sample::Number("1,23".to_string()))]);

    let report = gate.evaluate(&samples);

    assert_eq!(report.pass_count(), 0);
    assert_eq!(report.fail_count(), 1);
    assert_eq!(report.results()[0].id(), "num-frac");
    assert!(report.results()[0].detail().contains("1,23"));
}

#[test]
fn oracle_null_uses_the_single_bypass_case() {
    let gate = Gate::from_json(
        r#"{
          "cases": [{
            "id": "null-bypass",
            "type": "*",
            "tier": "m1",
            "kind": "bypass",
            "input": null,
            "expect": null
          }]
        }"#,
    )
    .unwrap();
    let samples = HashMap::from([("null-bypass".to_string(), Sample::Null)]);

    let report = gate.evaluate(&samples);

    assert_eq!(report.pass_count(), 1);
    assert_eq!(report.fail_count(), 0);
    assert!(report.results()[0].detail().contains("JSON null"));
}
