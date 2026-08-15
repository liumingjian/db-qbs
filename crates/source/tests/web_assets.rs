use db_qbs_source::embedded_web_asset;

#[test]
fn embedded_web_assets_resolve_entrypoint_without_masking_unknown_routes() {
    let index = embedded_web_asset("/").expect("embedded index");
    assert_eq!(index.content_type, "text/html; charset=utf-8");
    assert!(index.body.starts_with(b"<!doctype html>"));

    assert!(embedded_web_asset("/tasks").is_none());
    assert!(embedded_web_asset("/assets/not-built.js").is_none());
}
