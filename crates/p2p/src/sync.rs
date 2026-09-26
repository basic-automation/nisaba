use std::collections::HashMap;

use tracing::{debug, info};
use turso::params;

use nisaba_core::db::Db;
use nisaba_core::types::{Platform, Product};

use crate::error::P2PError;
use crate::types::{
    CompanyConfigPayload, CompanyLogoPayload, MergeSummary, PlatformSyncMeta, SyncPayload,
    SyncableAuthToken, SyncableMapping, SyncableVariant, SyncableVendorPlugin,
};

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
            .map(|p| SyncableVendorPlugin {
                id: p.id,
                plugin_file: p.plugin_file,
                display_name: p.display_name,
                description: p.description,
                files_json: p.files_json,
                code: String::new(),
                version: p.version,
                config_fields: p.config_fields,
                config_json: p.config_json,
                status: p.status,
                submitted_by: p.submitted_by,
                approved_by: p.approved_by,
                category: p.category,
                icon: p.icon,
                include_vendor_stock: p.include_vendor_stock,
                updated_at: p.updated_at,
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

/// Merge a remote peer's payload into our local database.
/// Returns a summary of what changed.
pub async fn merge_remote_payload(db: &Db, remote: &SyncPayload) -> Result<MergeSummary, P2PError> {
    let mut summary = MergeSummary::default();

    // ── Products: last-write-wins by updated_at ──────────────
    let local_products = db.list_products().await?;
    let local_map: HashMap<String, &Product> =
        local_products.iter().map(|p| (p.id.clone(), p)).collect();

    for remote_product in &remote.products {
        match local_map.get(&remote_product.id) {
            Some(local) => {
                if remote_product.updated_at > local.updated_at {
                    let conn = db.connect().await?;
                    conn.execute(
                        "UPDATE products SET canonical_sku = ?1, name = ?2, quantity = ?3,
                         low_stock_threshold = ?4, is_tracked = ?5, has_variants = ?6, updated_at = ?7
                         WHERE id = ?8",
                        params![
                            remote_product.canonical_sku.clone(),
                            remote_product.name.clone(),
                            remote_product.quantity,
                            remote_product.low_stock_threshold,
                            remote_product.is_tracked,
                            remote_product.has_variants,
                            remote_product.updated_at.clone(),
                            remote_product.id.clone()
                        ],
                    )
                    .await?;
                    summary.products_updated += 1;
                    debug!(
                        product_id = %remote_product.id,
                        "Updated product from remote (newer updated_at)"
                    );
                }
            }
            None => {
                let conn = db.connect().await?;
                conn.execute(
                    "INSERT INTO products (id, canonical_sku, name, quantity, low_stock_threshold, is_tracked, has_variants, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        remote_product.id.clone(),
                        remote_product.canonical_sku.clone(),
                        remote_product.name.clone(),
                        remote_product.quantity,
                        remote_product.low_stock_threshold,
                        remote_product.is_tracked,
                        remote_product.has_variants,
                        remote_product.created_at.clone(),
                        remote_product.updated_at.clone()
                    ],
                )
                .await?;
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
        let vkey = remote_mapping.variant_id.clone().unwrap_or_default();
        let key = (
            remote_mapping.product_id.clone(),
            remote_mapping.platform.as_str().to_string(),
            vkey,
        );

        match local_mapping_map.get(&key) {
            Some((local_id, _local_deleted, local_updated)) => {
                if remote_mapping.updated_at > *local_updated {
                    let conn = db.connect().await?;
                    conn.execute(
                        "UPDATE platform_mappings SET platform_item_id = ?1, platform_sku = ?2,
                         is_active = ?3, deleted_at = ?4, updated_at = ?5, variant_id = ?6
                         WHERE id = ?7",
                        params![
                            remote_mapping.platform_item_id.clone(),
                            remote_mapping.platform_sku.clone(),
                            remote_mapping.is_active,
                            remote_mapping.deleted_at.clone(),
                            remote_mapping.updated_at.clone(),
                            remote_mapping.variant_id.clone(),
                            *local_id
                        ],
                    )
                    .await?;
                    summary.mappings_updated += 1;
                }
            }
            None => {
                let conn = db.connect().await?;
                conn.execute(
                    "INSERT OR IGNORE INTO platform_mappings (product_id, platform, platform_item_id, platform_sku, is_active, deleted_at, updated_at, variant_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        remote_mapping.product_id.clone(),
                        remote_mapping.platform.as_str(),
                        remote_mapping.platform_item_id.clone(),
                        remote_mapping.platform_sku.clone(),
                        remote_mapping.is_active,
                        remote_mapping.deleted_at.clone(),
                        remote_mapping.updated_at.clone(),
                        remote_mapping.variant_id.clone()
                    ],
                )
                .await?;
                summary.mappings_updated += 1;
            }
        }
    }

    // ── Platform Snapshots: newer last_polled_at wins ─────────
    for remote_snap in &remote.platform_snapshots {
        let local_snap = db
            .get_snapshot(&remote_snap.product_id, remote_snap.platform.as_str())
            .await?;

        match local_snap {
            Some(local) => {
                if remote_snap.last_polled_at > local.last_polled_at {
                    db.upsert_snapshot(
                        &remote_snap.product_id,
                        remote_snap.platform.as_str(),
                        remote_snap.last_known_quantity,
                        &remote_snap.last_polled_at,
                    )
                    .await?;
                    summary.snapshots_updated += 1;
                }
            }
            None => {
                db.upsert_snapshot(
                    &remote_snap.product_id,
                    remote_snap.platform.as_str(),
                    remote_snap.last_known_quantity,
                    &remote_snap.last_polled_at,
                )
                .await?;
                summary.snapshots_updated += 1;
            }
        }
    }

    // ── XMR Processed Orders: union (insert if missing) ──────
    for remote_order in &remote.xmr_processed_orders {
        let already = db.is_xmr_order_processed(&remote_order.order_id).await?;
        if !already {
            db.mark_xmr_order_processed(
                &remote_order.order_id,
                remote_order.product_id.as_deref(),
                &remote_order.listing_id,
                remote_order.quantity,
            )
            .await?;
            summary.orders_added += 1;
        }
    }

    // ── Platform Sync Meta: newer wins ───────────────────────
    if let Some(ref remote_at) = remote.platform_sync_meta.last_platform_sync_at {
        let local_at = db.get_p2p_meta("last_platform_sync_at").await?;
        let should_update = match &local_at {
            Some(local) => remote_at > local,
            None => true,
        };
        if should_update {
            db.set_p2p_meta("last_platform_sync_at", remote_at).await?;
            if let Some(ref by) = remote.platform_sync_meta.last_platform_sync_by {
                db.set_p2p_meta("last_platform_sync_by", by).await?;
            }
        }
    }

    // ── Company Config: LWW by updated_at ────────────────────
    if let Some(ref remote_config) = remote.company_config {
        let local_config = db.get_company_config().await?;
        let should_update = match &local_config {
            Some((_, local_updated)) => remote_config.updated_at > *local_updated,
            None => true,
        };
        if should_update {
            db.set_company_config(&remote_config.config_json, &remote_config.updated_at)
                .await?;
            debug!("Updated company config from remote");
        }
    }

    // ── Auth Tokens: LWW by updated_at ───────────────────────
    for remote_token in &remote.auth_tokens {
        let local_token = db.get_auth_token(&remote_token.platform).await?;
        let should_update = match &local_token {
            Some(local) => remote_token.updated_at > local.updated_at,
            None => true,
        };
        if should_update {
            db.upsert_auth_token(
                &remote_token.platform,
                &remote_token.access_token,
                remote_token.refresh_token.as_deref(),
                remote_token.expires_at.as_deref(),
                remote_token.cookies.as_deref(),
            )
            .await?;
            debug!(platform = %remote_token.platform, "Updated auth token from remote");
        }
    }

    // ── Company Logo: LWW by updated_at ────────────────────
    if let Some(ref remote_logo) = remote.company_logo {
        let local_logo = db.get_company_logo().await?;
        let should_update = match &local_logo {
            Some((_, local_updated)) => remote_logo.updated_at > *local_updated,
            None => true,
        };
        if should_update {
            db.set_company_logo(&remote_logo.logo, &remote_logo.updated_at)
                .await?;
            debug!("Updated company logo from remote");
        }
    }

    // ── Vendor Plugins: LWW by updated_at per plugin id ────
    for remote_plugin in &remote.vendor_plugins {
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
                config_json: remote_plugin.config_json.clone(),
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
            if let Ok(()) = db.upsert_registry_plugin(&row).await {
                summary.vendor_plugins_updated += 1;
                debug!(plugin_id = %remote_plugin.id, "Updated vendor plugin from remote");
            }
        }
    }

    // ── Product Variants: LWW by updated_at ─────────────────
    let mut affected_product_ids = std::collections::HashSet::new();
    for remote_variant in &remote.product_variants {
        let local_variant = db.get_variant(&remote_variant.id).await?;
        match local_variant {
            Some(local) => {
                if remote_variant.updated_at > local.updated_at {
                    let attrs: std::collections::HashMap<String, String> =
                        serde_json::from_str(&remote_variant.attributes_json).unwrap_or_default();
                    let variant = nisaba_core::types::ProductVariant {
                        id: remote_variant.id.clone(),
                        product_id: remote_variant.product_id.clone(),
                        sku: remote_variant.sku.clone(),
                        name: remote_variant.name.clone(),
                        attributes: attrs,
                        quantity: remote_variant.quantity,
                        on_hand_quantity: remote_variant.on_hand_quantity,
                        image_url: remote_variant.image_url.clone(),
                        sort_order: remote_variant.sort_order,
                        source_plugin_id: local.source_plugin_id.clone(),
                        source_vendor_item_id: local.source_vendor_item_id.clone(),
                        created_at: local.created_at,
                        updated_at: remote_variant.updated_at.clone(),
                    };
                    db.upsert_variant_from_peer(&variant).await?;
                    summary.variants_updated += 1;
                    affected_product_ids.insert(remote_variant.product_id.clone());
                }
            }
            None => {
                let attrs: std::collections::HashMap<String, String> =
                    serde_json::from_str(&remote_variant.attributes_json).unwrap_or_default();
                let variant = nisaba_core::types::ProductVariant {
                    id: remote_variant.id.clone(),
                    product_id: remote_variant.product_id.clone(),
                    sku: remote_variant.sku.clone(),
                    name: remote_variant.name.clone(),
                    attributes: attrs,
                    quantity: remote_variant.quantity,
                    on_hand_quantity: remote_variant.on_hand_quantity,
                    image_url: remote_variant.image_url.clone(),
                    sort_order: remote_variant.sort_order,
                    source_plugin_id: None,
                    source_vendor_item_id: None,
                    created_at: remote_variant.updated_at.clone(),
                    updated_at: remote_variant.updated_at.clone(),
                };
                db.upsert_variant_from_peer(&variant).await?;
                summary.variants_updated += 1;
                affected_product_ids.insert(remote_variant.product_id.clone());
            }
        }
    }

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
