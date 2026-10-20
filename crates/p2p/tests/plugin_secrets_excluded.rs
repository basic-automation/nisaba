//! Plugin `secret: true` config values must never travel in the P2P sync
//! payload (either direction) and must never sit in `config_json` once the
//! keyring write path is used. Uses an in-memory secret store — no OS keyring,
//! no network.

use std::collections::HashMap;

use nisaba_core::crypto::InMemoryPluginSecrets;
use nisaba_core::db::Db;
use nisaba_core::types::VendorPluginRow;
use nisaba_p2p::sync::{load_full_sync_payload, merge_remote_payload};
use nisaba_p2p::types::{PlatformSyncMeta, SyncPayload, SyncableVendorPlugin};

const SECRET_FIELDS_JSON: &str = r#"[
    {"key":"api_key","label":"API key","required":true,"secret":true},
    {"key":"region","label":"Region","required":false,"secret":false}
]"#;

/// Deliberately fake; only used to prove a value does (not) appear somewhere.
const FAKE_SECRET: &str = "sk-test-fake-secret-001";

fn now_ts() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn plugin_row() -> VendorPluginRow {
    VendorPluginRow {
        id: "plug".to_string(),
        plugin_file: "plug.zip".to_string(),
        display_name: "Plug".to_string(),
        description: String::new(),
        files_json: r#"{"index.ts":""}"#.to_string(),
        version: "1.0.0".to_string(),
        config_fields: Some(SECRET_FIELDS_JSON.to_string()),
        // Legacy state: the secret is still sitting in config_json in plaintext.
        config_json: Some(format!(r#"{{"api_key":"{FAKE_SECRET}","region":"us"}}"#)),
        status: "approved".to_string(),
        submitted_by: None,
        approved_by: None,
        category: "vendor".to_string(),
        icon: None,
        include_vendor_stock: false,
        created_at: now_ts(),
        updated_at: now_ts(),
        allowed_hosts_json: None,
    }
}

async fn test_db(dir: &tempfile::TempDir, name: &str) -> Db {
    let db = Db::open(dir.path().join(name).to_str().unwrap())
        .await
        .unwrap();
    db.migrate().await.unwrap();
    db
}

/// Even a row that still carries a plaintext secret in `config_json` produces
/// a payload without it: the build path strips `secret: true` values.
#[tokio::test]
async fn sync_payload_never_carries_plugin_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let db = test_db(&dir, "a.db").await;
    db.upsert_registry_plugin(&plugin_row()).await.unwrap();

    let payload = load_full_sync_payload(&db).await.unwrap();
    assert_eq!(payload.vendor_plugins.len(), 1);
    let wire = serde_json::to_string(&payload.vendor_plugins).unwrap();
    assert!(
        !wire.contains(FAKE_SECRET),
        "P2P payload must not contain the plugin secret"
    );
    assert_eq!(
        payload.vendor_plugins[0].config_json.as_deref(),
        Some(r#"{"region":"us"}"#),
        "stripped config_json keeps the non-secret values"
    );
}

/// The full new write path: secrets go to the store, `config_json` and the
/// payload built from it are both clean.
#[tokio::test]
async fn config_json_and_payload_stay_clean_after_keyring_write() {
    let dir = tempfile::tempdir().unwrap();
    let db = test_db(&dir, "a.db").await;
    db.upsert_registry_plugin(&plugin_row()).await.unwrap();

    let store = InMemoryPluginSecrets::new();
    let config = HashMap::from([
        ("api_key".to_string(), FAKE_SECRET.to_string()),
        ("region".to_string(), "us".to_string()),
    ]);
    db.set_plugin_config_split("plug", &config, &store)
        .await
        .unwrap();

    // config_json, serialised, contains no secret value…
    let raw = db
        .get_registry_plugin("plug")
        .await
        .unwrap()
        .unwrap()
        .config_json
        .unwrap();
    let raw_wire = serde_json::to_string(&raw).unwrap();
    assert!(
        !raw_wire.contains(FAKE_SECRET),
        "config_json must not contain the plugin secret: {raw}"
    );

    // …and neither does the P2P payload built from it.
    let payload = load_full_sync_payload(&db).await.unwrap();
    let wire = serde_json::to_string(&payload.vendor_plugins).unwrap();
    assert!(
        !wire.contains(FAKE_SECRET),
        "P2P payload must not contain the plugin secret"
    );

    // The secret itself still reaches plugin runs from the store.
    let merged = db.get_plugin_config_merged("plug", &store).await.unwrap();
    assert_eq!(merged.get("api_key").map(String::as_str), Some(FAKE_SECRET));
}

/// A hostile peer's secrets are stripped before its plugin rows are stored.
#[tokio::test]
async fn incoming_plugin_secrets_are_stripped_before_storage() {
    let dir = tempfile::tempdir().unwrap();
    let db = test_db(&dir, "b.db").await;

    let hostile = SyncableVendorPlugin {
        id: "plug".to_string(),
        plugin_file: "plug.zip".to_string(),
        display_name: "Plug".to_string(),
        description: String::new(),
        files_json: r#"{"index.ts":""}"#.to_string(),
        code: String::new(),
        version: "1.0.0".to_string(),
        config_fields: Some(SECRET_FIELDS_JSON.to_string()),
        config_json: Some(format!(r#"{{"api_key":"{FAKE_SECRET}","region":"us"}}"#)),
        status: "approved".to_string(),
        submitted_by: None,
        approved_by: None,
        category: "vendor".to_string(),
        icon: None,
        include_vendor_stock: false,
        updated_at: now_ts(),
    };
    let payload = SyncPayload {
        products: vec![],
        platform_mappings: vec![],
        platform_snapshots: vec![],
        xmr_processed_orders: vec![],
        platform_sync_meta: PlatformSyncMeta::default(),
        company_config: None,
        auth_tokens: vec![],
        company_logo: None,
        vendor_plugins: vec![hostile],
        product_variants: vec![],
    };

    let summary = merge_remote_payload(&db, &payload).await.unwrap();
    assert_eq!(summary.vendor_plugins_updated, 1);

    let stored = db
        .get_registry_plugin("plug")
        .await
        .unwrap()
        .expect("plugin merged");
    let stored_json = stored.config_json.unwrap();
    assert!(
        !stored_json.contains(FAKE_SECRET),
        "peer-sent secrets must be stripped before storage: {stored_json}"
    );
    assert_eq!(stored_json, r#"{"region":"us"}"#);
}
