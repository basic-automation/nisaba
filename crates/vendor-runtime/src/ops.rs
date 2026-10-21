use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

use deno_core::{op2, OpState};

use nisaba_core::net_guard;

/// Post-DNS SSRF guard for `op_nisaba_fetch`, consulted alongside `FetchPolicy`.
/// When `true` (the production default — the op fails closed when it is absent from
/// the `OpState`), hosts resolving to loopback, private, link-local or otherwise
/// non-public addresses are refused, including on every redirect hop. The loopback
/// fixture servers used by the test suite opt out explicitly.
#[derive(Clone, Copy)]
pub struct BlockPrivateHosts(pub bool);

/// The largest response body `op_nisaba_fetch` will read, in bytes. Bodies are buffered
/// in Rust — outside the V8 heap and its limit — so they need a ceiling of their own.
/// Without one in the `OpState`, bodies are unbounded.
#[derive(Clone, Copy)]
pub struct MaxResponseBytes(pub usize);

/// Which hosts `op_nisaba_fetch` may reach during this run. Without a policy in the
/// `OpState` every fetch is refused.
#[derive(Debug, Clone)]
pub enum FetchPolicy {
    /// No network at all — a metadata read has no business making requests.
    Deny,
    /// The plugin declared no `allowed_hosts`, so any host is reachable. Kept for plugins
    /// written before the allowlist existed; the UI flags them.
    Unrestricted,
    /// Only these hosts. An entry is an exact host name (`api.example.com`), or
    /// `*.example.com` for any subdomain of `example.com` (not the apex itself).
    AllowList(Arc<Vec<String>>),
}

impl FetchPolicy {
    /// Whether a request to `url` — including any redirect hop — is permitted.
    pub fn allows(&self, url: &reqwest::Url) -> bool {
        if !matches!(url.scheme(), "http" | "https") {
            return false;
        }
        match self {
            FetchPolicy::Deny => false,
            FetchPolicy::Unrestricted => true,
            FetchPolicy::AllowList(hosts) => {
                let Some(host) = url.host_str() else {
                    return false;
                };
                let host = host.trim_end_matches('.').to_ascii_lowercase();
                hosts.iter().any(|entry| {
                    let entry = entry.trim_end_matches('.').to_ascii_lowercase();
                    match entry.strip_prefix("*.") {
                        Some(apex) => host
                            .strip_suffix(apex)
                            .is_some_and(|sub| sub.len() > 1 && sub.ends_with('.')),
                        None => host == entry,
                    }
                })
            }
        }
    }

    fn refusal(&self, url: &reqwest::Url) -> String {
        match self {
            FetchPolicy::Deny => {
                "Nisaba.fetch is not available while reading plugin metadata".into()
            }
            _ => format!(
                "Nisaba.fetch to {} is not allowed: it is not in the plugin's allowed_hosts",
                url.host_str().unwrap_or(url.as_str())
            ),
        }
    }
}

/// Redirects are followed at most this many hops; every hop is held to the same
/// policy *and* the same post-DNS refusal as the initial request, so an allowed
/// host cannot bounce the request somewhere the plugin could not reach directly.
const MAX_REDIRECTS: usize = 10;

/// The full gate for one fetch target: the host-name policy first, then the
/// post-DNS SSRF refusal.
async fn check_fetch_target(
    policy: &FetchPolicy,
    block_private: bool,
    url: &reqwest::Url,
) -> Result<(), String> {
    if !policy.allows(url) {
        return Err(policy.refusal(url));
    }
    if block_private {
        match url.host_str() {
            Some(host) => net_guard::ensure_host_is_public(host).await?,
            None => return Err(format!("Fetch error: URL has no host: {url}")),
        }
    }
    Ok(())
}

