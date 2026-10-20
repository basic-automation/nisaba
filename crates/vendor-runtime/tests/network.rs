//! The per-plugin fetch allowlist: a plugin declares `allowed_hosts` in its metadata and
//! `Nisaba.fetch` reaches those hosts and no others — redirects included. Every server
//! here is on loopback; nothing touches the network.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use nisaba_vendor_runtime::{FetchPolicy, VendorRuntime};

/// A loopback server that answers every connection with `response` and records whether
/// it was ever contacted.
struct Server {
    port: u16,
    contacted: Arc<AtomicBool>,
}

fn serve(response: impl Fn(u16) -> String + Send + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let contacted = Arc::new(AtomicBool::new(false));
    let flag = contacted.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            flag.store(true, Ordering::SeqCst);
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(response(port).as_bytes());
        }
    });
    Server { port, contacted }
}

fn ok(body: &'static str) -> impl Fn(u16) -> String {
    move |_| {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }
}

fn redirect_to(location: String) -> impl Fn(u16) -> String {
    move |_| {
        format!(
            "HTTP/1.1 302 Found\r\nlocation: {location}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        )
    }
}

/// Run a plugin with the given `allowed_hosts` literal (or none) that fetches `url` and
/// reports either the body or the error message.
async fn fetch_as(allowed_hosts: Option<&str>, url: &str) -> (String, String) {
    let hosts = allowed_hosts
        .map(|h| format!("allowed_hosts: {h},"))
        .unwrap_or_default();
    let code = format!(
        r#"export const metadata = {{ name: "net", version: "1.0.0", {hosts} }};
           export async function fetchListings() {{
               try {{
                   const r = await Nisaba.fetch("{url}");
                   return [{{ vendor_item_id: "ok", title: r.text() }}];
               }} catch (e) {{
                   return [{{ vendor_item_id: "err", title: e.message }}];
               }}
           }}"#
    );
    let listings = VendorRuntime::execute_plugin(&code, HashMap::new(), None)
        .await
        .unwrap();
    (
        listings[0].vendor_item_id.clone(),
        listings[0].title.clone(),
    )
}

#[tokio::test]
async fn an_allowlisted_host_is_reachable() {
    let server = serve(ok("hello"));
    let (kind, body) = fetch_as(
        Some(r#"["127.0.0.1"]"#),
        &format!("http://127.0.0.1:{}/", server.port),
    )
    .await;
    assert_eq!((kind.as_str(), body.as_str()), ("ok", "hello"));
}

#[tokio::test]
async fn a_host_off_the_allowlist_is_refused_before_any_connection() {
    let server = serve(ok("secret"));
    let (kind, msg) = fetch_as(
        Some(r#"["api.example.com"]"#),
        &format!("http://127.0.0.1:{}/", server.port),
    )
    .await;
    assert_eq!(kind, "err");
    assert!(msg.contains("not in the plugin's allowed_hosts"), "{msg}");
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        !server.contacted.load(Ordering::SeqCst),
        "the refused host was contacted"
    );
}

#[tokio::test]
async fn an_empty_allowlist_means_no_network() {
    let server = serve(ok("secret"));
    let (kind, _) = fetch_as(Some("[]"), &format!("http://127.0.0.1:{}/", server.port)).await;
    assert_eq!(kind, "err");
    assert!(!server.contacted.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_plugin_without_an_allowlist_is_unrestricted() {
    let server = serve(ok("legacy"));
    let (kind, body) = fetch_as(None, &format!("http://127.0.0.1:{}/", server.port)).await;
    assert_eq!((kind.as_str(), body.as_str()), ("ok", "legacy"));
}

#[tokio::test]
async fn a_redirect_off_the_allowlist_is_refused() {
    // `localhost` and `127.0.0.1` are the same machine but different host names, which is
    // exactly the distinction the allowlist draws.
    let target = serve(ok("secret"));
    let bouncer = serve(redirect_to(format!("http://localhost:{}/", target.port)));
    let (kind, msg) = fetch_as(
        Some(r#"["127.0.0.1"]"#),
        &format!("http://127.0.0.1:{}/", bouncer.port),
    )
    .await;
    assert_eq!(kind, "err", "followed the redirect: {msg}");
    assert!(msg.contains("redirect refused"), "{msg}");
    std::thread::sleep(Duration::from_millis(50));
    assert!(bouncer.contacted.load(Ordering::SeqCst));
    assert!(
        !target.contacted.load(Ordering::SeqCst),
        "the redirect target was contacted"
    );
}

#[tokio::test]
async fn a_redirect_within_the_allowlist_is_followed() {
    let target = serve(ok("arrived"));
    let bouncer = serve(redirect_to(format!("http://127.0.0.1:{}/", target.port)));
    let (kind, body) = fetch_as(
        Some(r#"["127.0.0.1"]"#),
        &format!("http://127.0.0.1:{}/", bouncer.port),
    )
    .await;
    assert_eq!((kind.as_str(), body.as_str()), ("ok", "arrived"));
}

#[tokio::test]
async fn an_endless_redirect_loop_is_cut_off() {
    let looper = serve(|port| {
        format!(
            "HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:{port}/again\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        )
    });
    let (kind, msg) = fetch_as(None, &format!("http://127.0.0.1:{}/", looper.port)).await;
    assert_eq!(kind, "err");
    assert!(msg.contains("too many redirects"), "{msg}");
}

#[tokio::test]
async fn top_level_code_cannot_fetch() {
    // Module top-level code runs during the metadata read — at install time, and again
    // before each run's allowlist is applied — so it gets no network at all.
    let server = serve(ok("install-time"));
    let meta = VendorRuntime::read_metadata(&format!(
        r#"let outcome = "not attempted";
           try {{ await Nisaba.fetch("http://127.0.0.1:{}/"); outcome = "fetched"; }}
           catch (e) {{ outcome = e.message; }}
           export const metadata = {{ name: "n", version: "1.0.0", description: outcome }};"#,
        server.port
    ))
    .await
    .unwrap();
    assert!(
        meta.description
            .contains("not available while reading plugin metadata"),
        "{}",
        meta.description
    );
    std::thread::sleep(Duration::from_millis(50));
    assert!(!server.contacted.load(Ordering::SeqCst));
}

#[tokio::test]
async fn metadata_reports_the_declared_allowlist() {
    let declared = VendorRuntime::read_metadata(
        r#"export const metadata = { name: "a", version: "1", allowed_hosts: ["www.rothco.com"] };"#,
    )
    .await
    .unwrap();
    assert_eq!(
        declared.allowed_hosts,
        Some(vec!["www.rothco.com".to_string()])
    );

    let undeclared =
        VendorRuntime::read_metadata(r#"export const metadata = { name: "b", version: "1" };"#)
            .await
            .unwrap();
    assert_eq!(undeclared.allowed_hosts, None);
}

#[test]
fn host_matching_rules() {
    let policy = FetchPolicy::AllowList(Arc::new(vec![
        "www.rothco.com".to_string(),
        "*.cdn.example.com".to_string(),
        "UPPER.example.org.".to_string(),
    ]));
    let allows = |url: &str| policy.allows(&reqwest::Url::parse(url).unwrap());

    assert!(allows("https://www.rothco.com/graphql"));
    assert!(allows("http://www.rothco.com:8443/x"));
    assert!(
        allows("https://WWW.Rothco.COM./x"),
        "case and a trailing dot are normalised"
    );
    assert!(
        !allows("https://rothco.com/"),
        "an exact entry does not cover the apex"
    );
    assert!(!allows("https://evil.www.rothco.com/"), "…or subdomains");
    assert!(
        !allows("https://www.rothco.com.evil.net/"),
        "suffix tricks do not match"
    );
    assert!(!allows("https://notwww.rothco.com/"));

    assert!(allows("https://img.cdn.example.com/a.png"));
    assert!(allows("https://a.b.cdn.example.com/"));
    assert!(
        !allows("https://cdn.example.com/"),
        "a wildcard does not cover its apex"
    );
    assert!(!allows("https://evilcdn.example.com/"));

    assert!(allows("https://upper.example.org/"));

    assert!(!allows("ftp://www.rothco.com/"), "only http and https");
    assert!(!allows("file:///etc/passwd"));
    assert!(!FetchPolicy::Deny.allows(&reqwest::Url::parse("https://www.rothco.com/").unwrap()));
    assert!(
        FetchPolicy::Unrestricted.allows(&reqwest::Url::parse("https://anything.test/").unwrap())
    );
    assert!(!FetchPolicy::Unrestricted.allows(&reqwest::Url::parse("data:text/plain,hi").unwrap()));
}

#[tokio::test]
async fn the_shipped_rothco_plugin_declares_its_only_host() {
    // Reads metadata only — executing it would call the real Rothco API.
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/rothco-wholesale");
    let files: HashMap<String, String> = ["index.ts", "types.ts"]
        .iter()
        .map(|f| (f.to_string(), std::fs::read_to_string(dir.join(f)).unwrap()))
        .collect();
    let meta = VendorRuntime::read_metadata_from_files(&files)
        .await
        .unwrap();
    assert_eq!(meta.name, "Rothco Wholesale");
    assert_eq!(meta.allowed_hosts, Some(vec!["www.rothco.com".to_string()]));
}
