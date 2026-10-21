//! Incremental vendor sync (N-7.4): `fetchListings(config, { since, cursor })`
//! and the two result shapes the host accepts.
//!
//! The host-side merge into the cached catalog lives in
//! `nisaba_core::Db::merge_vendor_listings_cache` and is tested there; these
//! pin down the runtime contract: the plugin really receives `{ since,
//! cursor }`, a delta object parses as a delta, and a plugin that ignores the
//! arguments keeps full-pull semantics.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use nisaba_vendor_runtime::{
    FetchListingsArgs, FetchListingsResult, PluginLimits, PluginStore, StoreError, VendorRuntime,
};

/// In-memory `Nisaba.store` backend, mirroring `tests/store.rs`: entries are
/// keyed by (plugin_id, key) so no path serves one plugin's entries to another.
#[derive(Default)]
struct MemoryStore {
    data: Mutex<HashMap<(String, String), String>>,
}

impl PluginStore for MemoryStore {
    fn get(
        &self,
        plugin_id: &str,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, StoreError>> + '_>> {
        let value = self
            .data
            .lock()
            .unwrap()
            .get(&(plugin_id.to_string(), key.to_string()))
            .cloned();
        Box::pin(async move { Ok(value) })
    }

    fn set(
        &self,
        plugin_id: &str,
        key: &str,
        value: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + '_>> {
        self.data
            .lock()
            .unwrap()
            .insert((plugin_id.to_string(), key.to_string()), value.to_string());
        Box::pin(async move { Ok(()) })
    }

    fn delete(
        &self,
        plugin_id: &str,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, StoreError>> + '_>> {
        let removed = self
            .data
            .lock()
            .unwrap()
            .remove(&(plugin_id.to_string(), key.to_string()))
            .is_some();
        Box::pin(async move { Ok(removed) })
    }

    fn total_bytes(
        &self,
        plugin_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<u64, StoreError>> + '_>> {
        let total: u64 = self
            .data
            .lock()
            .unwrap()
            .iter()
            .filter(|((p, _), _)| p == plugin_id)
            .map(|((_, k), v)| k.len() as u64 + v.len() as u64)
            .sum();
        Box::pin(async move { Ok(total) })
    }
}

const METADATA: &str = r#"export const metadata = { name: "fixture", version: "1.0.0" };"#;

/// A supplier with a delta endpoint: a full catalog on the first run (when
/// `since` is null), and only what changed on later runs.
const INCREMENTAL_PLUGIN: &str = r#"
export async function fetchListings(config, args) {
  if (args.since == null) {
    return [
      { vendor_item_id: "a", title: "Apple" },
      { vendor_item_id: "b", title: "Banana" },
    ];
  }
  return {
    changes: [{ vendor_item_id: "a", title: "Apple (new harvest)" }],
    deleted: ["b"],
    cursor: `${args.since}|${args.cursor}`,
  };
}
"#;

async fn run_incremental(
    plugin_body: &str,
    args: &FetchListingsArgs,
) -> anyhow::Result<FetchListingsResult> {
    let code = format!("{METADATA}\n{plugin_body}");
    let files = HashMap::from([("index.ts".to_string(), code)]);
    let store: Arc<dyn PluginStore> = Arc::new(MemoryStore::default());
    VendorRuntime::execute_plugin_from_files_with_store_incremental(
        &files,
        "plug",
        store,
        HashMap::new(),
        None,
        PluginLimits::default(),
        args,
    )
    .await
}

