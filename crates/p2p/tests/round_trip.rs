//! Full-state P2P sync without Tor: peer A's payload goes through the real wire format
//! (`SyncRequest` as JSON, exactly what `client::sync_with_peer` posts) and is merged into
//! peer B. The onion transport is the only thing left out.

use std::collections::HashMap;

use nisaba_core::db::Db;
use nisaba_core::types::{Product, ProductVariant, VendorPluginRow};
use nisaba_p2p::sync::{load_full_sync_payload, merge_remote_payload};
use nisaba_p2p::types::{MergeSummary, SyncPayload, SyncRequest};
use turso::params;

async fn peer(dir: &tempfile::TempDir, name: &str) -> Db {
    let db = Db::open(dir.path().join(name).to_str().unwrap())
        .await
        .unwrap();
    db.migrate().await.unwrap();
    db
}

/// Serialize a payload the way the client sends it and parse it the way the server does.
fn over_the_wire(payload: SyncPayload) -> SyncPayload {
    let request = SyncRequest {
        protocol_version: 1,
        sender_peer_id: "peer-a".to_string(),
        sender_onion: "a.onion".to_string(),
        sender_time: "2026-09-26T00:00:00Z".to_string(),
        payload,
    };
    let body = serde_json::to_string(&request).unwrap();
    serde_json::from_str::<SyncRequest>(&body).unwrap().payload
}

async fn exec(db: &Db, sql: &str, args: impl turso::IntoParams) {
    db.connect()
        .await
        .unwrap()
        .execute(sql, args)
        .await
        .unwrap();
}

