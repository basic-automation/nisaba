use std::collections::HashMap;

use chrono::{DateTime, NaiveDateTime, Utc};
use tracing::{debug, info, warn};
use turso::params;
use turso::params::Params;

use nisaba_core::db::Db;
use nisaba_core::types::{Platform, PlatformSnapshot, Product, ProductVariant};

use crate::error::P2PError;
use crate::types::{
    CompanyConfigPayload, CompanyLogoPayload, MergeSummary, PlatformSyncMeta, SyncPayload,
    SyncableAuthToken, SyncableMapping, SyncableVariant, SyncableVendorPlugin,
};

/// How far ahead of our clock a peer's timestamp may be — clock skew — before we refuse
/// the row it stamps.
const MAX_CLOCK_SKEW_MINUTES: i64 = 10;

/// Whether a peer-supplied timestamp can be trusted as a last-writer-wins clock.
///
/// Merges compare timestamps as strings, so a row stamped far in the future (or with
/// anything that sorts after a digit, like `"Z"`) would win every later merge and could
/// never be corrected. Accepts SQLite's `YYYY-MM-DD HH:MM:SS` and RFC 3339, up to
/// [`MAX_CLOCK_SKEW_MINUTES`] ahead of now.
fn plausible_timestamp(ts: &str, now: DateTime<Utc>) -> bool {
    let parsed = DateTime::parse_from_rfc3339(ts)
        .map(|t| t.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|t| t.and_utc())
        });
    parsed.is_some_and(|t| t <= now + chrono::Duration::minutes(MAX_CLOCK_SKEW_MINUTES))
}

/// Refuse a remote row whose timestamp cannot be trusted, counting and logging it.
fn implausible(
    summary: &mut MergeSummary,
    now: DateTime<Utc>,
    what: &str,
    id: &str,
    ts: &str,
) -> bool {
    if plausible_timestamp(ts, now) {
        return false;
    }
    warn!(
        what,
        id,
        timestamp = ts,
        "Refusing remote row with an implausible timestamp"
    );
    summary.rejected_timestamps += 1;
    true
}

type MappingKey = (String, String, String);
type MappingMeta = (i64, Option<String>, String);

