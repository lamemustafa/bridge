#[test]
fn renderer_receives_only_native_lifecycle_event_permissions() {
    let capability: serde_json::Value =
        serde_json::from_str(include_str!("../capabilities/default.json"))
            .expect("valid capability JSON");
    assert_eq!(
        capability["permissions"],
        serde_json::json!(["core:event:allow-listen", "core:event:allow-unlisten"])
    );
}

#[test]
fn production_csp_has_no_remote_browser_egress_or_inline_code() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("valid Tauri config");
    let csp = config["app"]["security"]["csp"]
        .as_str()
        .expect("production CSP string");
    assert!(!csp.contains("unsafe-inline"));
    assert!(!csp.contains("https://"));
    assert!(csp.contains("default-src 'none'"));
}
