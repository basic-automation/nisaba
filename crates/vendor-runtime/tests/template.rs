//! `plugins/template` is the starting point for third-party plugins, so it has to work:
//! these run it end to end against a fake supplier API on loopback.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use nisaba_vendor_runtime::{BatchCallback, VendorRuntime};

fn template_source() -> String {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/template/index.ts");
    std::fs::read_to_string(path).unwrap()
}

/// The template aimed at a loopback server instead of api.example.com.
fn template_against(port: u16) -> HashMap<String, String> {
    let code = template_source()
        .replace(
            "https://api.example.com",
            &format!("http://127.0.0.1:{port}"),
        )
        .replace("'api.example.com'", "'127.0.0.1'");
    HashMap::from([("index.ts".to_string(), code)])
}

/// Every request the fake API saw: `(request line, Authorization header)`.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// A fake supplier API. Each request's line and Authorization header are recorded; the
/// response comes from `route(request_line, times_seen_before)`.
fn fake_api(
    route: impl Fn(&str, usize) -> (u16, &'static str, String) + Send + 'static,
) -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let request_line = request_line.trim_end().to_string();
            let mut auth = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim_end().is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("authorization:") {
                    auth = v.trim().to_string();
                }
            }
            let prior = log
                .lock()
                .unwrap()
                .iter()
                .filter(|(l, _): &&(String, String)| *l == request_line)
                .count();
            log.lock().unwrap().push((request_line.clone(), auth));
            let (status, extra_header, body) = route(&request_line, prior);
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: {}\r\n{extra_header}connection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    (port, seen)
}

fn config(api_key: &str) -> HashMap<String, String> {
    HashMap::from([("api_key".to_string(), api_key.to_string())])
}

#[tokio::test]
async fn the_template_metadata_is_complete() {
    let meta = VendorRuntime::read_metadata(&template_source())
        .await
        .unwrap();
    assert_eq!(meta.name, "Example Supplier");
    assert_eq!(
        meta.allowed_hosts,
        Some(vec!["api.example.com".to_string()])
    );
    let key = &meta.config_fields[0];
    assert_eq!(key.key, "api_key");
    assert!(key.required && key.secret);
}

#[tokio::test]
async fn the_template_imports_a_paged_catalog() {
    let (port, seen) = fake_api(|line, prior| {
        if line.starts_with("GET /v1/products?page=1 ") {
            // Rate-limited once, then served.
            if prior == 0 {
                return (429, "retry-after: 1\r\n", String::new());
            }
            return (
                200,
                "",
                r#"{"items": [
                    {"id": 1, "name": "Canteen", "url": "https://example.com/1",
                     "description": "1 qt", "image": "https://cdn/1.jpg",
                     "variants": [
                        {"sku": "C-OD", "price": 12.5, "stock": 4, "upc": "0001",
                         "options": {"Color": "Olive"}},
                        {"sku": "C-BK", "price": 12.5, "stock": 0,
                         "options": {"Color": "Black"}}]},
                    {"name": "missing id"}
                  ], "next_page": 2}"#
                    .to_string(),
            );
        }
        if line.starts_with("GET /v1/products?page=2 ") {
            return (
                200,
                "",
                r#"{"items": [{"id": 2, "name": "Poncho", "price": 30, "stock": 9}],
                    "next_page": null}"#
                    .to_string(),
            );
        }
        (404, "", String::new())
    });

    let batches = Arc::new(Mutex::new(0usize));
    let counter = batches.clone();
    let cb = BatchCallback(Box::new(move |_| *counter.lock().unwrap() += 1));

    let listings = VendorRuntime::execute_plugin_from_files(
        &template_against(port),
        config("k-123"),
        Some(cb),
    )
    .await
    .unwrap();

    let ids: Vec<&str> = listings.iter().map(|l| l.vendor_item_id.as_str()).collect();
    assert_eq!(
        ids,
        ["C-OD", "C-BK", "2"],
        "the malformed product is skipped, not fatal"
    );

    let olive = &listings[0];
    assert_eq!(olive.title, "Canteen");
    assert_eq!((olive.price, olive.quantity), (Some(12.5), Some(4)));
    assert_eq!(olive.group_key.as_deref(), Some("1"));
    assert_eq!(olive.variant_attributes["Color"], "Olive");
    assert_eq!(olive.extras["upc"], "0001");
    assert_eq!(olive.extras["description"], "1 qt");
    assert_eq!(olive.image_url.as_deref(), Some("https://cdn/1.jpg"));

    let poncho = &listings[2];
    assert_eq!(
        poncho.group_key, None,
        "a product without variants is its own listing"
    );
    assert_eq!((poncho.price, poncho.quantity), (Some(30.0), Some(9)));

    let seen = seen.lock().unwrap();
    let lines: Vec<&str> = seen
        .iter()
        .map(|(l, _)| l.split(' ').nth(1).unwrap())
        .collect();
    assert_eq!(
        lines,
        [
            "/v1/products?page=1",
            "/v1/products?page=1",
            "/v1/products?page=2"
        ]
    );
    assert!(seen.iter().all(|(_, auth)| auth == "bearer k-123"));
    // Three listings is below the template's batch size, so nothing was streamed.
    assert_eq!(*batches.lock().unwrap(), 0);
}

#[tokio::test]
async fn the_template_explains_a_missing_or_rejected_key() {
    let err = VendorRuntime::execute_plugin_from_files(&template_against(1), config("  "), None)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("Set an API key"), "{err:#}");

    let (port, _) = fake_api(|_, _| (401, "", String::new()));
    let err =
        VendorRuntime::execute_plugin_from_files(&template_against(port), config("bad"), None)
            .await
            .unwrap_err();
    assert!(
        format!("{err:#}").contains("rejected the API key"),
        "{err:#}"
    );
}
