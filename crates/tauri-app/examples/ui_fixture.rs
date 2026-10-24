//! Seed a throwaway Nisaba install for UI smoke tests: a `config.toml` with every
//! platform disabled, an app database, and one company whose plugin registry covers each
//! network-access state the marketplace shows.
//!
//!     cargo run -p nisaba-tauri --example ui_fixture -- <empty-dir>
//!
//! The app picks up `./config.toml` from its working directory before anything else, so
//! launching it from `<dir>` never touches a real install. `scripts/ui-smoke.py` does
//! that inside a network namespace, so nothing it starts can reach the network either.
//!
//! `--with-sample-catalog` additionally seeds a small fictional product catalog (used
//! for README screenshots; never real customer data).

use std::collections::HashMap;
use std::path::Path;

use nisaba_core::db::Db;
use nisaba_core::types::{Product, ProductVariant, VendorPluginRow};

const COMPANY_ID: &str = "ui-fixture-company";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args
        .next()
        .expect("usage: ui_fixture <empty-dir> [--with-sample-catalog]");
    let with_catalog = args.any(|a| a == "--with-sample-catalog");
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir.join("companies")).unwrap();
    let dir = dir.canonicalize().unwrap();
    if dir.join("config.toml").exists() {
        panic!(
            "{} already holds a config.toml; use an empty directory",
            dir.display()
        );
    }

    let app_db_path = dir.join("nisaba.db");
    let company_db_path = dir.join("companies").join(format!("{COMPANY_ID}.db"));

    std::fs::write(
        dir.join("config.toml"),
        format!(
            r#"# Throwaway UI-test install. Every platform is disabled.
[general]
sync_schedule = "0 */600 * * * *"
max_retries = 0

[database]
path = {:?}

[ebay]
enabled = false

[squarespace]
enabled = false

[xmrbazaar]
enabled = false

[amazon]
enabled = false

[company]
enabled = false
"#,
            app_db_path.to_string_lossy()
        ),
    )
    .unwrap();

    let app_db = Db::open(app_db_path.to_str().unwrap()).await.unwrap();
    app_db.migrate().await.unwrap();
    let secret_hash = nisaba_core::crypto::hash_secret("ui-fixture-not-a-secret");
    app_db
        .register_company(
            COMPANY_ID,
            "UI Fixture Co",
            "admin",
            &secret_hash,
            company_db_path.to_str().unwrap(),
        )
        .await
        .unwrap();
    app_db
        .set_app_setting("active_company_id", COMPANY_ID)
        .await
        .unwrap();

    let db = Db::open(company_db_path.to_str().unwrap()).await.unwrap();
    db.migrate().await.unwrap();
    db.create_company(COMPANY_ID, "UI Fixture Co", &secret_hash, "admin")
        .await
        .unwrap();

    // One approved plugin per network-access state, plus an installed one. Every plugin's
    // code is inert: it declares no network and returns nothing, so the vendor sync that
    // runs at startup executes harmlessly.
    let inert = serde_json::json!({
        "index.ts": "export const metadata = { name: 'inert', version: '1.0.0', allowed_hosts: [] };\nexport async function fetchListings() { return []; }"
    })
    .to_string();
    for (id, name, allowed_hosts) in [
        (
            "restricted",
            "Fixture Restricted",
            Some(r#"["api.example.com","*.cdn.example.com"]"#),
        ),
        ("unrestricted", "Fixture Unrestricted", Some("null")),
        ("unknown", "Fixture From Peer", None),
        ("offline", "Fixture Offline", Some("[]")),
    ] {
        db.upsert_registry_plugin(&VendorPluginRow {
            id: id.to_string(),
            plugin_file: format!("{id}.zip"),
            display_name: name.to_string(),
            description: format!("UI fixture: {id} network access."),
            files_json: inert.clone(),
            version: "1.0.0".to_string(),
            config_fields: Some("[]".to_string()),
            config_json: None,
            status: "approved".to_string(),
            submitted_by: None,
            approved_by: None,
            category: "vendor".to_string(),
            icon: None,
            include_vendor_stock: false,
            created_at: "2026-09-26 00:00:00".to_string(),
            updated_at: "2026-09-26 00:00:00".to_string(),
            allowed_hosts_json: allowed_hosts.map(str::to_string),
            code_hash: None,
        })
        .await
        .unwrap();
    }
    db.install_plugin("offline").await.unwrap();
    db.set_plugin_timeout("offline", Some(120)).await.unwrap();

    if with_catalog {
        seed_sample_catalog(&db).await;
    }

    println!("{}", dir.display());
}

/// A small fictional catalog so README screenshots show a populated app.
/// Fictional sample data only — never real customer data.
async fn seed_sample_catalog(db: &Db) {
    const CTS: &str = "2026-10-01 12:00:00";
    let products = [
        (
            "p-mug",
            "MUG-AURORA-2PK",
            "Aurora Ceramic Mug — Set of 2",
            34,
            Some(5),
            false,
        ),
        (
            "p-bottle",
            "BTL-TRAIL-750",
            "Trailhead Insulated Bottle 750ml",
            58,
            Some(10),
            false,
        ),
        (
            "p-tote",
            "TOTE-HERITAGE",
            "Heritage Canvas Tote",
            3,
            Some(5),
            false,
        ),
        (
            "p-organizer",
            "DESK-WALNUT-ORG",
            "Walnut Desk Organizer",
            21,
            Some(5),
            false,
        ),
        (
            "p-candle",
            "CNDL-BEESWAX-AMB",
            "Beeswax Candle — Amber",
            0,
            Some(4),
            false,
        ),
        (
            "p-pillow",
            "PLW-LINEN",
            "Linen Throw Pillow Cover",
            26,
            Some(5),
            true,
        ),
    ];
    for (id, sku, name, qty, threshold, variants) in products {
        db.insert_product(&Product {
            id: id.to_string(),
            canonical_sku: sku.to_string(),
            name: name.to_string(),
            quantity: qty,
            low_stock_threshold: threshold,
            is_tracked: true,
            has_variants: variants,
            created_at: CTS.to_string(),
            updated_at: CTS.to_string(),
        })
        .await
        .unwrap();
    }
    for (i, (color, qty)) in [("Ivory", 12), ("Sage", 9), ("Terracotta", 5)]
        .iter()
        .enumerate()
    {
        let mut attributes = HashMap::new();
        attributes.insert("Color".to_string(), color.to_string());
        db.insert_variant(&ProductVariant {
            id: format!("v-pillow-{i}"),
            product_id: "p-pillow".to_string(),
            sku: format!("PLW-LINEN-{}", color.to_uppercase()),
            name: color.to_string(),
            attributes,
            quantity: *qty,
            on_hand_quantity: *qty,
            image_url: None,
            sort_order: i as i32,
            source_plugin_id: None,
            source_vendor_item_id: None,
            created_at: CTS.to_string(),
            updated_at: CTS.to_string(),
        })
        .await
        .unwrap();
    }
}
