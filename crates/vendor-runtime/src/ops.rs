use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{op2, OpState};

/// The largest response body `op_nisaba_fetch` will read, in bytes. Bodies are buffered
/// in Rust — outside the V8 heap and its limit — so they need a ceiling of their own.
/// Without one in the `OpState`, bodies are unbounded.
#[derive(Clone, Copy)]
pub struct MaxResponseBytes(pub usize);

/// HTTP fetch via reqwest — the only network access plugins get.
#[op2]
#[string]
pub async fn op_nisaba_fetch(
    state: Rc<RefCell<OpState>>,
    #[string] url: String,
    #[string] options_json: String,
) -> Result<String, deno_error::JsErrorBox> {
    let max_body = state
        .borrow()
        .try_borrow::<MaxResponseBytes>()
        .map_or(usize::MAX, |m| m.0);

    #[derive(serde::Deserialize)]
    struct FetchOptions {
        #[serde(default = "default_method")]
        method: String,
        #[serde(default)]
        headers: std::collections::HashMap<String, String>,
        body: Option<String>,
    }

    fn default_method() -> String {
        "GET".to_string()
    }

    let opts: FetchOptions = serde_json::from_str(&options_json).unwrap_or(FetchOptions {
        method: "GET".to_string(),
        headers: Default::default(),
        body: None,
    });

    let client = reqwest::Client::new();
    let method =
        reqwest::Method::from_bytes(opts.method.as_bytes()).unwrap_or(reqwest::Method::GET);

    let mut req = client.request(method, &url);

    for (k, v) in &opts.headers {
        req = req.header(k.as_str(), v.as_str());
    }

    if let Some(body) = opts.body {
        req = req.body(body);
    }

    let mut resp = req
        .send()
        .await
        .map_err(|e| deno_error::JsErrorBox::generic(format!("Fetch error: {e}")))?;
    let status = resp.status().as_u16();
    let resp_headers = resp.headers().clone();
    let headers: std::collections::HashMap<String, String> = resp_headers
        .iter()
        .map(
            |(k, v): (&reqwest::header::HeaderName, &reqwest::header::HeaderValue)| {
                (k.to_string(), v.to_str().unwrap_or("").to_string())
            },
        )
        .collect();
    let too_large = || {
        deno_error::JsErrorBox::generic(format!(
            "Response body exceeds the plugin limit of {max_body} bytes"
        ))
    };
    // Refuse up front when the server declares the size, and count while streaming for
    // when it does not (or lies).
    if resp
        .content_length()
        .is_some_and(|len| len > max_body as u64)
    {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| deno_error::JsErrorBox::generic(format!("Body read error: {e}")))?
    {
        if bytes.len() + chunk.len() > max_body {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    let body = String::from_utf8_lossy(&bytes).into_owned();

    let result = serde_json::json!({
        "status": status,
        "headers": headers,
        "body": body,
    });

    Ok(result.to_string())
}

/// Sleep op — async delay for rate limiting / throttling.
#[op2]
pub async fn op_nisaba_sleep(#[bigint] millis: u64) -> Result<(), deno_error::JsErrorBox> {
    tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
    Ok(())
}

/// Logging op — routes to Rust tracing.
#[op2(fast)]
pub fn op_nisaba_log(#[string] level: &str, #[string] msg: &str) {
    match level {
        "trace" => tracing::trace!(target: "vendor_plugin", "{msg}"),
        "debug" => tracing::debug!(target: "vendor_plugin", "{msg}"),
        "info" => tracing::info!(target: "vendor_plugin", "{msg}"),
        "warn" => tracing::warn!(target: "vendor_plugin", "{msg}"),
        "error" => tracing::error!(target: "vendor_plugin", "{msg}"),
        _ => tracing::info!(target: "vendor_plugin", "{msg}"),
    }
}

/// Store plugin result from JS into OpState for Rust retrieval.
#[op2(fast)]
pub fn op_nisaba_set_result(
    state: &mut OpState,
    #[string] json: String,
) -> Result<(), deno_error::JsErrorBox> {
    OutputBudget::charge(state, json.len())?;
    state.put(PluginResult(json));
    Ok(())
}

/// Wrapper to store a plugin result string in OpState.
pub struct PluginResult(pub String);

/// Callback for streaming batch results to the host application.
pub struct BatchCallback(pub Box<dyn Fn(String) + Send + Sync>);

/// How many more bytes of listing JSON a plugin may hand the host, across every
/// `emitBatch` and its final result. Without a budget in the `OpState` output is unbounded.
pub struct OutputBudget {
    pub remaining: usize,
}

impl OutputBudget {
    fn charge(state: &mut OpState, bytes: usize) -> Result<(), deno_error::JsErrorBox> {
        if let Some(budget) = state.try_borrow_mut::<OutputBudget>() {
            if bytes > budget.remaining {
                budget.remaining = 0;
                return Err(deno_error::JsErrorBox::generic(
                    "Plugin exceeded its output limit",
                ));
            }
            budget.remaining -= bytes;
        }
        Ok(())
    }
}

/// Emit a batch of listings to the host for progressive UI updates.
#[op2(fast)]
pub fn op_nisaba_emit_batch(
    state: &mut OpState,
    #[string] json: String,
) -> Result<(), deno_error::JsErrorBox> {
    OutputBudget::charge(state, json.len())?;
    if let Some(cb) = state.try_borrow::<BatchCallback>() {
        (cb.0)(json);
    }
    Ok(())
}
