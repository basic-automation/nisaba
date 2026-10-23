use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use nisaba_amazon::AmazonAdapter;
use nisaba_core::types::Platform;
use nisaba_ebay::EbayAdapter;
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tracing::{error, info};

use crate::state::AppState;

#[tauri::command]
pub async fn start_ebay_auth(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters
        .get(&Platform::Ebay)
        .ok_or("eBay adapter is not enabled")?;

    let ebay = adapter
        .as_any()
        .downcast_ref::<EbayAdapter>()
        .ok_or("Failed to get eBay adapter")?;

    let auth_url = ebay.authorization_url();
    let adapter_clone = adapter.clone();
    drop(adapters);

    // Close existing auth window if still open
    if let Some(existing) = app.get_webview_window("ebay-auth") {
        let _ = existing.close();
    }

    let url_parsed: url::Url = auth_url
        .parse()
        .map_err(|e: url::ParseError| format!("Invalid URL: {e}"))?;

    let handled = Arc::new(AtomicBool::new(false));
    let handled_nav = handled.clone();
    let app_nav = app.clone();

    let _auth_window =
        WebviewWindowBuilder::new(&app, "ebay-auth", WebviewUrl::External(url_parsed))
            .title("eBay Authorization")
            .inner_size(900.0, 700.0)
            .center()
            .on_navigation(move |url| {
                if handled_nav.load(Ordering::SeqCst) {
                    return false;
                }

                if let Some(code) = extract_code_from_url(url) {
                    handled_nav.store(true, Ordering::SeqCst);
                    // Bind this code to the authorization attempt we issued:
                    // eBay must echo the `state` we put in the authorize URL.
                    let returned_state = extract_state_from_url(url);
                    let adapter = adapter_clone.clone();
                    let app = app_nav.clone();

                    tauri::async_runtime::spawn(async move {
                        if let Some(ebay) = adapter.as_any().downcast_ref::<EbayAdapter>() {
                            match ebay.complete_auth(&code, returned_state.as_deref()).await {
                                Ok(()) => {
                                    info!("eBay OAuth completed via auth window");
                                    let _ = app.emit("ebay-auth-complete", true);
                                }
                                Err(e) => {
                                    error!("eBay OAuth failed: {e}");
                                    let _ = app.emit("ebay-auth-error", e.to_string());
                                }
                            }
                        }
                        if let Some(w) = app.get_webview_window("ebay-auth") {
                            let _ = w.close();
                        }
                    });

                    return false;
                }

                if url.as_str().contains("isAuthSuccessful=false") {
                    handled_nav.store(true, Ordering::SeqCst);
                    let app = app_nav.clone();
                    let _ = app.emit("ebay-auth-error", "Authorization was declined");
                    if let Some(w) = app.get_webview_window("ebay-auth") {
                        let _ = w.close();
                    }
                    return false;
                }

                true
            })
            .build()
            .map_err(|e| format!("Failed to create auth window: {e}"))?;

    Ok(())
}

#[tauri::command]
pub async fn get_ebay_auth_url(state: State<'_, AppState>) -> Result<String, String> {
    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters
        .get(&Platform::Ebay)
        .ok_or("eBay adapter is not enabled")?;

    let ebay = adapter
        .as_any()
        .downcast_ref::<EbayAdapter>()
        .ok_or("Failed to get eBay adapter")?;

    Ok(ebay.authorization_url())
}

