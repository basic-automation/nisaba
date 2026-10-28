//! One-shot loopback listener for signing in to a marketplace through the user's own browser.
//!
//! A marketplace login works far more reliably in the user's normal browser (its cookies,
//! fingerprint, password manager and passkeys) than in an embedded webview, which eBay's
//! edge sometimes rejects. The app opens the broker's authorize URL in that browser; after
//! sign-in the broker forwards `code` and `state` (and Amazon's `selling_partner_id`) to
//! `http://127.0.0.1:<port>/<marketplace>/callback`, where this listener is waiting. The
//! port rides in the `state` prefix, so the broker stays stateless and can only ever
//! redirect to loopback.

use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

/// How long the user has to finish signing in before the attempt is abandoned.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

const MAX_REQUEST_BYTES: usize = 8 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// The marketplace a sign-in is for: it names the callback path and the user-facing text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marketplace {
    Ebay,
    Amazon,
}

impl Marketplace {
    fn callback_path(self) -> &'static str {
        match self {
            Marketplace::Ebay => "/ebay/callback",
            Marketplace::Amazon => "/amazon/callback",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Marketplace::Ebay => "eBay",
            Marketplace::Amazon => "Amazon",
        }
    }
}

/// The broker's forward of a completed sign-in.
#[derive(Debug, PartialEq, Eq)]
pub struct Answer {
    pub code: String,
    pub state: Option<String>,
    /// Amazon's `selling_partner_id` for the seller who just authorized, when sent.
    pub seller_id: Option<String>,
}

/// What one HTTP request to the listener means.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    /// Not the callback (a favicon fetch, a probe, a wrong Host): answer 404, keep waiting.
    Ignore,
    /// The broker's forward of the marketplace's answer.
    Answer(Answer),
    /// The marketplace or the user declined; carries the reason to show.
    Denied(String),
}

/// Classify the head of an HTTP request. Only a `GET` of the callback path whose `Host`
/// is this loopback listener counts, so a web page cannot reach the listener through a
/// name that rebinds to 127.0.0.1.
pub fn parse_request(head: &str, port: u16, marketplace: Marketplace) -> Request {
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or("").split(' ');
    let (method, target) = (request_line.next(), request_line.next());
    if method != Some("GET") {
        return Request::Ignore;
    }
    let host_ok = lines.any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("host")
                && [format!("127.0.0.1:{port}"), format!("localhost:{port}")]
                    .iter()
                    .any(|allowed| value.trim().eq_ignore_ascii_case(allowed))
        })
    });
    if !host_ok {
        return Request::Ignore;
    }
    let Some(target) = target else {
        return Request::Ignore;
    };
    let Ok(url) = url::Url::parse(&format!("http://127.0.0.1:{port}{target}")) else {
        return Request::Ignore;
    };
    if url.path() != marketplace.callback_path() {
        return Request::Ignore;
    }
    let param = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    if let Some(error) = param("error") {
        return Request::Denied(param("error_description").unwrap_or(error));
    }
    match param("code") {
        Some(code) if !code.is_empty() => Request::Answer(Answer {
            code,
            state: param("state"),
            seller_id: param("selling_partner_id"),
        }),
        _ => Request::Ignore,
    }
}

/// Why waiting ended without a usable answer.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// A newer attempt, or the user, took over: say nothing.
    Superseded,
    /// Show this to the user.
    Message(String),
}

