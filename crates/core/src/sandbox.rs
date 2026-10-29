//! Sandbox mode: talk to the marketplaces' test environments instead of the real ones.
//!
//! Debug builds (`cargo tauri dev`) run in sandbox mode, release builds do not, and the
//! `NISABA_SANDBOX` environment variable (`1` or `0`) overrides either at launch. In
//! sandbox mode one-click sign-in goes through the sandbox broker (eBay's sandbox keyset
//! and Amazon's sandbox app), eBay and Amazon API calls go to their sandbox hosts, and the
//! app keeps its config, catalog and tokens in a separate `nisaba-sandbox` data directory,
//! so test tokens never overwrite real ones.

use std::sync::OnceLock;

/// The sandbox broker compiled into this build (`NISABA_SANDBOX_BROKER_URL` at build time),
/// else Basic Automation's hosted one.
pub fn default_broker_url() -> &'static str {
    option_env!("NISABA_SANDBOX_BROKER_URL")
        .unwrap_or("https://nisaba-auth-sandbox.basicautomation.io")
}

/// Whether this process runs in sandbox mode. Decided once, at first use.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| decide(std::env::var("NISABA_SANDBOX").ok().as_deref()))
}

/// `NISABA_SANDBOX` wins when it is set to a recognised value; otherwise debug builds are
/// sandboxed. This crate's own unit tests default to production so they test real paths.
fn decide(env: Option<&str>) -> bool {
    match env.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if ["1", "true", "yes", "on"].contains(&v.as_str()) => true,
        Some(v) if ["0", "false", "no", "off"].contains(&v.as_str()) => false,
        _ => cfg!(debug_assertions) && !cfg!(test),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_variable_overrides_the_build() {
        assert!(decide(Some("1")));
        assert!(decide(Some(" TRUE ")));
        assert!(!decide(Some("0")));
        assert!(!decide(Some("off")));
        // Unset or unrecognised: the build decides (production under `cargo test`).
        assert!(!decide(None));
        assert!(!decide(Some("maybe")));
    }

    #[test]
    fn the_default_sandbox_broker_is_https() {
        assert!(default_broker_url().starts_with("https://"));
    }
}