/// HTTP fetch via reqwest — the only network access plugins get.
#[op2]
#[string]
pub async fn op_nisaba_fetch(
    state: Rc<RefCell<OpState>>,
    #[string] url: String,
    #[string] options_json: String,
) -> Result<String, deno_error::JsErrorBox> {
    let (max_body, policy, block_private) = {
        let state = state.borrow();
        (
            state
                .try_borrow::<MaxResponseBytes>()
                .map_or(usize::MAX, |m| m.0),
            state
                .try_borrow::<FetchPolicy>()
                .cloned()
                .unwrap_or(FetchPolicy::Deny),
            // Absent guard means enforced: production always installs it explicitly.
            state.try_borrow::<BlockPrivateHosts>().is_none_or(|b| b.0),
        )
    };

    let parsed = reqwest::Url::parse(&url)
        .map_err(|e| deno_error::JsErrorBox::generic(format!("Fetch error: invalid URL: {e}")))?;
    check_fetch_target(&policy, block_private, &parsed)
        .await
        .map_err(deno_error::JsErrorBox::generic)?;

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

    // Redirects are followed manually rather than by reqwest's redirect policy so
    // that every hop can be gated asynchronously: the host-name policy and the
    // post-DNS SSRF refusal both run again on the resolved `Location`, so an
    // allowed host cannot bounce the request somewhere the plugin could not reach
    // directly.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| deno_error::JsErrorBox::generic(format!("Fetch error: {e}")))?;
    let mut method =
        reqwest::Method::from_bytes(opts.method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut url = parsed;
    let mut headers = opts.headers;
    let mut body = opts.body;

    let send_error = |e: reqwest::Error| {
        // reqwest's Display omits the cause, and the cause is what a plugin author
        // needs — e.g. why the connection failed.
        let mut msg = format!("Fetch error: {e}");
        let mut source = std::error::Error::source(&e);
        while let Some(cause) = source {
            msg.push_str(&format!(": {cause}"));
            source = cause.source();
        }
        deno_error::JsErrorBox::generic(msg)
    };

    let mut resp = None;
    for _ in 0..=MAX_REDIRECTS {
        let mut req = client.request(method.clone(), url.clone());
        for (k, v) in &headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if let Some(b) = &body {
            req = req.body(b.clone());
        }
        let r = req.send().await.map_err(send_error)?;
        if !r.status().is_redirection() {
            resp = Some(r);
            break;
        }
        let location = r
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                deno_error::JsErrorBox::generic("Fetch error: redirect without a Location header")
            })?;
        let next = url.join(location).map_err(|e| {
            deno_error::JsErrorBox::generic(format!("Fetch error: bad redirect location: {e}"))
        })?;
        if let Err(e) = check_fetch_target(&policy, block_private, &next).await {
            return Err(deno_error::JsErrorBox::generic(format!(
                "redirect refused: {e}"
            )));
        }
        // 301/302/303 turn a non-GET/HEAD request into a GET without a body, as
        // browsers do; 307/308 keep method and body.
        if matches!(r.status().as_u16(), 301..=303)
            && !matches!(method, reqwest::Method::GET | reqwest::Method::HEAD)
        {
            method = reqwest::Method::GET;
            body = None;
        }
        // Do not leak credentials to a different origin on redirect.
        if next.host_str() != url.host_str()
            || next.port_or_known_default() != url.port_or_known_default()
        {
            headers.retain(|k, _| {
                let k = k.to_ascii_lowercase();
                k != "authorization" && k != "cookie"
            });
        }
        url = next;
    }
    let mut resp =
        resp.ok_or_else(|| deno_error::JsErrorBox::generic("Fetch error: too many redirects"))?;
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

/// Per-run collector for a plugin's own log lines (N-7.5). Installed in the
/// `OpState` by the host when it wants the run's lines back (for the run
/// history); the lines are also routed to Rust tracing either way, exactly
/// as before. Capped so a chatty plugin cannot grow it without bound.
#[derive(Clone, Default)]
pub struct PluginLogCollector {
    lines: Arc<std::sync::Mutex<Vec<String>>>,
}

/// The most log lines kept for one run.
pub const PLUGIN_LOG_MAX_LINES: usize = 200;
/// The most characters kept for one log line.
pub const PLUGIN_LOG_MAX_LINE_CHARS: usize = 500;

impl PluginLogCollector {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, level: &str, msg: &str) {
        if let Ok(mut lines) = self.lines.lock() {
            if lines.len() < PLUGIN_LOG_MAX_LINES {
                let msg: String = msg.chars().take(PLUGIN_LOG_MAX_LINE_CHARS).collect();
                lines.push(format!("[{level}] {msg}"));
            }
        }
    }

    /// The lines captured so far, oldest first.
    pub fn snapshot(&self) -> Vec<String> {
        self.lines.lock().map(|l| l.clone()).unwrap_or_default()
    }
}

