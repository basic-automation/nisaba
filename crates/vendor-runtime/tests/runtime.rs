//! Behaviour of the plugin runtime as a plugin author sees it: metadata reads, the
//! TypeScript transpile path, config hand-off, `emitBatch`, `Nisaba.fetch` (against a
//! loopback server — never the network), `sleep`, `log`, and how failures surface.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nisaba_vendor_runtime::{BatchCallback, VendorListing, VendorRuntime};

const METADATA: &str = r#"export const metadata = { name: "fixture", version: "1.0.0" };"#;

async fn run(body: &str) -> anyhow::Result<Vec<VendorListing>> {
    run_with(body, HashMap::new(), None).await
}

async fn run_with(
    body: &str,
    config: HashMap<String, String>,
    cb: Option<BatchCallback>,
) -> anyhow::Result<Vec<VendorListing>> {
    let code = format!("{METADATA}\nexport async function fetchListings(config) {{ {body} }}");
    VendorRuntime::execute_plugin(&code, config, cb).await
}

fn listing(id: &str) -> String {
    format!(r#"{{ vendor_item_id: "{id}", title: "Item {id}" }}"#)
}

// ── metadata ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn metadata_is_read_with_defaults_filled_in() {
    let meta = VendorRuntime::read_metadata(
        r#"export const metadata = {
               name: "Acme Wholesale",
               version: "2.1.0",
               config_fields: [
                   { key: "api_key", label: "API key", required: true, secret: true },
                   { key: "region", label: "Region", placeholder: "us-east" },
               ],
           };
           export async function fetchListings() { return []; }"#,
    )
    .await
    .unwrap();

    assert_eq!(meta.name, "Acme Wholesale");
    assert_eq!(meta.version, "2.1.0");
    assert_eq!(meta.description, "");
    assert_eq!(meta.category, "vendor");
    assert_eq!(meta.icon, None);
    assert_eq!(meta.config_fields.len(), 2);
    let key = &meta.config_fields[0];
    assert!(key.required && key.secret);
    assert_eq!(key.placeholder, None);
    let region = &meta.config_fields[1];
    assert!(!region.required && !region.secret);
    assert_eq!(region.placeholder.as_deref(), Some("us-east"));
}

#[tokio::test]
async fn metadata_read_does_not_call_fetch_listings() {
    let meta = VendorRuntime::read_metadata(&format!(
        r#"{METADATA}
           export async function fetchListings() {{ throw new Error("must not run"); }}"#
    ))
    .await
    .unwrap();
    assert_eq!(meta.name, "fixture");
}

#[tokio::test]
async fn a_plugin_without_metadata_is_rejected() {
    let err = VendorRuntime::read_metadata("export async function fetchListings() { return []; }")
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("metadata"), "{err:#}");
}

#[tokio::test]
async fn metadata_missing_a_required_field_is_rejected() {
    let err = VendorRuntime::read_metadata(r#"export const metadata = { name: "no version" };"#)
        .await
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("Failed to parse plugin metadata"),
        "{err:#}"
    );
}

// ── transpile ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn typescript_is_transpiled() {
    let code = r#"
        interface Raw { id: number; name: string; price?: number }
        type Mapped = { vendor_item_id: string; title: string; price: number | null };
        enum Currency { Usd = "USD" }
        function map<T extends Raw>(r: T): Mapped {
            return { vendor_item_id: String(r.id), title: r.name, price: r.price ?? null };
        }
        export const metadata = { name: "ts", version: "1.0.0" } satisfies { name: string };
        export async function fetchListings(_config: Record<string, string>): Promise<Mapped[]> {
            const raw = [{ id: 7, name: "Widget", price: 9.5 }] as Raw[];
            return raw.map(map).map((m) => ({ ...m, currency: Currency.Usd }));
        }
    "#;
    let listings = VendorRuntime::execute_plugin(code, HashMap::new(), None)
        .await
        .unwrap();
    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].vendor_item_id, "7");
    assert_eq!(listings[0].title, "Widget");
    assert_eq!(listings[0].price, Some(9.5));
    assert_eq!(listings[0].currency.as_deref(), Some("USD"));
}

#[tokio::test]
async fn a_typescript_syntax_error_is_reported() {
    let err = VendorRuntime::execute_plugin("export const = ;", HashMap::new(), None)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("parse error"), "{err:#}");
}

