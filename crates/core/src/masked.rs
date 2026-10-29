//! Write-only secrets for the settings UI.
//!
//! No webview may read a stored secret back: the main window renders untrusted
//! marketplace and plugin HTML, so anything an IPC command returns could be read
//! by injected script. Commands that hand config to the UI replace every stored
//! secret with [`MASKED`]; commands that save config put the stored value back
//! wherever the UI returned [`MASKED`] unchanged. An empty string still clears a
//! secret, and any other value replaces it.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::config::AppConfig;

/// Stands in for a stored secret in anything sent to the UI.
pub const MASKED: &str = "••••••••";

/// Secret fields of the per-company platform config (a JSON object).
const COMPANY_SECRET_POINTERS: &[&str] = &[
    "/ebay/client_secret",
    "/squarespace/api_key",
    "/xmrbazaar/password",
    "/amazon/client_secret",
    "/amazon/refresh_token",
];

fn mask(value: &mut String) {
    if !value.is_empty() {
        *value = MASKED.to_string();
    }
}

fn restore(value: &mut String, stored: &str) {
    if value == MASKED {
        *value = stored.to_string();
    }
}

/// A copy of `config` safe to send to the UI.
pub fn mask_app_config(config: &AppConfig) -> AppConfig {
    let mut c = config.clone();
    mask(&mut c.ebay.client_secret);
    mask(&mut c.squarespace.api_key);
    mask(&mut c.xmrbazaar.password);
    mask(&mut c.amazon.client_secret);
    mask(&mut c.amazon.refresh_token);
    c
}

/// Put `stored`'s secrets back wherever `incoming` still carries [`MASKED`].
pub fn restore_app_config(incoming: &mut AppConfig, stored: &AppConfig) {
    restore(&mut incoming.ebay.client_secret, &stored.ebay.client_secret);
    restore(
        &mut incoming.squarespace.api_key,
        &stored.squarespace.api_key,
    );
    restore(&mut incoming.xmrbazaar.password, &stored.xmrbazaar.password);
    restore(
        &mut incoming.amazon.client_secret,
        &stored.amazon.client_secret,
    );
    restore(
        &mut incoming.amazon.refresh_token,
        &stored.amazon.refresh_token,
    );
}

/// Mask the secrets of a per-company platform config in place.
pub fn mask_company_config(config: &mut Value) {
    for pointer in COMPANY_SECRET_POINTERS {
        if let Some(Value::String(s)) = config.pointer_mut(pointer) {
            mask(s);
        }
    }
}

/// Put the stored secrets back wherever `incoming` still carries [`MASKED`].
/// With nothing stored, a [`MASKED`] value becomes empty rather than being saved.
pub fn restore_company_config(incoming: &mut Value, stored: Option<&Value>) {
    for pointer in COMPANY_SECRET_POINTERS {
        let stored_value = stored
            .and_then(|s| s.pointer(pointer))
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(Value::String(s)) = incoming.pointer_mut(pointer) {
            restore(s, stored_value);
        }
    }
}

/// Mask the `secret_keys` of a plugin config map in place.
pub fn mask_plugin_config(config: &mut HashMap<String, String>, secret_keys: &HashSet<String>) {
    for (key, value) in config.iter_mut() {
        if secret_keys.contains(key) {
            mask(value);
        }
    }
}

/// Put the stored values back wherever `incoming` still carries [`MASKED`].
pub fn restore_plugin_config(
    incoming: &mut HashMap<String, String>,
    stored: &HashMap<String, String>,
) {
    for (key, value) in incoming.iter_mut() {
        restore(value, stored.get(key).map(String::as_str).unwrap_or(""));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn company_config_round_trips_through_the_mask() {
        let stored = json!({
            "ebay": { "client_id": "id", "client_secret": "ebay-secret" },
            "squarespace": { "api_key": "" },
            "amazon": { "client_secret": "lwa-secret", "refresh_token": "rt", "seller_id": "S" }
        });
        let mut shown = stored.clone();
        mask_company_config(&mut shown);
        let text = shown.to_string();
        for secret in ["ebay-secret", "lwa-secret", "\"rt\""] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        assert_eq!(shown["amazon"]["seller_id"], "S");
        // An empty secret stays empty, so the UI can tell "not set" from "set".
        assert_eq!(shown["squarespace"]["api_key"], "");

        // Unchanged fields come back as stored; edits and clears win.
        shown["amazon"]["refresh_token"] = json!("");
        shown["ebay"]["client_secret"] = json!("new-secret");
        restore_company_config(&mut shown, Some(&stored));
        assert_eq!(shown["amazon"]["client_secret"], "lwa-secret");
        assert_eq!(shown["amazon"]["refresh_token"], "");
        assert_eq!(shown["ebay"]["client_secret"], "new-secret");
    }

    #[test]
    fn a_mask_with_nothing_stored_is_never_saved() {
        let mut incoming = json!({ "amazon": { "client_secret": MASKED } });
        restore_company_config(&mut incoming, None);
        assert_eq!(incoming["amazon"]["client_secret"], "");
    }

    #[test]
    fn app_config_secrets_are_masked_and_restored() {
        let mut stored: AppConfig = toml::from_str("[general]\n").expect("minimal config");
        stored.ebay.client_secret = "ebay-secret".into();
        stored.xmrbazaar.password = "pw".into();
        stored.ebay.client_id = "visible".into();
        let mut shown = mask_app_config(&stored);
        assert_eq!(shown.ebay.client_secret, MASKED);
        assert_eq!(shown.xmrbazaar.password, MASKED);
        assert_eq!(shown.ebay.client_id, "visible");
        assert_eq!(shown.amazon.client_secret, "");
        restore_app_config(&mut shown, &stored);
        assert_eq!(shown.ebay.client_secret, "ebay-secret");
        assert_eq!(shown.xmrbazaar.password, "pw");
    }

    #[test]
    fn plugin_secrets_are_masked_and_restored() {
        let secret_keys: HashSet<String> = ["token".to_string()].into();
        let stored: HashMap<String, String> =
            [("token".into(), "t0k".into()), ("user".into(), "me".into())].into();
        let mut shown = stored.clone();
        mask_plugin_config(&mut shown, &secret_keys);
        assert_eq!(shown["token"], MASKED);
        assert_eq!(shown["user"], "me");
        restore_plugin_config(&mut shown, &stored);
        assert_eq!(shown, stored);
    }
}
