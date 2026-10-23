//! N-11.4: one-button diagnostics export.
//!
//! Builds a redacted support bundle (zip) containing recent logs, app/OS/platform
//! versions, and the app config with every secret value replaced by `[REDACTED]`.
//! This is the 1.0 replacement for any telemetry or crash-reporting service:
//! nothing leaves the machine unless the user attaches the bundle to a bug
//! report themselves.
//!
//! Redaction is two-layered:
//! 1. Key-based: every JSON value sitting under a known secret key
//!    ([`SECRET_KEYS`]) in the config is replaced with `[REDACTED]`.
//! 2. Value-based: the collected secret values are also scrubbed out of the log
//!    text, in case a token ever got logged verbatim. Only values of length >= 4
//!    are scrubbed from logs so short values cannot mangle unrelated text;
//!    short secrets are still redacted by key in the config file itself.

use std::io::Write;

use nisaba_core::config::AppConfig;
use serde_json::Value;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tracing::info;

use crate::log_capture::LogLine;
use crate::state::AppState;

/// Placeholder written over every secret value in the bundle.
pub const REDACTED: &str = "[REDACTED]";

/// Config keys whose values are secrets and must never appear in the bundle.
/// Matched case-insensitively against JSON object keys (see [`redact_value`]).
const SECRET_KEYS: &[&str] = &[
    "client_secret",
    "refresh_token",
    "access_token",
    "api_key",
    "api_secret",
    "password",
];

/// Minimum secret length scrubbed out of free text (logs). Shorter values are
/// still redacted by key in the config file; skipping them in logs avoids
/// mangling unrelated words that happen to contain them.
const MIN_SCRUB_LEN: usize = 4;

fn is_secret_key(key: &str) -> bool {
    SECRET_KEYS.iter().any(|k| k.eq_ignore_ascii_case(key))
}

/// Recursively replace every value under a secret key with `[REDACTED]`.
fn redact_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if is_secret_key(k) {
                    *v = Value::String(REDACTED.to_string());
                } else {
                    redact_value(v);
                }
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                redact_value(v);
            }
        }
        _ => {}
    }
}

/// Recursively collect every non-empty string value sitting under a secret
/// key, so it can also be scrubbed out of free text (logs).
fn collect_secret_values(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if is_secret_key(k) {
                    if let Value::String(s) = v {
                        if !s.is_empty() {
                            out.push(s.clone());
                        }
                    }
                } else {
                    collect_secret_values(v, out);
                }
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_secret_values(v, out);
            }
        }
        _ => {}
    }
}

/// Replace every occurrence of each known secret value in `text` with
/// `[REDACTED]`. Longest values first so overlapping secrets do not partially
/// mask each other.
fn scrub_text(text: &str, secrets: &[String]) -> String {
    let mut ordered: Vec<&str> = secrets.iter().map(String::as_str).collect();
    ordered.sort_by_key(|a| std::cmp::Reverse(a.len()));
    let mut out = text.to_string();
    for secret in ordered {
        if secret.len() >= MIN_SCRUB_LEN {
            out = out.replace(secret, REDACTED);
        }
    }
    out
}