/// Load the full sync payload from the local database.
pub async fn load_full_sync_payload(db: &Db) -> Result<SyncPayload, P2PError> {
    let products = db.list_products().await?;
    let snapshots = db.list_all_snapshots().await?;
    let orders = db.list_xmr_processed_orders().await?;

    // Load all mappings including soft-deleted
    let raw_mappings = db.list_all_mappings_for_sync().await?;
    let platform_mappings: Vec<SyncableMapping> = raw_mappings
        .into_iter()
        .map(
            |(
                _,
                product_id,
                platform,
                platform_item_id,
                platform_sku,
                is_active,
                deleted_at,
                updated_at,
                variant_id,
            )| {
                SyncableMapping {
                    product_id,
                    platform: Platform::from_str_loose(&platform).unwrap_or(Platform::Ebay),
                    platform_item_id,
                    platform_sku,
                    is_active,
                    deleted_at,
                    updated_at,
                    variant_id,
                }
            },
        )
        .collect();

    // Load product variants
    let all_variants = db.list_all_variants().await.unwrap_or_default();
    let product_variants: Vec<SyncableVariant> = all_variants
        .into_iter()
        .map(|v| {
            let attrs_json =
                serde_json::to_string(&v.attributes).unwrap_or_else(|_| "{}".to_string());
            SyncableVariant {
                id: v.id,
                product_id: v.product_id,
                sku: v.sku,
                name: v.name,
                attributes_json: attrs_json,
                quantity: v.quantity,
                on_hand_quantity: v.on_hand_quantity,
                image_url: v.image_url,
                sort_order: v.sort_order,
                updated_at: v.updated_at,
                source_plugin_id: v.source_plugin_id,
                source_vendor_item_id: v.source_vendor_item_id,
            }
        })
        .collect();

    // Load platform sync meta
    let last_platform_sync_at = db.get_p2p_meta("last_platform_sync_at").await?;
    let last_platform_sync_by = db.get_p2p_meta("last_platform_sync_by").await?;

    // Load company config (cleartext for transit)
    let company_config = db
        .get_company_config()
        .await?
        .map(|(config_json, updated_at)| CompanyConfigPayload {
            config_json,
            updated_at,
        });

    // Load auth tokens
    let mut auth_tokens = Vec::new();
    for platform_str in &["ebay", "squarespace", "xmrbazaar"] {
        if let Ok(Some(token)) = db.get_auth_token(platform_str).await {
            auth_tokens.push(SyncableAuthToken {
                platform: platform_str.to_string(),
                access_token: token.access_token,
                refresh_token: token.refresh_token,
                expires_at: token.expires_at,
                cookies: token.cookies,
                updated_at: token.updated_at,
            });
        }
    }

    // Load company logo
    let company_logo = db
        .get_company_logo()
        .await?
        .map(|(logo, updated_at)| CompanyLogoPayload { logo, updated_at });

    // Load vendor plugin registry
    let vendor_plugins: Vec<SyncableVendorPlugin> = match db.list_registry_plugins().await {
        Ok(plugins) => plugins
            .into_iter()
            .map(|p| {
                // `secret: true` values never leave this machine: they live in
                // the OS keyring and are stripped from the payload even if a
                // row somehow still carries them in config_json.
                let config_json = nisaba_core::db::strip_secret_values(
                    p.config_fields.as_deref(),
                    p.config_json.as_deref(),
                );
                SyncableVendorPlugin {
                    id: p.id,
                    plugin_file: p.plugin_file,
                    display_name: p.display_name,
                    description: p.description,
                    files_json: p.files_json,
                    code: String::new(),
                    version: p.version,
                    config_fields: p.config_fields,
                    config_json,
                    status: p.status,
                    submitted_by: p.submitted_by,
                    approved_by: p.approved_by,
                    category: p.category,
                    icon: p.icon,
                    include_vendor_stock: p.include_vendor_stock,
                    updated_at: p.updated_at,
                }
            })
            .collect(),
        Err(_) => Vec::new(), // table may not exist on old schemas
    };

    Ok(SyncPayload {
        products,
        platform_mappings,
        platform_snapshots: snapshots,
        xmr_processed_orders: orders,
        platform_sync_meta: PlatformSyncMeta {
            last_platform_sync_at,
            last_platform_sync_by,
        },
        company_config,
        auth_tokens,
        company_logo,
        vendor_plugins,
        product_variants,
    })
}

/// Convert a `params!` value into an owned, cloneable [`Params`] for a planned
/// write in [`merge_remote_payload`]'s batch.
fn owned_params(p: impl turso::params::IntoParams) -> Params {
    p.into_params().expect("param conversion should not fail")
}