/// The acceptance test: a second run with `{ since, cursor }` changes one
/// listing, deletes another, and returns a new cursor — and the plugin saw
/// exactly the `since`/`cursor` the host sent.
#[tokio::test]
async fn incremental_second_run_changes_one_listing() {
    // First run: no incremental state — a full catalog.
    let first = run_incremental(INCREMENTAL_PLUGIN, &FetchListingsArgs::default())
        .await
        .unwrap();
    let listings = match first {
        FetchListingsResult::Full(listings) => listings,
        FetchListingsResult::Delta(_) => panic!("first run must be a full catalog"),
    };
    assert_eq!(listings.len(), 2);
    assert_eq!(listings[0].vendor_item_id, "a");
    assert_eq!(listings[1].vendor_item_id, "b");

    // Second run: the host hands back its recorded state.
    let args = FetchListingsArgs {
        since: Some("2026-10-21 08:00:00".to_string()),
        cursor: Some("c1".to_string()),
    };
    let second = run_incremental(INCREMENTAL_PLUGIN, &args).await.unwrap();
    let delta = match second {
        FetchListingsResult::Delta(delta) => delta,
        FetchListingsResult::Full(_) => panic!("second run must be a delta"),
    };
    assert_eq!(delta.changes.len(), 1);
    assert_eq!(delta.changes[0].vendor_item_id, "a");
    assert_eq!(delta.changes[0].title, "Apple (new harvest)");
    assert_eq!(delta.deleted, vec!["b".to_string()]);
    // The cursor echoes what the plugin received: proof the host's `{ since,
    // cursor }` reached `fetchListings` intact.
    assert_eq!(
        delta.cursor.as_deref(),
        Some("2026-10-21 08:00:00|c1"),
        "plugin must receive the exact since and cursor the host sent"
    );
}

/// A plugin written before N-7.4 ignores the second argument entirely; the new
/// entry point must still hand it the full catalog with replace semantics.
#[tokio::test]
async fn plugin_ignoring_args_keeps_full_pull_semantics() {
    let plugin = r#"
export async function fetchListings(config) {
  return [{ vendor_item_id: "x", title: "Xylophone" }];
}
"#;
    let args = FetchListingsArgs {
        since: Some("2026-10-21 08:00:00".to_string()),
        cursor: Some("c1".to_string()),
    };
    let result = run_incremental(plugin, &args).await.unwrap();
    match result {
        FetchListingsResult::Full(listings) => {
            assert_eq!(listings.len(), 1);
            assert_eq!(listings[0].vendor_item_id, "x");
        }
        FetchListingsResult::Delta(_) => panic!("ignoring plugin must stay full-pull"),
    }
}

/// `since: null` must arrive as a real null the plugin can branch on — not a
/// missing property or an empty string.
#[tokio::test]
async fn first_run_hands_null_since_and_cursor() {
    let plugin = r#"
export async function fetchListings(config, args) {
  if (args.since !== null || args.cursor !== null) {
    throw new Error(`expected nulls, got since=${args.since} cursor=${args.cursor}`);
  }
  return [];
}
"#;
    let result = run_incremental(plugin, &FetchListingsArgs::default())
        .await
        .unwrap();
    assert!(matches!(result, FetchListingsResult::Full(listings) if listings.is_empty()));
}

/// N-7.5: the logged incremental entry point returns the plugin's own log
/// lines for the run history — including for a run that fails.
#[tokio::test]
async fn logged_run_returns_the_plugins_log_lines() {
    let code = format!(
        "{METADATA}\n{}",
        r#"
export async function fetchListings(config, args) {
  Nisaba.log.info("starting pull");
  Nisaba.log.warn("supplier is slow");
  return [{ vendor_item_id: "a", title: "Apple" }];
}
"#
    );
    let files = HashMap::from([("index.ts".to_string(), code)]);
    let store: Arc<dyn PluginStore> = Arc::new(MemoryStore::default());
    let (result, logs) = VendorRuntime::execute_plugin_from_files_with_store_incremental_logged(
        &files,
        "plug",
        store,
        HashMap::new(),
        None,
        PluginLimits::default(),
        &FetchListingsArgs::default(),
    )
    .await;
    assert!(matches!(result, Ok(FetchListingsResult::Full(_))));
    assert_eq!(logs, ["[info] starting pull", "[warn] supplier is slow"]);

    // A failing run still hands back what the plugin logged before it died.
    let code = format!(
        "{METADATA}\n{}",
        r#"
export async function fetchListings(config, args) {
  Nisaba.log.error("about to fail");
  throw new Error("supplier down");
}
"#
    );
    let files = HashMap::from([("index.ts".to_string(), code)]);
    let store: Arc<dyn PluginStore> = Arc::new(MemoryStore::default());
    let (result, logs) = VendorRuntime::execute_plugin_from_files_with_store_incremental_logged(
        &files,
        "plug",
        store,
        HashMap::new(),
        None,
        PluginLimits::default(),
        &FetchListingsArgs::default(),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(logs, ["[error] about to fail"]);
}
