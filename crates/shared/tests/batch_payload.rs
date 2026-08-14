use db_qbs_shared::BatchPayload;
use serde_json::{json, Value};

const GOLDEN: &str = include_str!("../../../docs/spikes/fixtures/batch-payload-golden.json");

#[test]
fn golden_payload_round_trips_wire_fields_without_losing_nulls() {
    let fixture: Value = serde_json::from_str(GOLDEN).unwrap();
    let payload: BatchPayload = serde_json::from_value(fixture.clone()).unwrap();
    let serialized = serde_json::to_value(&payload).unwrap();

    assert_eq!(
        serialized,
        json!({ "seq": fixture["seq"], "rows": fixture["rows"] })
    );
    assert!(payload.rows[0][3].as_deref() == Some(""));
    assert!(payload.rows[0][5].is_none());
}

#[test]
fn non_string_non_null_cells_are_rejected() {
    for value in [json!(1), json!(true), json!({"value": "1"}), json!(["1"])] {
        let candidate = json!({ "seq": 1, "rows": [[value]] });
        assert!(serde_json::from_value::<BatchPayload>(candidate).is_err());
    }
}
