//! Guards the webview Content Security Policy in `tauri.conf.json`.
//!
//! The frontend runs with full Tauri IPC access, so the CSP must keep
//! untrusted marketplace/plugin HTML from executing scripts that could
//! invoke secret-returning commands.

use serde_json::Value;

fn csp_value() -> String {
    let conf: Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json parses");
    conf.pointer("/app/security/csp")
        .and_then(Value::as_str)
        .expect("app.security.csp must be a non-null string")
        .to_owned()
}

/// Split the CSP into (directive, value) pairs.
fn directives(csp: &str) -> Vec<(&str, &str)> {
    csp.split(';')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let mut words = part.split_whitespace();
            let name = words.next()?;
            Some((name, part[name.len()..].trim()))
        })
        .collect()
}

fn directive_value<'a>(directives: &'a [(&'a str, &'a str)], name: &str) -> &'a str {
    directives
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| *v)
        .unwrap_or_else(|| panic!("CSP must define {name}"))
}

#[test]
fn csp_disables_inline_and_eval_scripts() {
    let csp = csp_value();
    let dirs = directives(&csp);
    let script_src = directive_value(&dirs, "script-src");
    assert!(
        script_src.contains("'self'"),
        "script-src must allow the app's own scripts, got: {script_src}"
    );
    for forbidden in ["'unsafe-inline'", "'unsafe-eval'"] {
        assert!(
            !script_src.contains(forbidden),
            "script-src must not contain {forbidden}, got: {script_src}"
        );
    }
    // default-src is the fallback for every other fetch directive; keep it tight.
    let default_src = directive_value(&dirs, "default-src");
    assert!(
        default_src.contains("'self'"),
        "default-src must be 'self'-based, got: {default_src}"
    );
    assert!(
        !default_src.contains("'unsafe-inline'") && !default_src.contains("'unsafe-eval'"),
        "default-src must not allow inline/eval, got: {default_src}"
    );
}

#[test]
fn csp_allows_tauri_ipc_but_no_external_network() {
    let csp = csp_value();
    let dirs = directives(&csp);
    let connect_src = directive_value(&dirs, "connect-src");
    assert!(
        connect_src.contains("ipc:") || connect_src.contains("http://ipc.localhost"),
        "connect-src must allow Tauri IPC, got: {connect_src}"
    );
    for forbidden in ["https:", "http:", "wss:", "ws:", "*"] {
        assert!(
            !connect_src
                .split_whitespace()
                .any(|token| token == forbidden),
            "connect-src must not allow external network ({forbidden}), got: {connect_src}"
        );
    }
}

#[test]
fn csp_blocks_plugins_objects_and_framing() {
    let csp = csp_value();
    let dirs = directives(&csp);
    assert_eq!(
        directive_value(&dirs, "object-src"),
        "'none'",
        "object-src must be 'none' (no plugins/embeds)"
    );
    let frame_ancestors = directive_value(&dirs, "frame-ancestors");
    assert!(
        frame_ancestors == "'none'" || frame_ancestors == "'self'",
        "frame-ancestors must deny embedding, got: {frame_ancestors}"
    );
}
