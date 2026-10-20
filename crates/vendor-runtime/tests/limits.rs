//! Resource limits: a plugin that spins, sleeps forever, allocates without bound, or
//! floods the host with output is stopped with an error — and the host survives it.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nisaba_vendor_runtime::{BatchCallback, PluginLimits, VendorListing, VendorRuntime};

const METADATA: &str = r#"export const metadata = { name: "limits", version: "1.0.0" };"#;

fn tight() -> PluginLimits {
    PluginLimits {
        timeout: Duration::from_millis(500),
        max_heap_bytes: 64 * 1024 * 1024,
        max_output_bytes: 4 * 1024,
        max_response_bytes: 16 * 1024,
    }
}

async fn run(
    body: &str,
    limits: PluginLimits,
    cb: Option<BatchCallback>,
) -> anyhow::Result<Vec<VendorListing>> {
    let files = HashMap::from([(
        "index.ts".to_string(),
        format!("{METADATA}\nexport async function fetchListings() {{ {body} }}"),
    )]);
    VendorRuntime::execute_plugin_from_files_with_limits(&files, HashMap::new(), cb, limits).await
}

fn assert_err_contains(result: anyhow::Result<Vec<VendorListing>>, needle: &str) {
    let err = result.expect_err("the plugin was not stopped");
    assert!(
        format!("{err:#}").contains(needle),
        "expected {needle:?}, got: {err:#}"
    );
}

#[tokio::test]
async fn a_busy_loop_is_stopped_at_the_time_limit() {
    let start = Instant::now();
    assert_err_contains(run("while (true) {}", tight(), None).await, "time limit");
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn a_busy_loop_that_swallows_exceptions_is_still_stopped() {
    // Termination is not an ordinary exception: try/catch cannot hold it off.
    assert_err_contains(
        run(
            "while (true) { try { while (true) {} } catch (e) {} }",
            tight(),
            None,
        )
        .await,
        "time limit",
    );
}

#[tokio::test]
async fn a_plugin_waiting_forever_is_stopped_at_the_time_limit() {
    let start = Instant::now();
    assert_err_contains(
        run("await Nisaba.sleep(1e9); return [];", tight(), None).await,
        "time limit",
    );
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn runaway_allocation_is_stopped_instead_of_aborting_the_process() {
    // Without a heap ceiling and a near-limit callback this hits V8's fatal OOM handler,
    // which kills the whole process — this test binary included.
    let limits = PluginLimits {
        timeout: Duration::from_secs(60),
        ..tight()
    };
    assert_err_contains(
        run(
            "const hoard = []; while (true) hoard.push(new Array(1e5).fill(1.5));",
            limits,
            None,
        )
        .await,
        "memory limit",
    );
}

#[tokio::test]
async fn output_beyond_the_budget_is_refused() {
    let seen = Arc::new(Mutex::new(Vec::<usize>::new()));
    let sink = seen.clone();
    let cb = BatchCallback(Box::new(move |json| sink.lock().unwrap().push(json.len())));

    // ~100-byte listings, 10 per batch: the 4 KiB budget admits a few batches, then the
    // plugin is refused — and catching the refusal does not reopen the budget.
    let result = run(
        r#"
        const batch = Array.from({ length: 10 }, (_, i) =>
            ({ vendor_item_id: String(i), title: "x".repeat(60) }));
        for (let i = 0; i < 1000; i++) {
            try { Nisaba.emitBatch(batch); } catch (e) {}
        }
        return batch;
        "#,
        tight(),
        Some(cb),
    )
    .await;
    assert_err_contains(result, "output limit");

    let delivered: usize = seen.lock().unwrap().iter().sum();
    assert!(!seen.lock().unwrap().is_empty(), "no batch got through");
    assert!(
        delivered <= 4 * 1024,
        "delivered {delivered} bytes past the budget"
    );
}

#[tokio::test]
async fn a_final_result_beyond_the_budget_is_refused() {
    assert_err_contains(
        run(
            r#"return Array.from({ length: 500 }, (_, i) => ({ vendor_item_id: String(i), title: "t" }));"#,
            tight(),
            None,
        )
        .await,
        "output limit",
    );
}

#[tokio::test]
async fn a_plugin_within_its_limits_is_unaffected() {
    let listings = run(
        r#"await Nisaba.sleep(10);
           return [{ vendor_item_id: "1", title: "fine" }];"#,
        tight(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(listings[0].title, "fine");
}

#[test]
fn the_same_thread_runs_plugins_normally_after_one_is_terminated() {
    // The host runs every plugin on a fresh current-thread runtime inside
    // `spawn_blocking`, so pool threads are reused: a termination must not poison them.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        assert_err_contains(run("while (true) {}", tight(), None).await, "time limit");
        let listings = run(
            r#"return [{ vendor_item_id: "after", title: "ok" }];"#,
            tight(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(listings[0].vendor_item_id, "after");
    });
}

#[tokio::test]
async fn a_metadata_read_cannot_hang_plugin_install() {
    // Top-level module code runs during the metadata read, which happens at install time
    // before the user has run anything. It is bounded by `PluginLimits::metadata()`.
    let start = Instant::now();
    let err = VendorRuntime::read_metadata(&format!("{METADATA}\nwhile (true) {{}}"))
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("time limit"), "{err:#}");
    let elapsed = start.elapsed();
    assert!(
        elapsed >= PluginLimits::metadata().timeout && elapsed < Duration::from_secs(30),
        "{elapsed:?}"
    );
}

/// Answer one request on loopback with a `size`-byte body, optionally without declaring
/// its length (so the client has to count while streaming).
fn serve_body(size: usize, declare_length: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let head = if declare_length {
            format!("HTTP/1.1 200 OK\r\ncontent-length: {size}\r\nconnection: close\r\n\r\n")
        } else {
            "HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n".to_string()
        };
        let _ = stream.write_all(head.as_bytes());
        // The client may hang up once it has seen enough; that is the point.
        let _ = stream.write_all(&vec![b'x'; size]);
    });
    url
}

async fn fetch_size(url: &str) -> anyhow::Result<Vec<VendorListing>> {
    run(
        &format!(
            r#"try {{ const r = await Nisaba.fetch("{url}");
                     return [{{ vendor_item_id: "len", title: String(r.text().length) }}]; }}
               catch (e) {{ return [{{ vendor_item_id: "err", title: e.message }}]; }}"#
        ),
        tight(),
        None,
    )
    .await
}

#[tokio::test]
async fn a_response_body_within_the_ceiling_is_delivered_whole() {
    let listings = fetch_size(&serve_body(16 * 1024, true)).await.unwrap();
    assert_eq!(
        (
            listings[0].vendor_item_id.as_str(),
            listings[0].title.as_str()
        ),
        ("len", "16384")
    );
}

#[tokio::test]
async fn an_oversized_response_body_is_refused() {
    for declare_length in [true, false] {
        let listings = fetch_size(&serve_body(16 * 1024 + 1, declare_length))
            .await
            .unwrap();
        assert_eq!(
            listings[0].vendor_item_id, "err",
            "declared: {declare_length}"
        );
        assert!(
            listings[0].title.contains("exceeds the plugin limit"),
            "{}",
            listings[0].title
        );
    }
}