#[tauri::command]
pub async fn complete_ebay_auth(
    state: State<'_, AppState>,
    code: String,
    returned_state: Option<String>,
) -> Result<(), String> {
    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters
        .get(&Platform::Ebay)
        .ok_or("eBay adapter is not enabled")?;

    let ebay = adapter
        .as_any()
        .downcast_ref::<EbayAdapter>()
        .ok_or("Failed to get eBay adapter")?;

    ebay.complete_auth(&code, returned_state.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn start_amazon_auth(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters
        .get(&Platform::Amazon)
        .ok_or("Amazon adapter is not enabled")?;

    let amazon = adapter
        .as_any()
        .downcast_ref::<AmazonAdapter>()
        .ok_or("Failed to get Amazon adapter")?;

    let auth_url = amazon.authorization_url();
    let adapter_clone = adapter.clone();
    drop(adapters);

    // Close existing auth window if still open
    if let Some(existing) = app.get_webview_window("amazon-auth") {
        let _ = existing.close();
    }

    let url_parsed: url::Url = auth_url
        .parse()
        .map_err(|e: url::ParseError| format!("Invalid URL: {e}"))?;

    let handled = Arc::new(AtomicBool::new(false));
    let handled_nav = handled.clone();
    let app_nav = app.clone();

    let _auth_window =
        WebviewWindowBuilder::new(&app, "amazon-auth", WebviewUrl::External(url_parsed))
            .title("Amazon Authorization")
            .inner_size(900.0, 700.0)
            .center()
            .on_navigation(move |url| {
                if handled_nav.load(Ordering::SeqCst) {
                    return false;
                }

                // Amazon returns spapi_oauth_code in the redirect URL
                if let Some(code) = extract_amazon_code_from_url(url) {
                    handled_nav.store(true, Ordering::SeqCst);
                    let adapter = adapter_clone.clone();
                    let app = app_nav.clone();

                    tauri::async_runtime::spawn(async move {
                        if let Some(amazon) = adapter.as_any().downcast_ref::<AmazonAdapter>() {
                            match amazon.complete_auth(&code).await {
                                Ok(()) => {
                                    info!("Amazon OAuth completed via auth window");
                                    let _ = app.emit("amazon-auth-complete", true);
                                }
                                Err(e) => {
                                    error!("Amazon OAuth failed: {e}");
                                    let _ = app.emit("amazon-auth-error", e.to_string());
                                }
                            }
                        }
                        if let Some(w) = app.get_webview_window("amazon-auth") {
                            let _ = w.close();
                        }
                    });

                    return false;
                }

                true
            })
            .build()
            .map_err(|e| format!("Failed to create auth window: {e}"))?;

    Ok(())
}

#[tauri::command]
pub async fn complete_amazon_auth(state: State<'_, AppState>, code: String) -> Result<(), String> {
    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters
        .get(&Platform::Amazon)
        .ok_or("Amazon adapter is not enabled")?;

    let amazon = adapter
        .as_any()
        .downcast_ref::<AmazonAdapter>()
        .ok_or("Failed to get Amazon adapter")?;

    amazon.complete_auth(&code).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn check_platform_auth(
    state: State<'_, AppState>,
    platform: String,
) -> Result<bool, String> {
    let plat = Platform::from_str_loose(&platform)
        .ok_or_else(|| format!("Unknown platform: {platform}"))?;

    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    if let Some(adapter) = adapters.get(&plat) {
        Ok(adapter.is_authenticated().await)
    } else {
        Ok(false)
    }
}

/// N-11.1: probe a platform with a minimal read-only request and return a
/// human-readable summary. Used by the setup wizard's per-platform "Test
/// connection" button. The adapters are built from the saved company config,
/// so the wizard stage-saves the platform's fields before invoking this.
#[tauri::command]
pub async fn test_platform_connection(
    state: State<'_, AppState>,
    platform: String,
) -> Result<String, String> {
    let plat = Platform::from_str_loose(&platform)
        .ok_or_else(|| format!("Unknown platform: {platform}"))?;

    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters.get(&plat).ok_or_else(|| {
        format!(
            "{} is not enabled — save the platform's credentials first, then test the connection",
            plat.as_str()
        )
    })?;

    adapter.test_connection().await.map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
pub struct PlatformHealth {
    pub platform: String,
    pub enabled: bool,
    pub authenticated: bool,
}

/// N-11.2: per-platform credential-health state for the UI — when the token
/// or keys expire, with an advance warning, so the user can renew in the
/// setup wizard *before* a mid-sync refresh fails.
///
/// This command returns expiry timestamps only, never secrets, so it is
/// allowed on the main window (see `acl_check`).
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialHealthStatus {
    /// Credential valid, no action needed.
    Ok,
    /// Expiry (or rotation deadline) approaching — warn ahead of time.
    ExpiringSoon,
    /// Past expiry — sync will fail until renewed.
    Expired,
    /// Needs attention but has no deadline (e.g. never connected, or the
    /// expiry has not been recorded yet).
    Attention,
}

#[derive(serde::Serialize)]
pub struct CredentialHealth {
    pub platform: String,
    pub enabled: bool,
    pub status: CredentialHealthStatus,
    pub access_token_expires_at: Option<String>,
    pub refresh_token_expires_at: Option<String>,
    /// Human-readable summary for the UI, e.g. "Refresh token expires in
    /// 12 days (2027-04-18 10:00:00)".
    pub detail: String,
}

/// N-11.2: warn this many days before a refresh credential expires.
const REFRESH_TOKEN_WARN_DAYS: i64 = 30;
/// N-11.2: Amazon LWA client secrets have no API-exposed expiry; Amazon
/// expects periodic rotation, so warn once the recorded secret is older
/// than this.
const AMAZON_CLIENT_SECRET_ROTATION_DAYS: i64 = 365;

/// Parse the `YYYY-MM-DD HH:MM:SS` UTC timestamps the adapters persist in
/// `auth_tokens`.
fn parse_db_timestamp(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|naive| chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(naive, chrono::Utc))
}

fn days_until(expiry: chrono::DateTime<chrono::Utc>) -> i64 {
    (expiry - chrono::Utc::now()).num_days()
}

#[tauri::command]
pub async fn get_credential_health(
    state: State<'_, AppState>,
) -> Result<Vec<CredentialHealth>, String> {
    let all_platforms = [
        Platform::Squarespace,
        Platform::Ebay,
        Platform::XmrBazaar,
        Platform::Amazon,
    ];
    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let db = state.active_db().await?;

    // Amazon's client-secret rotation timestamp lives in the (encrypted)
    // company config. It is read backend-side only; the command returns
    // derived status text, never the secret.
    let amazon_secret_updated_at: Option<String> = super::config::load_company_config_value(&state)
        .await
        .and_then(|v| {
            v.pointer("/amazon/client_secret_updated_at")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string())
        });

    let mut results = Vec::new();
    for plat in &all_platforms {
        let enabled = adapters.contains_key(plat);
        let token_meta = db
            .get_auth_token_expiry(plat.as_str())
            .await
            .map_err(|e| e.to_string())?;

        let (status, detail) = match plat {
            Platform::Ebay => ebay_credential_status(token_meta.as_ref()),
            Platform::Amazon => {
                amazon_credential_status(token_meta.as_ref(), amazon_secret_updated_at.as_deref())
            }
            Platform::Squarespace => (
                CredentialHealthStatus::Ok,
                "API-key auth — the key does not expire.".to_string(),
            ),
            Platform::XmrBazaar => (
                CredentialHealthStatus::Ok,
                "Username/password auth — no expiry to track.".to_string(),
            ),
        };

        results.push(CredentialHealth {
            platform: plat.as_str().to_string(),
            enabled,
            status,
            access_token_expires_at: token_meta.as_ref().and_then(|m| m.expires_at.clone()),
            refresh_token_expires_at: token_meta
                .as_ref()
                .and_then(|m| m.refresh_token_expires_at.clone()),
            detail,
        });
    }

    Ok(results)
}

/// N-11.2: eBay credential state. eBay's refresh token is the credential
/// with a real expiry (18 months, from `refresh_token_expires_in`); the
/// access token auto-refreshes, so its timestamp is informational only.
fn ebay_credential_status(
    meta: Option<&nisaba_core::types::AuthTokenExpiry>,
) -> (CredentialHealthStatus, String) {
    let Some(meta) = meta else {
        return (
            CredentialHealthStatus::Attention,
            "Not connected — complete eBay authorization in the setup wizard.".to_string(),
        );
    };
    let Some(refresh_at) = meta.refresh_token_expires_at.as_deref() else {
        return (
            CredentialHealthStatus::Attention,
            "Connected, but the refresh-token expiry has not been recorded yet — \
             it is tracked from the next authorization or token refresh."
                .to_string(),
        );
    };
    let Some(expiry) = parse_db_timestamp(refresh_at) else {
        return (
            CredentialHealthStatus::Attention,
            format!("Connected, but the stored refresh-token expiry ({refresh_at}) could not be parsed."),
        );
    };
    let days = days_until(expiry);
    if days < 0 {
        (
            CredentialHealthStatus::Expired,
            format!(
                "Refresh token expired on {refresh_at} — re-authorize eBay in the setup \
                 wizard to keep syncing."
            ),
        )
    } else if days <= REFRESH_TOKEN_WARN_DAYS {
        (
            CredentialHealthStatus::ExpiringSoon,
            format!(
                "Refresh token expires in {days} day{s} ({refresh_at}) — renew it in the \
                 setup wizard before it lapses.",
                s = if days == 1 { "" } else { "s" }
            ),
        )
    } else {
        (
            CredentialHealthStatus::Ok,
            format!("Refresh token valid until {refresh_at}."),
        )
    }
}

/// N-11.2: Amazon credential state. LWA refresh tokens do not expire; the
/// credential with a deadline is the LWA client secret, which Amazon
/// expects the developer to rotate periodically. The UI tracks the last
/// rotation from `amazon.client_secret_updated_at` in the company config.
fn amazon_credential_status(
    meta: Option<&nisaba_core::types::AuthTokenExpiry>,
    secret_updated_at: Option<&str>,
) -> (CredentialHealthStatus, String) {
    let connected = meta.is_some();
    match secret_updated_at {
        Some(ts) => {
            let age_days = chrono::DateTime::parse_from_rfc3339(ts)
                .ok()
                .map(|dt| (chrono::Utc::now() - dt.with_timezone(&chrono::Utc)).num_days());
            match age_days {
                Some(age) if age > AMAZON_CLIENT_SECRET_ROTATION_DAYS => (
                    CredentialHealthStatus::ExpiringSoon,
                    format!(
                        "LWA client secret is {age} days old — rotate it in the Amazon \
                         Developer Console and re-enter it in the setup wizard."
                    ),
                ),
                _ => (
                    CredentialHealthStatus::Ok,
                    format!("LWA client secret last updated {ts}."),
                ),
            }
        }
        None if connected => (
            CredentialHealthStatus::Ok,
            "Connected. The client-secret rotation date is not recorded yet — it is \
             tracked from the next settings save."
                .to_string(),
        ),
        None => (
            CredentialHealthStatus::Attention,
            "Not connected — complete Amazon authorization in the setup wizard.".to_string(),
        ),
    }
}

#[tauri::command]
pub async fn get_platform_health(
    state: State<'_, AppState>,
) -> Result<Vec<PlatformHealth>, String> {
    let all_platforms = [
        Platform::Squarespace,
        Platform::Ebay,
        Platform::XmrBazaar,
        Platform::Amazon,
    ];
    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;

    let mut results = Vec::new();
    for plat in &all_platforms {
        let (enabled, authenticated) = if let Some(adapter) = adapters.get(plat) {
            (true, adapter.is_authenticated().await)
        } else {
            (false, false)
        };
        results.push(PlatformHealth {
            platform: plat.as_str().to_string(),
            enabled,
            authenticated,
        });
    }

    Ok(results)
}

fn extract_code_from_url(url: &url::Url) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())
}

