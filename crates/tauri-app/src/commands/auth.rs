use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use nisaba_core::types::Platform;
use nisaba_amazon::AmazonAdapter;
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

    let _auth_window = WebviewWindowBuilder::new(
        &app,
        "ebay-auth",
        WebviewUrl::External(url_parsed),
    )
    .title("eBay Authorization")
    .inner_size(900.0, 700.0)
    .center()
    .on_navigation(move |url| {
        if handled_nav.load(Ordering::SeqCst) {
            return false;
        }

        if let Some(code) = extract_code_from_url(url) {
            handled_nav.store(true, Ordering::SeqCst);
            let adapter = adapter_clone.clone();
            let app = app_nav.clone();

            tauri::async_runtime::spawn(async move {
                if let Some(ebay) = adapter.as_any().downcast_ref::<EbayAdapter>() {
                    match ebay.complete_auth(&code).await {
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

    ebay.complete_auth(&code).await.map_err(|e| e.to_string())
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

    let _auth_window = WebviewWindowBuilder::new(
        &app,
        "amazon-auth",
        WebviewUrl::External(url_parsed),
    )
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
pub async fn complete_amazon_auth(
    state: State<'_, AppState>,
    code: String,
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

#[derive(serde::Serialize)]
pub struct PlatformHealth {
    pub platform: String,
    pub enabled: bool,
    pub authenticated: bool,
}

#[tauri::command]
pub async fn get_platform_health(
    state: State<'_, AppState>,
) -> Result<Vec<PlatformHealth>, String> {
    let all_platforms = [Platform::Squarespace, Platform::Ebay, Platform::XmrBazaar, Platform::Amazon];
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

fn extract_amazon_code_from_url(url: &url::Url) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == "spapi_oauth_code")
        .map(|(_, value)| value.into_owned())
}