/// Merge a remote peer's payload into our local database.
/// Returns a summary of what changed.
///
/// The merge is planned in two phases so it applies transactionally. Phase one
/// reads the local state and decides every write the payload implies, without
/// writing anything. Phase two applies the planned writes in a single
/// transaction ([`Db::execute_write_batch`]): if any write fails, the whole
/// merge rolls back and the database is left exactly as it was — instead of the
/// old behaviour, where each row was applied with `?` in its own transaction,
/// so the first error left a half-applied merge that failed the same way on
/// every later sync cycle. The only step after the transaction is recomputing
/// derived quantities for products whose variants changed; that stays
/// best-effort derived-data repair, as it was before.
pub async fn merge_remote_payload(db: &Db, remote: &SyncPayload) -> Result<MergeSummary, P2PError> {
    let mut summary = MergeSummary::default();
    let now = Utc::now();
    let mut writes: Vec<(String, Params)> = Vec::new();

    // ── Products: last-write-wins by updated_at ──────────────
    let local_products = db.list_products().await?;
    let local_map: HashMap<String, &Product> =
        local_products.iter().map(|p| (p.id.clone(), p)).collect();

    for remote_product in &remote.products {
        if implausible(
            &mut summary,
            now,
            "product",
            &remote_product.id,
            &remote_product.updated_at,
        ) {
            continue;
        }
        match local_map.get(&remote_product.id) {
            Some(local) => {
                if remote_product.updated_at > local.updated_at {
                    writes.push((
                        "UPDATE products SET canonical_sku = ?1, name = ?2, quantity = ?3,
                         low_stock_threshold = ?4, is_tracked = ?5, has_variants = ?6, updated_at = ?7
                         WHERE id = ?8"
                            .to_string(),
                        owned_params(params![
                            remote_product.canonical_sku.clone(),
                            remote_product.name.clone(),
                            remote_product.quantity,
                            remote_product.low_stock_threshold,
                            remote_product.is_tracked,
                            remote_product.has_variants,
                            remote_product.updated_at.clone(),
                            remote_product.id.clone()
                        ]),
                    ));
                    summary.products_updated += 1;
                    debug!(
                        product_id = %remote_product.id,
                        "Updated product from remote (newer updated_at)"
                    );
                }
            }
            None => {
                writes.push((
                    "INSERT INTO products (id, canonical_sku, name, quantity, low_stock_threshold, is_tracked, has_variants, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
                        .to_string(),
                    owned_params(params![
                        remote_product.id.clone(),
                        remote_product.canonical_sku.clone(),
                        remote_product.name.clone(),
                        remote_product.quantity,
                        remote_product.low_stock_threshold,
                        remote_product.is_tracked,
                        remote_product.has_variants,
                        remote_product.created_at.clone(),
                        remote_product.updated_at.clone()
                    ]),
                ));
                summary.products_created += 1;
                debug!(product_id = %remote_product.id, "Created product from remote");
            }
        }
    }

    // ── Platform Mappings: union by (product_id, platform, variant_id), propagate tombstones ──
    let local_raw_mappings = db.list_all_mappings_for_sync().await?;
    let local_mapping_map: HashMap<MappingKey, MappingMeta> = local_raw_mappings
        .iter()
        .map(
            |(id, product_id, platform, _, _, _, deleted_at, updated_at, variant_id)| {
                let vkey = variant_id.clone().unwrap_or_default();
                (
                    (product_id.clone(), platform.clone(), vkey),
                    (*id, deleted_at.clone(), updated_at.clone()),
                )
            },
        )
        .collect();

    for remote_mapping in &remote.platform_mappings {
        if implausible(
            &mut summary,
            now,
            "mapping",
            &remote_mapping.product_id,
            &remote_mapping.updated_at,
        ) {
            continue;
        }
        let vkey = remote_mapping.variant_id.clone().unwrap_or_default();
        let key = (
            remote_mapping.product_id.clone(),
            remote_mapping.platform.as_str().to_string(),
            vkey,
        );

        match local_mapping_map.get(&key) {
            Some((local_id, _local_deleted, local_updated)) => {
                if remote_mapping.updated_at > *local_updated {
                    writes.push((
                        "UPDATE platform_mappings SET platform_item_id = ?1, platform_sku = ?2,
                         is_active = ?3, deleted_at = ?4, updated_at = ?5, variant_id = ?6
                         WHERE id = ?7"
                            .to_string(),
                        owned_params(params![
                            remote_mapping.platform_item_id.clone(),
                            remote_mapping.platform_sku.clone(),
                            remote_mapping.is_active,
                            remote_mapping.deleted_at.clone(),
                            remote_mapping.updated_at.clone(),
                            remote_mapping.variant_id.clone(),
                            *local_id
                        ]),
                    ));
                    summary.mappings_updated += 1;
                }
            }
            None => {
                writes.push((
                    "INSERT OR IGNORE INTO platform_mappings (product_id, platform, platform_item_id, platform_sku, is_active, deleted_at, updated_at, variant_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
                        .to_string(),
                    owned_params(params![
                        remote_mapping.product_id.clone(),
                        remote_mapping.platform.as_str(),
                        remote_mapping.platform_item_id.clone(),
                        remote_mapping.platform_sku.clone(),
                        remote_mapping.is_active,
                        remote_mapping.deleted_at.clone(),
                        remote_mapping.updated_at.clone(),
                        remote_mapping.variant_id.clone()
                    ]),
                ));
                summary.mappings_updated += 1;
            }
        }
    }

    // ── Platform Snapshots: newer last_polled_at wins ─────────
    let local_snapshots = db.list_all_snapshots().await?;
    let local_snap_map: HashMap<(String, String), &PlatformSnapshot> = local_snapshots
        .iter()
        .map(|s| ((s.product_id.clone(), s.platform.as_str().to_string()), s))
        .collect();

    for remote_snap in &remote.platform_snapshots {
        if implausible(
            &mut summary,
            now,
            "snapshot",
            &remote_snap.product_id,
            &remote_snap.last_polled_at,
        ) {
            continue;
        }
        let key = (
            remote_snap.product_id.clone(),
            remote_snap.platform.as_str().to_string(),
        );
        let should_apply = match local_snap_map.get(&key) {
            Some(local) => remote_snap.last_polled_at > local.last_polled_at,
            None => true,
        };
        if should_apply {
            // The same upsert `Db::upsert_snapshot` runs.
            writes.push((
                "INSERT INTO platform_snapshots (product_id, platform, last_known_quantity, last_polled_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(product_id, platform) DO UPDATE
                 SET last_known_quantity = ?3, last_polled_at = ?4"
                    .to_string(),
                owned_params(params![
                    remote_snap.product_id.clone(),
                    remote_snap.platform.as_str(),
                    remote_snap.last_known_quantity,
                    remote_snap.last_polled_at.clone()
                ]),
            ));
            summary.snapshots_updated += 1;
        }
    }

    // ── XMR Processed Orders: union (insert if missing) ──────
    let local_orders = db.list_xmr_processed_orders().await?;
    let processed_orders: std::collections::HashSet<&str> =
        local_orders.iter().map(|o| o.order_id.as_str()).collect();

    for remote_order in &remote.xmr_processed_orders {
        if !processed_orders.contains(remote_order.order_id.as_str()) {
            // The same insert `Db::mark_xmr_order_processed` runs.
            writes.push((
                "INSERT OR IGNORE INTO xmr_processed_orders (order_id, product_id, listing_id, quantity)
                 VALUES (?1, ?2, ?3, ?4)"
                    .to_string(),
                owned_params(params![
                    remote_order.order_id.clone(),
                    remote_order.product_id.clone(),
                    remote_order.listing_id.clone(),
                    remote_order.quantity
                ]),
            ));
            summary.orders_added += 1;
        }
    }

    // ── Platform Sync Meta: newer wins ───────────────────────
    if let Some(remote_at) = remote
        .platform_sync_meta
        .last_platform_sync_at
        .as_ref()
        .filter(|at| {
            !implausible(
                &mut summary,
                now,
                "platform sync meta",
                "last_platform_sync_at",
                at,
            )
        })
    {
        let local_at = db.get_p2p_meta("last_platform_sync_at").await?;
        let should_update = match &local_at {
            Some(local) => remote_at > local,
            None => true,
        };
        if should_update {
            // The same upsert `Db::set_p2p_meta` runs.
            writes.push((
                "INSERT INTO p2p_meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = ?2"
                    .to_string(),
                owned_params(params!["last_platform_sync_at", remote_at.as_str()]),
            ));
            if let Some(ref by) = remote.platform_sync_meta.last_platform_sync_by {
                writes.push((
                    "INSERT INTO p2p_meta (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = ?2"
                        .to_string(),
                    owned_params(params!["last_platform_sync_by", by.as_str()]),
                ));
            }
        }
    }

    // ── Company Config: LWW by updated_at ────────────────────
    if let Some(remote_config) = remote
        .company_config
        .as_ref()
        .filter(|c| !implausible(&mut summary, now, "company config", "", &c.updated_at))
    {
        let local_config = db.get_company_config().await?;
        let should_update = match &local_config {
            Some((_, local_updated)) => remote_config.updated_at > *local_updated,
            None => true,
        };
        if should_update {
            // The same upsert `Db::set_company_config` runs.
            writes.push((
                "INSERT INTO company_config (id, config_json, updated_at)
                 VALUES (1, ?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET config_json = ?1, updated_at = ?2"
                    .to_string(),
                owned_params(params![
                    remote_config.config_json.clone(),
                    remote_config.updated_at.clone()
                ]),
            ));
            debug!("Updated company config from remote");
        }
    }

    // ── Auth Tokens: LWW by updated_at ───────────────────────
    for remote_token in &remote.auth_tokens {
        if implausible(
            &mut summary,
            now,
            "auth token",
            &remote_token.platform,
            &remote_token.updated_at,
        ) {
            continue;
        }
        let local_token = db.get_auth_token(&remote_token.platform).await?;
        let should_update = match &local_token {
            Some(local) => remote_token.updated_at > local.updated_at,
            None => true,
        };
        if should_update {
            // The same upsert `Db::upsert_auth_token_from_peer` runs, keeping the
            // peer's `updated_at`.
            writes.push((
                "INSERT INTO auth_tokens (platform, access_token, refresh_token, expires_at, cookies, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(platform) DO UPDATE
                 SET access_token = ?2, refresh_token = ?3, expires_at = ?4, cookies = ?5, updated_at = ?6"
                    .to_string(),
                owned_params(params![
                    remote_token.platform.clone(),
                    remote_token.access_token.clone(),
                    remote_token.refresh_token.clone(),
                    remote_token.expires_at.clone(),
                    remote_token.cookies.clone(),
                    remote_token.updated_at.clone()
                ]),
            ));
            debug!(platform = %remote_token.platform, "Updated auth token from remote");
        }
    }

    // ── Company Logo: LWW by updated_at ────────────────────
    if let Some(remote_logo) = remote
        .company_logo
        .as_ref()
        .filter(|l| !implausible(&mut summary, now, "company logo", "", &l.updated_at))
    {
        let local_logo = db.get_company_logo().await?;
        let should_update = match &local_logo {
            Some((_, local_updated)) => remote_logo.updated_at > *local_updated,
            None => true,
        };
        if should_update {
            // The same update `Db::set_company_logo` runs.
            writes.push((
                "UPDATE company SET logo = ?1, logo_updated_at = ?2, updated_at = datetime('now')"
                    .to_string(),
                owned_params(params![
                    remote_logo.logo.clone(),
                    remote_logo.updated_at.clone()
                ]),
            ));
            debug!("Updated company logo from remote");
        }
    }

    // ── Vendor Plugins: LWW by updated_at per plugin id ────
    for remote_plugin in &remote.vendor_plugins {
        if implausible(
            &mut summary,
            now,
            "vendor plugin",
            &remote_plugin.id,
            &remote_plugin.updated_at,
        ) {
            continue;
        }
        let local_plugin = db.get_registry_plugin(&remote_plugin.id).await;
        let should_update = match &local_plugin {
            Ok(Some(local)) => remote_plugin.updated_at > local.updated_at,
            Ok(None) => true,
            Err(_) => false, // table may not exist
        };
        if should_update {
            use nisaba_core::types::VendorPluginRow;
            let row = VendorPluginRow {
                id: remote_plugin.id.clone(),
                plugin_file: remote_plugin.plugin_file.clone(),
                display_name: remote_plugin.display_name.clone(),
                description: remote_plugin.description.clone(),
                files_json: remote_plugin.resolved_files_json(),
                version: remote_plugin.version.clone(),
                config_fields: remote_plugin.config_fields.clone(),
                // Secrets never sync between machines: strip any `secret: true`
                // values a peer may have sent before storing the row.
                config_json: nisaba_core::db::strip_secret_values(
                    remote_plugin.config_fields.as_deref(),
                    remote_plugin.config_json.as_deref(),
                ),
                status: remote_plugin.status.clone(),
                submitted_by: remote_plugin.submitted_by.clone(),
                approved_by: remote_plugin.approved_by.clone(),
                category: remote_plugin.category.clone(),
                icon: remote_plugin.icon.clone(),
                include_vendor_stock: remote_plugin.include_vendor_stock,
                created_at: remote_plugin.updated_at.clone(), // use updated_at as fallback
                updated_at: remote_plugin.updated_at.clone(),
                // Never taken from a peer: the allowlist shown to the user must come from
                // these files, so it is recomputed locally when the plugin is installed.
                allowed_hosts_json: None,
            };
            // The same upsert `Db::upsert_registry_plugin` runs.
            writes.push((
                "INSERT INTO vendor_plugin_registry
                    (id, plugin_file, display_name, description, code, files_json, version,
                     config_fields, config_json, status, submitted_by, approved_by,
                     category, icon, include_vendor_stock, created_at, updated_at, allowed_hosts)
                 VALUES (?1, ?2, ?3, ?4, '', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                 ON CONFLICT(id) DO UPDATE SET
                    plugin_file = ?2, display_name = ?3, description = ?4, files_json = ?5,
                    version = ?6, config_fields = ?7, config_json = ?8, status = ?9,
                    submitted_by = ?10, approved_by = ?11, category = ?12, icon = ?13,
                    include_vendor_stock = ?14, updated_at = ?16, allowed_hosts = ?17"
                    .to_string(),
                owned_params(params![
                    row.id.clone(),
                    row.plugin_file.clone(),
                    row.display_name.clone(),
                    row.description.clone(),
                    row.files_json.clone(),
                    row.version.clone(),
                    row.config_fields.clone(),
                    row.config_json.clone(),
                    row.status.clone(),
                    row.submitted_by.clone(),
                    row.approved_by.clone(),
                    row.category.clone(),
                    row.icon.clone(),
                    row.include_vendor_stock as i64,
                    row.created_at.clone(),
                    row.updated_at.clone(),
                    row.allowed_hosts_json.clone(),
                ]),
            ));
            summary.vendor_plugins_updated += 1;
            debug!(plugin_id = %remote_plugin.id, "Updated vendor plugin from remote");
        }
    }

    // ── Product Variants: LWW by updated_at ─────────────────
    let local_variants = db.list_all_variants().await?;
    let local_variant_map: HashMap<String, &ProductVariant> =
        local_variants.iter().map(|v| (v.id.clone(), v)).collect();
    let mut affected_product_ids = std::collections::HashSet::new();
    for remote_variant in &remote.product_variants {
        if implausible(
            &mut summary,
            now,
            "variant",
            &remote_variant.id,
            &remote_variant.updated_at,
        ) {
            continue;
        }
        let local_variant = local_variant_map.get(&remote_variant.id).copied();
        let variant = match local_variant {
            Some(local) => {
                if remote_variant.updated_at <= local.updated_at {
                    continue;
                }
                let attrs: std::collections::HashMap<String, String> =
                    serde_json::from_str(&remote_variant.attributes_json).unwrap_or_default();
                ProductVariant {
                    id: remote_variant.id.clone(),
                    product_id: remote_variant.product_id.clone(),
                    sku: remote_variant.sku.clone(),
                    name: remote_variant.name.clone(),
                    attributes: attrs,
                    quantity: remote_variant.quantity,
                    on_hand_quantity: remote_variant.on_hand_quantity,
                    image_url: remote_variant.image_url.clone(),
                    sort_order: remote_variant.sort_order,
                    // The winning edit's link; an older peer sends none, so keep ours.
                    source_plugin_id: remote_variant
                        .source_plugin_id
                        .clone()
                        .or(local.source_plugin_id.clone()),
                    source_vendor_item_id: remote_variant
                        .source_vendor_item_id
                        .clone()
                        .or(local.source_vendor_item_id.clone()),
                    created_at: local.created_at.clone(),
                    updated_at: remote_variant.updated_at.clone(),
                }
            }
            None => {
                let attrs: std::collections::HashMap<String, String> =
                    serde_json::from_str(&remote_variant.attributes_json).unwrap_or_default();
                ProductVariant {
                    id: remote_variant.id.clone(),
                    product_id: remote_variant.product_id.clone(),
                    sku: remote_variant.sku.clone(),
                    name: remote_variant.name.clone(),
                    attributes: attrs,
                    quantity: remote_variant.quantity,
                    on_hand_quantity: remote_variant.on_hand_quantity,
                    image_url: remote_variant.image_url.clone(),
                    sort_order: remote_variant.sort_order,
                    source_plugin_id: remote_variant.source_plugin_id.clone(),
                    source_vendor_item_id: remote_variant.source_vendor_item_id.clone(),
                    created_at: remote_variant.updated_at.clone(),
                    updated_at: remote_variant.updated_at.clone(),
                }
            }
        };
        let attrs_json = serde_json::to_string(&variant.attributes)
            .map_err(|e| P2PError::SerializationError(e.to_string()))?;
        // The same upsert `Db::upsert_variant_from_peer` runs, keeping the peer's
        // `updated_at`.
        writes.push((
            "INSERT INTO product_variants (id, product_id, sku, name, attributes_json, quantity, on_hand_quantity, image_url, sort_order, source_plugin_id, source_vendor_item_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
             ON CONFLICT(id) DO UPDATE SET sku = ?3, name = ?4, attributes_json = ?5, quantity = ?6,
                on_hand_quantity = ?7, image_url = ?8, sort_order = ?9,
                source_plugin_id = COALESCE(?10, source_plugin_id),
                source_vendor_item_id = COALESCE(?11, source_vendor_item_id),
                updated_at = ?12"
                .to_string(),
            owned_params(params![
                variant.id.clone(),
                variant.product_id.clone(),
                variant.sku.clone(),
                variant.name.clone(),
                attrs_json,
                variant.quantity,
                variant.on_hand_quantity,
                variant.image_url.clone(),
                variant.sort_order as i64,
                variant.source_plugin_id.clone(),
                variant.source_vendor_item_id.clone(),
                variant.updated_at.clone(),
            ]),
        ));
        summary.variants_updated += 1;
        affected_product_ids.insert(remote_variant.product_id.clone());
    }

    // ── Apply: the whole merge in one transaction ────────────
    // Any error here rolls every planned write back, leaving the database
    // unchanged; the summary is only meaningful once this has committed.
    db.execute_write_batch(writes).await?;

    // Recalc quantities for affected products
    for product_id in &affected_product_ids {
        let _ = db.recalc_product_quantity(product_id).await;
    }

    info!(
        products_created = summary.products_created,
        products_updated = summary.products_updated,
        mappings_updated = summary.mappings_updated,
        snapshots_updated = summary.snapshots_updated,
        orders_added = summary.orders_added,
        vendor_plugins_updated = summary.vendor_plugins_updated,
        variants_updated = summary.variants_updated,
        "Merge complete"
    );

    Ok(summary)
}