/// Wait for the broker's forward of the marketplace's answer. `is_current` says whether this attempt
/// still owns the sign-in; `is_ours` says whether a returned `state` is the one issued, so
/// a stray local request with another state is refused and the wait goes on.
pub async fn wait_for_answer(
    listener: TcpListener,
    port: u16,
    marketplace: Marketplace,
    is_current: impl Fn() -> bool,
    is_ours: impl Fn(Option<&str>) -> bool,
    limit: Duration,
) -> Result<Answer, Failure> {
    let deadline = Instant::now() + limit;
    loop {
        if !is_current() {
            return Err(Failure::Superseded);
        }
        if Instant::now() >= deadline {
            return Err(Failure::Message(format!(
                "Sign-in timed out. Try Authorize with {} again.",
                marketplace.name()
            )));
        }
        let Ok(Ok((mut stream, _))) = timeout(POLL_INTERVAL, listener.accept()).await else {
            continue;
        };
        let head = read_head(&mut stream).await;
        match parse_request(&head, port, marketplace) {
            Request::Ignore => respond(&mut stream, "404 Not Found", "Not found.").await,
            Request::Denied(reason) => {
                respond(
                    &mut stream,
                    "200 OK",
                    "Sign-in was not completed. You can close this tab and return to Nisaba.",
                )
                .await;
                return Err(Failure::Message(format!(
                    "{} sign-in was declined: {reason}",
                    marketplace.name()
                )));
            }
            Request::Answer(answer) => {
                if !is_ours(answer.state.as_deref()) {
                    respond(
                        &mut stream,
                        "400 Bad Request",
                        "This sign-in is not for Nisaba.",
                    )
                    .await;
                    continue;
                }
                respond(
                    &mut stream,
                    "200 OK",
                    "Signed in. You can close this tab and return to Nisaba.",
                )
                .await;
                return Ok(answer);
            }
        }
    }
}

