//! Seed a throwaway Nisaba install for UI smoke tests: a `config.toml` with every
//! platform disabled, an app database, and one company whose plugin registry covers each
//! network-access state the marketplace shows.
//!
//!     cargo run -p nisaba-tauri --example ui_fixture -- <empty-dir>
//!
//! The app picks up `./config.toml` from its working directory before anything else, so
//! launching it from `<dir>` never touches a real install. `scripts/ui-smoke.py` does
//! that inside a network namespace, so nothing it starts can reach the network either.

use std::path::Path;

use nisaba_core::db::Db;
use nisaba_core::types::VendorPluginRow;

const COMPANY_ID: &str = "ui-fixture-company";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: ui_fixture <empty-dir>");
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
log_level = "info"
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
        })
        .await
        .unwrap();
    }
    db.install_plugin("offline").await.unwrap();
    db.set_plugin_timeout("offline", Some(120)).await.unwrap();

    println!("{}", dir.display());
}