/// Build the diagnostics zip: `versions.json`, `logs.txt`, `config.redacted.json`.
/// Pure function (no Tauri state) so the redaction guarantees are unit-testable.
pub fn build_diagnostics_bundle(
    app_version: &str,
    config: &AppConfig,
    logs: &[LogLine],
) -> Result<Vec<u8>, String> {
    let config_value = serde_json::to_value(config).map_err(|e| e.to_string())?;

    // Collect secrets BEFORE redaction, then redact a clone.
    let mut secrets = Vec::new();
    collect_secret_values(&config_value, &mut secrets);
    let mut redacted = config_value.clone();
    redact_value(&mut redacted);

    let mut log_text = String::new();
    for line in logs {
        log_text.push_str(&format!(
            "[{}] [{}] [{}] {}\n",
            line.timestamp, line.level, line.target, line.message
        ));
    }
    let log_text = scrub_text(&log_text, &secrets);

    // Non-secret platform targeting info: which adapters are on and which
    // environment/region they point at. Anything credential-like stays out.
    let versions = serde_json::json!({
        "app_version": app_version,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "platforms": {
            "ebay": { "enabled": config.ebay.enabled, "environment": config.ebay.environment },
            "amazon": {
                "enabled": config.amazon.enabled,
                "region": config.amazon.region,
                "marketplace_ids": config.amazon.marketplace_ids,
            },
            "squarespace": { "enabled": config.squarespace.enabled },
            "xmrbazaar": { "enabled": config.xmrbazaar.enabled },
        },
    });
    let versions_text = serde_json::to_string_pretty(&versions).map_err(|e| e.to_string())?;
    let config_text = serde_json::to_string_pretty(&redacted).map_err(|e| e.to_string())?;

    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in [
        ("versions.json", versions_text.as_str()),
        ("logs.txt", log_text.as_str()),
        ("config.redacted.json", config_text.as_str()),
    ] {
        writer
            .start_file(name, options)
            .map_err(|e| e.to_string())?;
        writer
            .write_all(content.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let bytes = writer.finish().map_err(|e| e.to_string())?.into_inner();
    Ok(bytes)
}

/// Non-secret accessor for the About page (N-11.4).
///
/// The settings window's About view shows the app version next to the
/// *Export diagnostics* button. A bare version string cannot leak anything,
/// so this is allowed on every window (it is not a secret-returning command).
#[tauri::command]
pub async fn get_app_version(app: AppHandle) -> Result<String, String> {
    Ok(app.package_info().version.to_string())
}

/// Build the redacted diagnostics bundle and let the user save it as a zip.
///
/// The bundle contains recent logs, app/OS/platform versions, and the app
/// config with all secrets redacted. Returns a human-readable summary, or
/// `"Cancelled"` when the user dismisses the save dialog.
#[tauri::command]
pub async fn export_diagnostics(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    let config = state.config.read().await.clone();
    let (lines, _) = state.log_buffer.read_from(0);
    let line_count = lines.len();
    let app_version = app.package_info().version.to_string();

    let zip_bytes = tokio::task::spawn_blocking(move || {
        build_diagnostics_bundle(&app_version, &config, &lines)
    })
    .await
    .map_err(|e| e.to_string())??;

    let dialog = app.dialog().clone();
    let path = tokio::task::spawn_blocking(move || {
        dialog
            .file()
            .set_file_name("nisaba-diagnostics.zip")
            .add_filter("ZIP", &["zip"])
            .blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;

    let Some(path) = path else {
        return Ok("Cancelled".to_string());
    };

    let file_path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&file_path, &zip_bytes).map_err(|e| format!("Failed to write file: {e}"))?;

    let summary =
        format!("Diagnostics bundle saved: {line_count} log lines, redacted config, versions");
    info!("{summary} to {}", file_path.display());

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Read;

    /// Test config carrying known secret values across every platform section.
    /// All `AppConfig` fields have `#[serde(default)]`, so a partial JSON
    /// literal deserializes cleanly.
    fn secret_config() -> AppConfig {
        serde_json::from_value(serde_json::json!({
            "general": {},
            "ebay": {
                "enabled": true,
                "client_id": "TEST-EBAY-CLIENT-ID-001",
                "client_secret": "TEST-EBAY-CLIENT-SECRET-9f8e7d6c",
                "redirect_uri": "https://localhost:8443/ebay/callback",
                "environment": "sandbox"
            },
            "amazon": {
                "enabled": true,
                "client_secret": "TEST-AMAZON-CLIENT-SECRET-z9y8x7",
                "refresh_token": "TEST-AMAZON-REFRESH-TOKEN-a1b2c3",
                "seller_id": "TEST-SELLER-ID-42",
                "region": "NA"
            },
            "squarespace": {
                "enabled": true,
                "api_key": "TEST-SQUARESPACE-API-KEY-qwerty12",
                "store_page_id": "TEST-STORE-PAGE-7"
            },
            "xmrbazaar": {
                "enabled": true,
                "username": "testuser",
                "password": "TEST-XMRBAZAAR-PASSWORD-hunter2x",
                "monero_address": "TEST-MONERO-ADDRESS-44ABC"
            }
        }))
        .expect("test config deserializes")
    }

    fn test_logs() -> Vec<LogLine> {
        vec![
            LogLine {
                timestamp: "2026-10-22T20:00:00".to_string(),
                level: "INFO".to_string(),
                target: "nisaba".to_string(),
                message: "sync started".to_string(),
            },
            // A secret that leaked into a log line must be scrubbed too.
            LogLine {
                timestamp: "2026-10-22T20:01:00".to_string(),
                level: "DEBUG".to_string(),
                target: "nisaba_amazon".to_string(),
                message: "refreshing with TEST-AMAZON-REFRESH-TOKEN-a1b2c3 now".to_string(),
            },
        ]
    }

    fn unzip_to_map(bytes: &[u8]) -> HashMap<String, String> {
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("bundle is a valid zip");
        let mut map = HashMap::new();
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).expect("entry readable");
            let name = file.name().expect("entry name is valid UTF-8").to_string();
            let mut content = String::new();
            file.read_to_string(&mut content)
                .expect("entry is UTF-8 text");
            map.insert(name, content);
        }
        map
    }

    const KNOWN_SECRETS: &[&str] = &[
        "TEST-EBAY-CLIENT-SECRET-9f8e7d6c",
        "TEST-AMAZON-CLIENT-SECRET-z9y8x7",
        "TEST-AMAZON-REFRESH-TOKEN-a1b2c3",
        "TEST-SQUARESPACE-API-KEY-qwerty12",
        "TEST-XMRBAZAAR-PASSWORD-hunter2x",
    ];

    #[test]
    fn bundle_contains_expected_files() {
        let bytes = build_diagnostics_bundle("0.1.1-test", &secret_config(), &test_logs())
            .expect("bundle builds");
        let entries = unzip_to_map(&bytes);
        assert!(entries.contains_key("versions.json"));
        assert!(entries.contains_key("logs.txt"));
        assert!(entries.contains_key("config.redacted.json"));
        assert_eq!(entries.len(), 3);

        let versions: Value =
            serde_json::from_str(&entries["versions.json"]).expect("versions.json parses");
        assert_eq!(versions["app_version"], "0.1.1-test");
        assert!(versions["os"].is_string());
        assert_eq!(versions["platforms"]["ebay"]["environment"], "sandbox");
    }

    #[test]
    fn bundle_contains_no_known_secret_values() {
        let bytes = build_diagnostics_bundle("0.1.1-test", &secret_config(), &test_logs())
            .expect("bundle builds");
        let entries = unzip_to_map(&bytes);
        for (name, content) in &entries {
            for secret in KNOWN_SECRETS {
                assert!(
                    !content.contains(secret),
                    "secret {secret} leaked into bundle entry {name}"
                );
            }
        }
    }

    #[test]
    fn bundle_redacts_secrets_and_keeps_non_secrets() {
        let bytes = build_diagnostics_bundle("0.1.1-test", &secret_config(), &test_logs())
            .expect("bundle builds");
        let entries = unzip_to_map(&bytes);

        let redacted = &entries["config.redacted.json"];
        // Every secret slot is visibly redacted...
        assert!(
            redacted.matches(REDACTED).count() >= KNOWN_SECRETS.len(),
            "expected at least one {REDACTED} marker per secret"
        );
        // ...while non-secret config values survive intact.
        for kept in [
            "TEST-EBAY-CLIENT-ID-001",
            "TEST-SELLER-ID-42",
            "TEST-STORE-PAGE-7",
            "TEST-MONERO-ADDRESS-44ABC",
            "sandbox",
        ] {
            assert!(
                redacted.contains(kept),
                "non-secret value {kept} should survive redaction"
            );
        }

        // The leaked token in the log line is scrubbed; the rest of the line survives.
        let logs = &entries["logs.txt"];
        assert!(logs.contains("sync started"));
        assert!(logs.contains("refreshing with [REDACTED] now"));
    }

    #[test]
    fn empty_secrets_still_build_a_valid_bundle() {
        let config: AppConfig = serde_json::from_value(serde_json::json!({ "general": {} }))
            .expect("empty config deserializes");
        let bytes = build_diagnostics_bundle("0.1.1-test", &config, &[])
            .expect("bundle builds with empty config and no logs");
        let entries = unzip_to_map(&bytes);
        assert_eq!(entries.len(), 3);
        assert!(entries["logs.txt"].is_empty());
    }
}