async fn read_head(stream: &mut TcpStream) -> String {
    let mut received = Vec::new();
    let mut chunk = [0u8; 1024];
    while received.len() < MAX_REQUEST_BYTES && !received.windows(4).any(|w| w == b"\r\n\r\n") {
        match timeout(READ_TIMEOUT, stream.read(&mut chunk)).await {
            Ok(Ok(n)) if n > 0 => received.extend_from_slice(&chunk[..n]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&received).into_owned()
}

/// A tiny page with no scripts, styles or links, and nothing cached or leaked as referrer.
async fn respond(stream: &mut TcpStream, status: &str, text: &str) {
    let body = format!("<!doctype html><meta charset=\"utf-8\"><title>Nisaba</title><p>{text}</p>");
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'none'\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(target: &str, host: &str) -> String {
        format!("GET {target} HTTP/1.1\r\nHost: {host}\r\nAccept: */*\r\n\r\n")
    }

    #[test]
    fn the_brokers_forward_is_an_answer() {
        let head = get(
            "/ebay/callback?code=abc%20123&state=54321.xyz",
            "127.0.0.1:54321",
        );
        assert_eq!(
            parse_request(&head, 54321, Marketplace::Ebay),
            Request::Answer(Answer {
                code: "abc 123".to_string(),
                state: Some("54321.xyz".to_string()),
                seller_id: None,
            })
        );
        // `localhost` is the same machine and is accepted too.
        let head = get("/ebay/callback?code=c&state=s", "localhost:54321");
        assert!(matches!(
            parse_request(&head, 54321, Marketplace::Ebay),
            Request::Answer(_)
        ));
    }

    #[test]
    fn a_decline_carries_its_reason() {
        let head = get(
            "/ebay/callback?error=access_denied&error_description=The+user+denied&state=s",
            "127.0.0.1:54321",
        );
        assert_eq!(
            parse_request(&head, 54321, Marketplace::Ebay),
            Request::Denied("The user denied".to_string())
        );
        let head = get("/ebay/callback?error=access_denied", "127.0.0.1:54321");
        assert_eq!(
            parse_request(&head, 54321, Marketplace::Ebay),
            Request::Denied("access_denied".to_string())
        );
    }

    #[test]
    fn anything_else_is_ignored() {
        let ok_host = "127.0.0.1:54321";
        for head in [
            get("/favicon.ico", ok_host),
            get("/", ok_host),
            get("/ebay/callback", ok_host),              // no code
            get("/ebay/callback?code=", ok_host),        // empty code
            get("/ebay/callback/extra?code=c", ok_host), // wrong path
            get("/ebay/callback?code=c", "evil.example:54321"), // DNS rebinding: wrong Host
            get("/ebay/callback?code=c", "127.0.0.1:9999"), // wrong port
            "POST /ebay/callback?code=c HTTP/1.1\r\nHost: 127.0.0.1:54321\r\n\r\n".to_string(),
            "GET /ebay/callback?code=c HTTP/1.1\r\n\r\n".to_string(), // no Host
            String::new(),
        ] {
            assert_eq!(
                parse_request(&head, 54321, Marketplace::Ebay),
                Request::Ignore,
                "{head:?}"
            );
        }
    }

    async fn send(port: u16, request: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        s.write_all(request.as_bytes()).await.expect("write");
        let mut out = String::new();
        let _ = s.read_to_string(&mut out).await;
        out
    }

    #[tokio::test]
    async fn waits_past_strays_and_a_foreign_state_for_the_real_answer() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let waiter = tokio::spawn(wait_for_answer(
            listener,
            port,
            Marketplace::Ebay,
            || true,
            |state| state == Some("ours"),
            Duration::from_secs(20),
        ));
        let host = format!("127.0.0.1:{port}");
        assert!(send(port, &get("/favicon.ico", &host))
            .await
            .starts_with("HTTP/1.1 404"));
        let foreign = send(port, &get("/ebay/callback?code=stolen&state=theirs", &host)).await;
        assert!(foreign.starts_with("HTTP/1.1 400"), "{foreign}");
        assert!(
            !waiter.is_finished(),
            "a foreign state must not end the wait"
        );
        let real = send(port, &get("/ebay/callback?code=REAL&state=ours", &host)).await;
        assert!(real.starts_with("HTTP/1.1 200"), "{real}");
        assert!(
            real.contains("Referrer-Policy: no-referrer") && real.contains("default-src 'none'")
        );
        assert_eq!(
            waiter.await.expect("join"),
            Ok(Answer {
                code: "REAL".to_string(),
                state: Some("ours".to_string()),
                seller_id: None,
            })
        );
    }

    #[tokio::test]
    async fn a_decline_ends_the_wait_with_its_reason() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let waiter = tokio::spawn(wait_for_answer(
            listener,
            port,
            Marketplace::Ebay,
            || true,
            |_| true,
            Duration::from_secs(20),
        ));
        let host = format!("127.0.0.1:{port}");
        send(
            port,
            &get(
                "/ebay/callback?error=access_denied&error_description=No",
                &host,
            ),
        )
        .await;
        assert_eq!(
            waiter.await.expect("join"),
            Err(Failure::Message(
                "eBay sign-in was declined: No".to_string()
            ))
        );
    }

    #[tokio::test]
    async fn gives_up_quietly_when_superseded_and_loudly_on_timeout() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        assert_eq!(
            wait_for_answer(
                listener,
                port,
                Marketplace::Ebay,
                || false,
                |_| true,
                Duration::from_secs(20)
            )
            .await,
            Err(Failure::Superseded)
        );
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        assert!(matches!(
            wait_for_answer(listener, port, Marketplace::Ebay, || true, |_| true, Duration::from_millis(1)).await,
            Err(Failure::Message(m)) if m.contains("timed out")
        ));
    }

    #[test]
    fn amazons_forward_carries_the_seller_and_uses_its_own_path() {
        let head = get(
            "/amazon/callback?code=abc&state=54321.xyz&selling_partner_id=A1B2C3",
            "127.0.0.1:54321",
        );
        assert_eq!(
            parse_request(&head, 54321, Marketplace::Amazon),
            Request::Answer(Answer {
                code: "abc".to_string(),
                state: Some("54321.xyz".to_string()),
                seller_id: Some("A1B2C3".to_string()),
            })
        );
        // Each marketplace's listener answers only its own path.
        assert_eq!(
            parse_request(&head, 54321, Marketplace::Ebay),
            Request::Ignore
        );
        let ebay = get("/ebay/callback?code=c&state=s", "127.0.0.1:54321");
        assert_eq!(
            parse_request(&ebay, 54321, Marketplace::Amazon),
            Request::Ignore
        );
    }

    #[tokio::test]
    async fn amazon_messages_name_amazon() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        assert!(matches!(
            wait_for_answer(listener, port, Marketplace::Amazon, || true, |_| true, Duration::from_millis(1)).await,
            Err(Failure::Message(m)) if m.contains("Amazon again")
        ));
    }
}