// ── config and results ──────────────────────────────────────────────────────

#[tokio::test]
async fn config_values_reach_the_plugin_verbatim() {
    // The config is spliced into the wrapper module as a JSON literal, so values that
    // look like code must arrive as inert strings.
    let hostile = [
        ("quotes", r#"a"b'c`d"#),
        ("backslash", r"C:\path\n"),
        ("newlines", "line1\nline2\r\n"),
        ("template", "${globalThis.pwned = 1}"),
        ("script", "</script><script>alert(1)</script>"),
        ("separators", "a\u{2028}b\u{2029}c"),
        ("unicode", "naïve — 日本語 🦀"),
        ("close_object", "\"}; globalThis.pwned = 1; ({\""),
    ];
    let config: HashMap<String, String> = hostile
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let listings = run_with(
        r#"return Object.entries(config).map(([k, v]) => ({
               vendor_item_id: k, title: v, extras: { pwned: String(globalThis.pwned ?? "") } }));"#,
        config,
        None,
    )
    .await
    .unwrap();

    let got: HashMap<&str, &str> = listings
        .iter()
        .map(|l| (l.vendor_item_id.as_str(), l.title.as_str()))
        .collect();
    for (k, v) in hostile {
        assert_eq!(got[k], v, "config value {k} was altered");
    }
    assert!(listings.iter().all(|l| l.extras["pwned"].is_empty()));
}

#[tokio::test]
async fn optional_listing_fields_default_when_omitted() {
    let listings = run(&format!("return [{}];", listing("1"))).await.unwrap();
    let l = &listings[0];
    assert_eq!(
        (l.price, l.quantity, &l.sku, &l.group_key),
        (None, None, &None, &None)
    );
    assert!(l.extras.is_empty() && l.variant_attributes.is_empty());
}

#[tokio::test]
async fn a_result_that_is_not_a_listing_array_is_rejected() {
    for body in [
        "return { listings: [] };",
        r#"return [{ title: "no id" }];"#,
        "return undefined;",
    ] {
        let err = run(body).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("Failed to parse plugin result"),
            "{body} → {err:#}"
        );
    }
}

#[tokio::test]
async fn a_throwing_plugin_is_an_error_carrying_its_message() {
    let err = run(r#"throw new Error("vendor API said no");"#)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("vendor API said no"), "{err:#}");
}

#[tokio::test]
async fn a_plugin_that_never_settles_is_an_error_not_a_hang() {
    // With nothing left on the event loop, a promise that can never resolve is detected
    // as a stalled top-level await. (A plugin that keeps the loop busy forever is not
    // bounded yet — that is the Phase 3 resource-limits item.)
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        run("await new Promise(() => {}); return [];"),
    )
    .await
    .expect("the runtime hung")
    .unwrap_err();
    assert!(!format!("{err:#}").is_empty());
}

#[tokio::test]
async fn a_missing_fetch_listings_export_is_rejected() {
    let err = VendorRuntime::execute_plugin(METADATA, HashMap::new(), None)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("fetchListings"), "{err:#}");
}

// ── multi-file ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_multi_file_plugin_needs_an_index_ts() {
    let files = HashMap::from([("main.ts".to_string(), METADATA.to_string())]);
    let err = VendorRuntime::read_metadata_from_files(&files)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("index.ts"), "{err:#}");
}

#[tokio::test]
async fn inline_data_url_assets_are_skipped_not_written() {
    // Icons travel in the files map as `data:` URLs; they are not modules and must not be
    // written out as if they were source.
    let files = HashMap::from([
        (
            "index.ts".to_string(),
            format!(
                r#"{METADATA}
                   export async function fetchListings() {{
                       try {{ await import("./icon.js"); return [{}]; }}
                       catch {{ return []; }}
                   }}"#,
                listing("icon-was-written")
            ),
        ),
        (
            "icon.js".to_string(),
            "data:image/png;base64,iVBORw0KGgo=".to_string(),
        ),
    ]);
    let meta = VendorRuntime::read_metadata_from_files(&files)
        .await
        .unwrap();
    assert_eq!(meta.name, "fixture");
    let listings = VendorRuntime::execute_plugin_from_files(&files, HashMap::new(), None)
        .await
        .unwrap();
    assert!(listings.is_empty(), "the data: asset was written to disk");
}

// ── emitBatch ───────────────────────────────────────────────────────────────

