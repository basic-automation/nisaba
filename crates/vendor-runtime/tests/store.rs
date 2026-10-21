//! `Nisaba.store` tests: the per-plugin key–value store behind the sandbox.
//!
//! These pin down the three properties the task requires: values persist
//! between runs, a plugin can never read another plugin's keys, and the size
//! cap is enforced. The backend is an in-memory map standing in for the
//! company database — the scoping and the cap live in the runtime and its
//! ops, not in any particular backend.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use nisaba_vendor_runtime::{
    PluginLimits, PluginStore, StoreError, VendorListing, VendorRuntime, PLUGIN_STORE_CAP_BYTES,
};

/// In-memory backend with the same scoping contract the database backend
/// must honour: every entry is keyed by (plugin_id, key), and there is no
/// path that serves one plugin's entries to another.
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

/// Run `body` as the whole of a plugin's `fetchListings`, with `plugin_id` as
/// its identity and `store` as the backend. Short limits keep the suite fast.
async fn run_with_store(
    store: &Arc<MemoryStore>,
    plugin_id: &str,
    body: &str,
) -> anyhow::Result<Vec<VendorListing>> {
    let code = format!(
        r#"
        export const metadata = {{ name: "storetest", version: "1.0.0" }};
        export async function fetchListings() {{
            {body}
            return [];
        }}
        "#
    );
    let files = HashMap::from([("index.ts".to_string(), code)]);
    VendorRuntime::execute_plugin_from_files_with_store(
        &files,
        plugin_id,
        store.clone(),
        HashMap::new(),
        None,
        PluginLimits::metadata(),
    )
    .await
}

#[tokio::test]
async fn store_set_then_get_round_trips_within_one_run() {
    let store = Arc::new(MemoryStore::default());
    run_with_store(
        &store,
        "plugin-a",
        r#"await Nisaba.store.set("cursor", "page-3");"#,
    )
    .await
    .unwrap();
    // Read it back in a fresh run of the same plugin: persistence between runs.
    let code = r#"
        export const metadata = { name: "storetest", version: "1.0.0" };
        export async function fetchListings() {
            const cursor = await Nisaba.store.get("cursor");
            if (cursor !== "page-3") throw new Error(`expected page-3, got ${cursor}`);
            return [];
        }
        "#;
    let files = HashMap::from([("index.ts".to_string(), code.to_string())]);
    VendorRuntime::execute_plugin_from_files_with_store(
        &files,
        "plugin-a",
        store,
        HashMap::new(),
        None,
        PluginLimits::metadata(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn store_get_of_a_missing_key_resolves_to_null() {
    let store = Arc::new(MemoryStore::default());
    run_with_store(
        &store,
        "plugin-a",
        r#"if (await Nisaba.store.get("never-set") !== null) throw new Error("expected null");"#,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn store_delete_reports_whether_the_key_existed() {
    let store = Arc::new(MemoryStore::default());
    run_with_store(
        &store,
        "plugin-a",
        r#"
        await Nisaba.store.set("temp", "x");
        if (await Nisaba.store.delete("temp") !== true) throw new Error("first delete must be true");
        if (await Nisaba.store.delete("temp") !== false) throw new Error("second delete must be false");
        if (await Nisaba.store.get("temp") !== null) throw new Error("key must be gone");
        "#,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_plugin_cannot_read_another_plugins_keys() {
    let store = Arc::new(MemoryStore::default());

    // Plugin A stores a secret under a key B will ask for too.
    run_with_store(
        &store,
        "plugin-a",
        r#"await Nisaba.store.set("cursor", "a-secret-cursor");"#,
    )
    .await
    .unwrap();

    // Plugin B reads nothing of A's, and its own write does not disturb A.
    run_with_store(
        &store,
        "plugin-b",
        r#"
        if (await Nisaba.store.get("cursor") !== null)
            throw new Error("plugin B read plugin A's key");
        await Nisaba.store.set("cursor", "b-cursor");
        "#,
    )
    .await
    .unwrap();

    // A's value is exactly what A wrote.
    run_with_store(
        &store,
        "plugin-a",
        r#"
        const cursor = await Nisaba.store.get("cursor");
        if (cursor !== "a-secret-cursor") throw new Error(`A's key was disturbed: ${cursor}`);
        "#,
    )
    .await
    .unwrap();

    // B deleting "its" key does not delete A's.
    run_with_store(
        &store,
        "plugin-b",
        r#"await Nisaba.store.delete("cursor");"#,
    )
    .await
    .unwrap();
    run_with_store(
        &store,
        "plugin-a",
        r#"
        if (await Nisaba.store.get("cursor") !== "a-secret-cursor")
            throw new Error("B's delete removed A's key");
        "#,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn store_set_past_the_cap_is_rejected() {
    let store = Arc::new(MemoryStore::default());

    // Fill the plugin's store to just under the cap through the backend,
    // the way many small writes across runs would.
    let pad = "p".repeat(PLUGIN_STORE_CAP_BYTES as usize - 64);
    store.set("capped-plugin", "pad", &pad).await.unwrap();

    // One more modest write would push it over: the run must fail, loudly.
    let err = run_with_store(
        &store,
        "capped-plugin",
        r#"await Nisaba.store.set("one-more", "y".repeat(128));"#,
    )
    .await
    .expect_err("a write past the cap must fail the run");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("store limit"),
        "the rejection must name the store limit, got: {msg}"
    );

    // The failed write left nothing behind.
    assert_eq!(store.get("capped-plugin", "one-more").await.unwrap(), None);

    // A write that fits still succeeds.
    run_with_store(
        &store,
        "capped-plugin",
        r#"await Nisaba.store.set("tiny", "ok");"#,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn store_key_limits_are_enforced() {
    let store = Arc::new(MemoryStore::default());
    // An empty key is rejected before it reaches the backend.
    let err = run_with_store(&store, "plugin-a", r#"await Nisaba.store.set("", "v");"#)
        .await
        .expect_err("an empty key must fail");
    assert!(format!("{err:#}").contains("non-empty string"));
    // Non-string values are rejected with a nudge toward JSON.stringify.
    let err = run_with_store(
        &store,
        "plugin-a",
        r#"await Nisaba.store.set("k", { not: "a string" });"#,
    )
    .await
    .expect_err("a non-string value must fail");
    assert!(format!("{err:#}").contains("JSON.stringify"));
}

#[tokio::test]
async fn store_is_unavailable_without_a_backend() {
    // The plain entry points (and metadata reads) install no store: touching
    // `Nisaba.store` there must fail with a clear message, not hang or panic.
    let code = r#"
        export const metadata = { name: "storetest", version: "1.0.0" };
        export async function fetchListings() {
            await Nisaba.store.get("k");
            return [];
        }
        "#;
    let err = VendorRuntime::execute_plugin(code, HashMap::new(), None)
        .await
        .expect_err("store access without a backend must fail");
    assert!(
        format!("{err:#}").contains("not available"),
        "unexpected error: {err:#}"
    );
}