fn product(id: &str, name: &str, quantity: i64) -> Product {
    Product {
        id: id.to_string(),
        canonical_sku: format!("SKU-{id}"),
        name: name.to_string(),
        quantity,
        low_stock_threshold: Some(2),
        is_tracked: true,
        has_variants: true,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

/// Peer A: one product with a variant, a mapping, a snapshot, a processed XMR order, a
/// platform token, the company config, sync metadata and a registry plugin.
async fn seed_peer_a(db: &Db) {
    db.insert_product(&product("p1", "Canteen", 3))
        .await
        .unwrap();
    db.insert_variant(&ProductVariant {
        id: "v1".to_string(),
        product_id: "p1".to_string(),
        sku: "CAN-OD".to_string(),
        name: "Olive".to_string(),
        attributes: HashMap::from([("Color".to_string(), "Olive".to_string())]),
        // Effective quantity is derived: on-hand, plus vendor stock for a dropship variant.
        quantity: 3,
        on_hand_quantity: 3,
        image_url: Some("https://img/1.jpg".to_string()),
        sort_order: 0,
        source_plugin_id: None,
        source_vendor_item_id: None,
        created_at: String::new(),
        updated_at: String::new(),
    })
    .await
    .unwrap();
    db.insert_mapping("p1", "ebay", "ITEM-1", Some("CAN-OD"), "v1")
        .await
        .unwrap();
    db.upsert_snapshot("p1", "ebay", 7, "2026-09-25 12:00:00")
        .await
        .unwrap();
    db.mark_xmr_order_processed("ord-1", Some("p1"), "L-1", 1)
        .await
        .unwrap();
    db.upsert_auth_token("ebay", "access-a", Some("refresh-a"), None, None)
        .await
        .unwrap();
    db.set_company_config(r#"{"ebay":{"enabled":true}}"#, "2026-09-25 12:00:00")
        .await
        .unwrap();
    db.set_p2p_meta("last_platform_sync_at", "2026-09-25 12:00:00")
        .await
        .unwrap();
    db.set_p2p_meta("last_platform_sync_by", "peer-a")
        .await
        .unwrap();
    db.upsert_registry_plugin(&VendorPluginRow {
        id: "plug".to_string(),
        plugin_file: "plug.zip".to_string(),
        display_name: "Plug".to_string(),
        description: String::new(),
        files_json: r#"{"index.ts":"export const metadata = {};"}"#.to_string(),
        version: "1.0.0".to_string(),
        config_fields: None,
        config_json: Some(r#"{"api_key":"k"}"#.to_string()),
        status: "approved".to_string(),
        submitted_by: None,
        approved_by: None,
        category: "vendor".to_string(),
        icon: None,
        include_vendor_stock: false,
        created_at: "2026-09-25 12:00:00".to_string(),
        updated_at: "2026-09-25 12:00:00".to_string(),
        allowed_hosts_json: Some(r#"["api.example.com"]"#.to_string()),
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_full_payload_round_trips_into_an_empty_peer() {
    let dir = tempfile::tempdir().unwrap();
    let a = peer(&dir, "a.db").await;
    let b = peer(&dir, "b.db").await;
    seed_peer_a(&a).await;

    let summary = merge_remote_payload(
        &b,
        &over_the_wire(load_full_sync_payload(&a).await.unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(summary.products_created, 1);
    assert_eq!(summary.variants_updated, 1);
    assert_eq!(summary.mappings_updated, 1);
    assert_eq!(summary.snapshots_updated, 1);
    assert_eq!(summary.orders_added, 1);
    assert_eq!(summary.vendor_plugins_updated, 1);

    let p = b.get_product("p1").await.unwrap().expect("product merged");
    assert_eq!(
        (p.name.as_str(), p.canonical_sku.as_str()),
        ("Canteen", "SKU-p1")
    );
    assert_eq!(
        (p.low_stock_threshold, p.is_tracked, p.has_variants),
        (Some(2), true, true)
    );

    let v = b.get_variant("v1").await.unwrap().expect("variant merged");
    assert_eq!(
        (v.sku.as_str(), v.quantity, v.on_hand_quantity),
        ("CAN-OD", 3, 3)
    );
    assert_eq!(v.attributes["Color"], "Olive");
    assert_eq!(v.image_url.as_deref(), Some("https://img/1.jpg"));

    let mappings = b.list_all_mappings_for_sync().await.unwrap();
    assert_eq!(mappings.len(), 1);
    let (_, product_id, platform, item, sku, active, deleted_at, _, variant) = &mappings[0];
    assert_eq!(
        (product_id.as_str(), platform.as_str(), item.as_str()),
        ("p1", "ebay", "ITEM-1")
    );
    assert_eq!(
        (sku.as_deref(), variant.as_deref()),
        (Some("CAN-OD"), Some("v1"))
    );
    assert!(*active && deleted_at.is_none());

    let snap = b
        .get_snapshot("p1", "ebay")
        .await
        .unwrap()
        .expect("snapshot merged");
    assert_eq!(snap.last_known_quantity, 7);
    assert!(b.is_xmr_order_processed("ord-1").await.unwrap());

    // Platform credentials travel in the payload. (In cleartext inside the Tor stream —
    // there is no application-layer encryption; see ROADMAP.md Phase 5.)
    let token = b
        .get_auth_token("ebay")
        .await
        .unwrap()
        .expect("token merged");
    assert_eq!(token.access_token, "access-a");
    assert_eq!(token.refresh_token.as_deref(), Some("refresh-a"));

    let (config, _) = b
        .get_company_config()
        .await
        .unwrap()
        .expect("config merged");
    assert_eq!(config, r#"{"ebay":{"enabled":true}}"#);
    assert_eq!(
        b.get_p2p_meta("last_platform_sync_by")
            .await
            .unwrap()
            .as_deref(),
        Some("peer-a")
    );

    let plugin = b
        .get_registry_plugin("plug")
        .await
        .unwrap()
        .expect("plugin merged");
    assert_eq!(plugin.config_json.as_deref(), Some(r#"{"api_key":"k"}"#));
    assert_eq!(
        plugin.allowed_hosts_json, None,
        "a peer's claim about a plugin's network access is not trusted"
    );
}

#[tokio::test]
async fn merging_the_same_payload_twice_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let a = peer(&dir, "a.db").await;
    let b = peer(&dir, "b.db").await;
    seed_peer_a(&a).await;

    let payload = over_the_wire(load_full_sync_payload(&a).await.unwrap());
    merge_remote_payload(&b, &payload).await.unwrap();
    let again = merge_remote_payload(&b, &payload).await.unwrap();
    assert_eq!(
        serde_json::to_value(&again).unwrap(),
        serde_json::to_value(MergeSummary::default()).unwrap(),
        "a repeated sync must be a no-op"
    );
}

#[tokio::test]
async fn the_newer_edit_wins_in_both_directions() {
    let dir = tempfile::tempdir().unwrap();
    let a = peer(&dir, "a.db").await;
    let b = peer(&dir, "b.db").await;
    a.insert_product(&product("p1", "from A", 5)).await.unwrap();
    b.insert_product(&product("p1", "from B", 9)).await.unwrap();

    // B's edit is newer: A's older payload must not overwrite it.
    exec(
        &a,
        "UPDATE products SET updated_at = '2026-09-25 10:00:00' WHERE id = 'p1'",
        (),
    )
    .await;
    exec(
        &b,
        "UPDATE products SET updated_at = '2026-09-25 11:00:00' WHERE id = 'p1'",
        (),
    )
    .await;
    let summary = merge_remote_payload(
        &b,
        &over_the_wire(load_full_sync_payload(&a).await.unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(summary.products_updated, 0);
    assert_eq!(b.get_product("p1").await.unwrap().unwrap().name, "from B");

    // Now A edits after B: A wins.
    exec(
        &a,
        "UPDATE products SET name = ?1, updated_at = '2026-09-25 12:00:00' WHERE id = 'p1'",
        params!["from A, later"],
    )
    .await;
    let summary = merge_remote_payload(
        &b,
        &over_the_wire(load_full_sync_payload(&a).await.unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(summary.products_updated, 1);
    let merged = b.get_product("p1").await.unwrap().unwrap();
    assert_eq!(
        (merged.name.as_str(), merged.quantity),
        ("from A, later", 5)
    );
}

#[tokio::test]
async fn a_deleted_mapping_propagates_as_a_tombstone() {
    let dir = tempfile::tempdir().unwrap();
    let a = peer(&dir, "a.db").await;
    let b = peer(&dir, "b.db").await;
    seed_peer_a(&a).await;
    merge_remote_payload(
        &b,
        &over_the_wire(load_full_sync_payload(&a).await.unwrap()),
    )
    .await
    .unwrap();

    // A deletes the mapping later. `delete_mapping` stamps `datetime('now')`, which has
    // one-second resolution, so move B's copy into the past to keep the order strict.
    exec(
        &b,
        "UPDATE platform_mappings SET updated_at = '2000-01-01 00:00:00'",
        (),
    )
    .await;
    let id = a.list_all_mappings_for_sync().await.unwrap()[0].0;
    a.delete_mapping(id).await.unwrap();

    merge_remote_payload(
        &b,
        &over_the_wire(load_full_sync_payload(&a).await.unwrap()),
    )
    .await
    .unwrap();
    let mappings = b.list_all_mappings_for_sync().await.unwrap();
    assert_eq!(mappings.len(), 1, "the tombstone is kept, not dropped");
    assert!(
        mappings[0].6.is_some(),
        "B's mapping was not marked deleted"
    );
}

fn is_noop(summary: &MergeSummary) -> bool {
    serde_json::to_value(summary).unwrap() == serde_json::to_value(MergeSummary::default()).unwrap()
}

#[tokio::test]
async fn two_peers_converge_after_syncing_both_ways() {
    // Peers sync in both directions every interval. Once they hold the same data a sync
    // must change nothing — otherwise every cycle rewrites rows on both sides forever.
    let dir = tempfile::tempdir().unwrap();
    let a = peer(&dir, "a.db").await;
    let b = peer(&dir, "b.db").await;
    seed_peer_a(&a).await;
    // A's catalog was last edited yesterday, not in the same second as the first sync.
    exec(
        &a,
        "UPDATE products SET updated_at = '2026-09-25 12:00:00'",
        (),
    )
    .await;
    exec(
        &a,
        "UPDATE product_variants SET updated_at = '2026-09-25 12:00:00'",
        (),
    )
    .await;

    let mut rounds = Vec::new();
    for _ in 0..3 {
        let to_b = merge_remote_payload(
            &b,
            &over_the_wire(load_full_sync_payload(&a).await.unwrap()),
        )
        .await
        .unwrap();
        // Timestamps have one-second resolution; let "now" move on between rounds so a
        // row re-stamped during a merge really does look newer.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let to_a = merge_remote_payload(
            &a,
            &over_the_wire(load_full_sync_payload(&b).await.unwrap()),
        )
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        rounds.push((to_b, to_a));
    }
    let (last_to_b, last_to_a) = rounds.last().unwrap();
    assert!(
        is_noop(last_to_b) && is_noop(last_to_a),
        "peers never converge; per-round summaries: {rounds:#?}"
    );
}