fn collecting_callback() -> (BatchCallback, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let cb = BatchCallback(Box::new(move |json| sink.lock().unwrap().push(json)));
    (cb, seen)
}

#[tokio::test]
async fn emit_batch_streams_batches_in_order_separately_from_the_result() {
    let (cb, seen) = collecting_callback();
    let listings = run_with(
        &format!(
            "Nisaba.emitBatch([{a}, {b}]); Nisaba.emitBatch([{c}]); return [{d}];",
            a = listing("a"),
            b = listing("b"),
            c = listing("c"),
            d = listing("final"),
        ),
        HashMap::new(),
        Some(cb),
    )
    .await
    .unwrap();

    let batches: Vec<Vec<VendorListing>> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|json| serde_json::from_str(json).unwrap())
        .collect();
    let ids: Vec<Vec<&str>> = batches
        .iter()
        .map(|b| b.iter().map(|l| l.vendor_item_id.as_str()).collect())
        .collect();
    assert_eq!(ids, [vec!["a", "b"], vec!["c"]]);
    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].vendor_item_id, "final");
}

#[tokio::test]
async fn emit_batch_without_a_callback_is_a_no_op() {
    let listings = run(&format!(
        "Nisaba.emitBatch([{}]); return [];",
        listing("dropped")
    ))
    .await
    .unwrap();
    assert!(listings.is_empty());
}

// ── fetch ───────────────────────────────────────────────────────────────────

/// What the loopback server saw.
#[derive(Debug, Default)]
struct Seen {
    request_line: String,
    headers: HashMap<String, String>,
    body: String,
}

/// Serve exactly one HTTP request on 127.0.0.1 with a canned response. Returns the URL
/// and a handle yielding what the request looked like.
fn serve_once(
    status: &'static str,
    headers: &'static [(&'static str, &'static str)],
    body: &'static str,
) -> (String, std::thread::JoinHandle<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/items?page=1", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut seen = Seen::default();
        reader.read_line(&mut seen.request_line).unwrap();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            let (k, v) = line.split_once(':').unwrap();
            seen.headers
                .insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
        let len: usize = seen
            .headers
            .get("content-length")
            .map(|v| v.parse().unwrap())
            .unwrap_or(0);
        let mut buf = vec![0; len];
        reader.read_exact(&mut buf).unwrap();
        seen.body = String::from_utf8(buf).unwrap();

        let mut resp = format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\n", body.len());
        for (k, v) in headers {
            resp.push_str(&format!("{k}: {v}\r\n"));
        }
        resp.push_str("connection: close\r\n\r\n");
        resp.push_str(body);
        let mut stream = stream;
        stream.write_all(resp.as_bytes()).unwrap();
        seen
    });
    (url, handle)
}

