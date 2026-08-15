use db_qbs_shared::BatchPayload;
use serde_json::{json, Value};

const BATCH_PAYLOAD_FIXTURE: &str =
    include_str!("../../../docs/spikes/fixtures/batch-payload-golden.json");

// The golden fixture's top-level `_comment` / `_source_columns` are human-only annotations
// (ADR-0011: they are not part of the wire message), so strip them before deserializing.
fn wire_view(fixture: &Value) -> Value {
    let mut wire = fixture.as_object().unwrap().clone();
    wire.retain(|key, _| !key.starts_with('_'));
    Value::Object(wire)
}

#[test]
fn golden_payload_round_trips_wire_fields_without_losing_nulls() {
    let fixture: Value = serde_json::from_str(BATCH_PAYLOAD_FIXTURE).unwrap();
    let payload: BatchPayload = serde_json::from_value(wire_view(&fixture)).unwrap();
    let serialized = serde_json::to_value(&payload).unwrap();

    assert_eq!(
        serialized,
        json!({ "seq": fixture["seq"], "rows": fixture["rows"] })
    );
    assert_eq!(payload.rows[0][3].as_deref(), Some(""));
    assert!(payload.rows[0][5].is_none());
}

#[test]
fn unknown_top_level_fields_are_rejected() {
    let candidate = json!({ "seq": 1, "rows": [[null]], "compression": "gzip" });
    assert!(serde_json::from_value::<BatchPayload>(candidate).is_err());
}

#[test]
fn non_string_non_null_cells_are_rejected() {
    for value in [json!(1), json!(true), json!({"value": "1"}), json!(["1"])] {
        let candidate = json!({ "seq": 1, "rows": [[value]] });
        assert!(serde_json::from_value::<BatchPayload>(candidate).is_err());
    }
}
