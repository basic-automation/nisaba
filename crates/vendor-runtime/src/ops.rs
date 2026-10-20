use std::cell::RefCell;
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