#[tokio::test]
async fn fetch_round_trips_method_headers_and_body() {
    let (url, server) = serve_once(
        "201 Created",
        &[
            ("content-type", "application/json"),
            ("x-rate-remaining", "42"),
        ],
        r#"{"items":[{"id":"sku-1","name":"Canteen"}]}"#,
    );

    let mut config = HashMap::new();
    config.insert("url".to_string(), url);
    let listings = run_with(
        r#"
        const res = await Nisaba.fetch(config.url, {
            method: "POST",
            headers: { "Authorization": "Bearer test-token", "Content-Type": "application/json" },
            body: JSON.stringify({ query: "{ items }" }),
        });
        const data = res.json();
        return data.items.map((i) => ({
            vendor_item_id: i.id,
            title: i.name,
            extras: {
                status: String(res.status),
                ok: String(res.ok),
                rate: res.headers["x-rate-remaining"],
                text_matches_json: String(JSON.stringify(JSON.parse(res.text())) === JSON.stringify(data)),
            },
        }));
        "#,
        config,
        None,
    )
    .await
    .unwrap();

    let seen = server.join().unwrap();
    assert!(
        seen.request_line.starts_with("POST /items?page=1 "),
        "{}",
        seen.request_line
    );
    assert_eq!(seen.headers["authorization"], "Bearer test-token");
    assert_eq!(seen.headers["content-type"], "application/json");
    assert_eq!(seen.body, r#"{"query":"{ items }"}"#);

    let l = &listings[0];
    assert_eq!(
        (l.vendor_item_id.as_str(), l.title.as_str()),
        ("sku-1", "Canteen")
    );
    assert_eq!(l.extras["status"], "201");
    assert_eq!(l.extras["ok"], "true");
    assert_eq!(l.extras["rate"], "42");
    assert_eq!(l.extras["text_matches_json"], "true");
}

#[tokio::test]
async fn fetch_defaults_to_get_without_a_body() {
    let (url, server) = serve_once("200 OK", &[], "plain text");
    let mut config = HashMap::new();
    config.insert("url".to_string(), url);
    let listings = run_with(
        r#"const res = await Nisaba.fetch(config.url);
           return [{ vendor_item_id: "1", title: res.text() }];"#,
        config,
        None,
    )
    .await
    .unwrap();
    let seen = server.join().unwrap();
    assert!(
        seen.request_line.starts_with("GET "),
        "{}",
        seen.request_line
    );
    assert_eq!(seen.body, "");
    assert_eq!(listings[0].title, "plain text");
}

#[tokio::test]
async fn a_non_2xx_response_is_not_ok_but_does_not_throw() {
    let (url, server) = serve_once(
        "429 Too Many Requests",
        &[("retry-after", "3")],
        "slow down",
    );
    let mut config = HashMap::new();
    config.insert("url".to_string(), url);
    let listings = run_with(
        r#"const res = await Nisaba.fetch(config.url);
           return [{ vendor_item_id: String(res.status), title: String(res.ok),
                     extras: { retry: res.headers["retry-after"], body: res.text() } }];"#,
        config,
        None,
    )
    .await
    .unwrap();
    server.join().unwrap();
    let l = &listings[0];
    assert_eq!(
        (l.vendor_item_id.as_str(), l.title.as_str()),
        ("429", "false")
    );
    assert_eq!(l.extras["retry"], "3");
    assert_eq!(l.extras["body"], "slow down");
}

#[tokio::test]
async fn a_connection_failure_throws_a_catchable_error() {
    // Bind then drop, so the port is (almost certainly) closed.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut config = HashMap::new();
    config.insert("url".to_string(), format!("http://127.0.0.1:{port}/"));
    let listings = run_with(
        r#"try { await Nisaba.fetch(config.url); return []; }
           catch (e) { return [{ vendor_item_id: "caught", title: String(e.message) }]; }"#,
        config,
        None,
    )
    .await
    .unwrap();
    assert_eq!(listings[0].vendor_item_id, "caught");
    assert!(
        listings[0].title.contains("Fetch error"),
        "{}",
        listings[0].title
    );
}

// ── sleep and log ───────────────────────────────────────────────────────────

#[tokio::test]
async fn sleep_waits_at_least_the_requested_time() {
    let start = Instant::now();
    run("await Nisaba.sleep(60); return [];").await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(60));
}

/// A `tracing` writer that appends everything to a shared buffer.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn log_routes_to_tracing_under_the_vendor_plugin_target() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(run(r#"
            Nisaba.log.trace("t-msg"); Nisaba.log.debug("d-msg"); Nisaba.log.info("i-msg");
            Nisaba.log.warn("w-msg"); Nisaba.log.error("e-msg");
            Nisaba.log.info({ not: "a string" }); Nisaba.log.info(undefined);
            console.log("c-log", { n: 1 }, [2]); console.warn("c-warn");
            console.error("c-error"); console.debug("c-debug");
            const cyclic = {}; cyclic.self = cyclic; console.info("c-cyclic", cyclic, 10n);
            return [];
        "#))
            .unwrap();
    });

    let out = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    for (level, msg) in [
        ("TRACE", "t-msg"),
        ("DEBUG", "d-msg"),
        ("INFO", "i-msg"),
        ("WARN", "w-msg"),
        ("ERROR", "e-msg"),
        ("INFO", "[object Object]"),
        ("INFO", "undefined"),
        // console.* lands in the same place, arguments joined and objects as JSON.
        ("INFO", r#"c-log {"n":1} [2]"#),
        ("WARN", "c-warn"),
        ("ERROR", "c-error"),
        ("DEBUG", "c-debug"),
        ("INFO", "c-cyclic [object Object] 10"),
    ] {
        assert!(
            out.lines()
                .any(|l| l.contains(level) && l.contains("vendor_plugin") && l.ends_with(msg)),
            "no {level} line for {msg:?} in:\n{out}"
        );
    }
}