fn extract_state_from_url(url: &url::Url) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
}

fn extract_amazon_code_from_url(url: &url::Url) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == "spapi_oauth_code")
        .map(|(_, value)| value.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nisaba_core::types::AuthTokenExpiry;

    fn meta(refresh_at: Option<&str>) -> AuthTokenExpiry {
        AuthTokenExpiry {
            expires_at: None,
            refresh_token_expires_at: refresh_at.map(|s| s.to_string()),
        }
    }

    fn db_timestamp(days_from_now: i64) -> String {
        (chrono::Utc::now() + chrono::Duration::days(days_from_now))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }

    /// N-11.2: a refresh token past its expiry is `expired`.
    #[test]
    fn ebay_status_expired_in_the_past() {
        let m = meta(Some(&db_timestamp(-1)));
        let (status, detail) = ebay_credential_status(Some(&m));
        assert!(matches!(status, CredentialHealthStatus::Expired));
        assert!(detail.contains("expired"), "got: {detail}");
    }

    /// N-11.2: a refresh token inside the 30-day window is `expiring_soon`.
    #[test]
    fn ebay_status_expiring_soon_within_window() {
        let ts = db_timestamp(12);
        let m = meta(Some(&ts));
        let (status, detail) = ebay_credential_status(Some(&m));
        assert!(matches!(status, CredentialHealthStatus::ExpiringSoon));
        // Whole-day truncation can render this as 11 or 12 days; the stable
        // assertions are the warning copy and the expiry timestamp.
        assert!(detail.contains("expires in"), "got: {detail}");
        assert!(detail.contains("day"), "got: {detail}");
        assert!(detail.contains(&ts), "got: {detail}");
    }

    /// N-11.2: a refresh token far from expiry is `ok`.
    #[test]
    fn ebay_status_ok_far_future() {
        let m = meta(Some(&db_timestamp(300)));
        let (status, _) = ebay_credential_status(Some(&m));
        assert!(matches!(status, CredentialHealthStatus::Ok));
    }

    /// N-11.2: no stored token is `attention`, not silent.
    #[test]
    fn ebay_status_attention_without_row() {
        let (status, detail) = ebay_credential_status(None);
        assert!(matches!(status, CredentialHealthStatus::Attention));
        assert!(detail.contains("Not connected"), "got: {detail}");
    }

    /// N-11.2: an unparseable stored expiry degrades to `attention`.
    #[test]
    fn ebay_status_attention_on_unparseable_timestamp() {
        let m = meta(Some("not-a-timestamp"));
        let (status, _) = ebay_credential_status(Some(&m));
        assert!(matches!(status, CredentialHealthStatus::Attention));
    }

    /// N-11.2: an Amazon client secret older than the rotation window warns.
    #[test]
    fn amazon_status_secret_rotation_due() {
        let old = (chrono::Utc::now() - chrono::Duration::days(400)).to_rfc3339();
        let (status, detail) = amazon_credential_status(None, Some(&old));
        assert!(matches!(status, CredentialHealthStatus::ExpiringSoon));
        assert!(detail.contains("rotate"), "got: {detail}");
    }

    /// N-11.2: a recently rotated Amazon secret is `ok`.
    #[test]
    fn amazon_status_secret_recently_rotated() {
        let recent = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        let (status, _) = amazon_credential_status(None, Some(&recent));
        assert!(matches!(status, CredentialHealthStatus::Ok));
    }

    /// N-11.2: pre-N-11.2 configs without a recorded rotation date do not
    /// nag — they report `ok` with guidance.
    #[test]
    fn amazon_status_ok_when_rotation_date_unknown() {
        let m = meta(None);
        let (status, _) = amazon_credential_status(Some(&m), None);
        assert!(matches!(status, CredentialHealthStatus::Ok));
    }
}