/// Logging op — routes to Rust tracing and, when the host installed a
/// [`PluginLogCollector`] for this run, captures the line for the run
/// history (N-7.5).
#[op2(fast)]
pub fn op_nisaba_log(state: &mut OpState, #[string] level: &str, #[string] msg: &str) {
    if let Some(collector) = state.try_borrow::<PluginLogCollector>() {
        collector.push(level, msg);
    }
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

// ── Per-plugin key–value store (`Nisaba.store`) ──────────────────────────

/// Total bytes (key + value, UTF-8) one plugin may keep in `Nisaba.store`.
/// Big enough for cursors, ETags and last-seen timestamps; small enough that
/// a runaway plugin cannot grow the company database without bound.
/// Documented in `plugins/template/index.ts` and `SECURITY.md`.
pub const PLUGIN_STORE_CAP_BYTES: u64 = 1024 * 1024;

/// A store key may be at most this many bytes (UTF-8).
pub const PLUGIN_STORE_MAX_KEY_BYTES: usize = 256;

/// A backend failure from the plugin store, surfaced to the plugin as a
/// rejected promise. The message never includes key material — only what the
/// plugin already passed in.
#[derive(Debug)]
pub struct StoreError(pub String);

/// Host-side storage behind `Nisaba.store.get/set/delete`.
///
/// Scoped by `plugin_id`: a backend must never serve one plugin's keys to
/// another. The company database gives the company half of the scoping for
/// free (every company has its own database file); the `plugin_id` argument is
/// the other half. The size cap is enforced by the ops, not the backend, so
/// every backend behaves identically.
pub trait PluginStore: Send + Sync + 'static {
    fn get(
        &self,
        plugin_id: &str,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, StoreError>> + '_>>;
    fn set(
        &self,
        plugin_id: &str,
        key: &str,
        value: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + '_>>;
    /// Delete a key. Returns whether the key existed.
    fn delete(
        &self,
        plugin_id: &str,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, StoreError>> + '_>>;
    /// Total bytes (key + value) currently stored for this plugin, which the
    /// `set` op charges the [`PLUGIN_STORE_CAP_BYTES`] cap against.
    fn total_bytes(
        &self,
        plugin_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<u64, StoreError>> + '_>>;
}

/// The store backend plus the plugin identity it is scoped to, installed in
/// the `OpState` for the `fetchListings` run only. Metadata reads get no
/// store: `Nisaba.store` throws there, because the plugin has no identity yet
/// and a metadata read must not have side effects.
pub struct PluginStoreContext {
    pub plugin_id: String,
    pub store: Arc<dyn PluginStore>,
}

fn store_context(
    state: &OpState,
) -> Result<(String, Arc<dyn PluginStore>), deno_error::JsErrorBox> {
    match state.try_borrow::<PluginStoreContext>() {
        Some(ctx) => Ok((ctx.plugin_id.clone(), ctx.store.clone())),
        None => Err(deno_error::JsErrorBox::generic(
            "Nisaba.store is not available in this context",
        )),
    }
}

fn validate_store_key(key: &str) -> Result<(), deno_error::JsErrorBox> {
    if key.is_empty() {
        return Err(deno_error::JsErrorBox::generic(
            "Nisaba.store: key must be a non-empty string",
        ));
    }
    if key.len() > PLUGIN_STORE_MAX_KEY_BYTES {
        return Err(deno_error::JsErrorBox::generic(format!(
            "Nisaba.store: key is {} bytes, the limit is {PLUGIN_STORE_MAX_KEY_BYTES}",
            key.len()
        )));
    }
    Ok(())
}

fn store_failed(context: &str, e: StoreError) -> deno_error::JsErrorBox {
    deno_error::JsErrorBox::generic(format!("Nisaba.store.{context} failed: {}", e.0))
}

/// `Nisaba.store.get(key)` — resolves to the stored string, or `null` when
/// the key was never set. Never sees another plugin's keys: the backend is
/// always queried with this run's `plugin_id`.
#[op2]
#[string]
pub async fn op_nisaba_store_get(
    state: Rc<RefCell<OpState>>,
    #[string] key: String,
) -> Result<String, deno_error::JsErrorBox> {
    validate_store_key(&key)?;
    let (plugin_id, store) = {
        let state = state.borrow();
        store_context(&state)?
    };
    let value = store
        .get(&plugin_id, &key)
        .await
        .map_err(|e| store_failed("get", e))?;
    Ok(match value {
        Some(v) => serde_json::json!({"found": true, "value": v}).to_string(),
        None => r#"{"found":false}"#.to_string(),
    })
}

/// `Nisaba.store.set(key, value)` — persists a string between runs. The write
/// is charged against the per-plugin [`PLUGIN_STORE_CAP_BYTES`] cap first, so
/// a plugin that would exceed it gets a clear rejection instead of silently
/// growing the database.
#[op2]
pub async fn op_nisaba_store_set(
    state: Rc<RefCell<OpState>>,
    #[string] key: String,
    #[string] value: String,
) -> Result<(), deno_error::JsErrorBox> {
    validate_store_key(&key)?;
    let (plugin_id, store) = {
        let state = state.borrow();
        store_context(&state)?
    };
    let total = store
        .total_bytes(&plugin_id)
        .await
        .map_err(|e| store_failed("set", e))?;
    let existing = store
        .get(&plugin_id, &key)
        .await
        .map_err(|e| store_failed("set", e))?;
    let existing_bytes: u64 = existing
        .map(|v| key.len() as u64 + v.len() as u64)
        .unwrap_or(0);
    let new_total = total.saturating_sub(existing_bytes) + key.len() as u64 + value.len() as u64;
    if new_total > PLUGIN_STORE_CAP_BYTES {
        return Err(deno_error::JsErrorBox::generic(format!(
            "Nisaba.store.set: this write would exceed the per-plugin store limit of {} bytes",
            PLUGIN_STORE_CAP_BYTES
        )));
    }
    store
        .set(&plugin_id, &key, &value)
        .await
        .map_err(|e| store_failed("set", e))?;
    Ok(())
}

/// `Nisaba.store.delete(key)` — resolves to whether the key existed.
#[op2]
pub async fn op_nisaba_store_delete(
    state: Rc<RefCell<OpState>>,
    #[string] key: String,
) -> Result<bool, deno_error::JsErrorBox> {
    validate_store_key(&key)?;
    let (plugin_id, store) = {
        let state = state.borrow();
        store_context(&state)?
    };
    store
        .delete(&plugin_id, &key)
        .await
        .map_err(|e| store_failed("delete", e))
}
