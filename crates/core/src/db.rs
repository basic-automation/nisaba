use tracing::{debug, info};
use turso::params::Params;
use turso::{params, Builder, Connection, Database, Error as TursoError};

use crate::config::CompanyRegistryEntry;
use crate::error::SyncError;
use crate::types::{
    AuthToken, ExportData, InventoryHistoryEntry, ListingDescription, ListingPhoto, NewSyncEvent,
    Platform, PlatformMapping, PlatformSnapshot, PricingSnapshot, Product, ProductVariant,
    SyncEvent, VendorListingCache, VendorPluginInstall, VendorPluginRow, XmrProcessedOrder,
};

const CURRENT_SCHEMA_VERSION: i64 = 17;

/// Convert any IntoParams value into a cloneable Params.
/// Panics on conversion failure (should not happen with valid params).
fn to_params(p: impl turso::params::IntoParams) -> Params {
    p.into_params().expect("param conversion should not fail")
}

/// Check if a turso error is retryable under MVCC concurrent transactions.
///
/// Returns true for:
/// - `Busy` / `BusySnapshot` — another writer holds a lock
/// - `Error` containing "conflict" — MVCC write-write conflict detected at commit
fn is_retryable(e: &TursoError) -> bool {
    matches!(e, TursoError::Busy(_) | TursoError::BusySnapshot(_))
        || matches!(e, TursoError::Error(msg) if msg.contains("conflict") || msg.contains("database is locked"))
}

pub struct Db {
    db: Database,
}

impl Db {
    /// Open (or create) a local database at the given path.
    ///
    /// If turso panics during open (e.g. incompatible automatic indexes from an
    /// older schema), the corrupted file is deleted and a fresh database is
    /// created. The caller must run `migrate()` afterwards to re-apply the
    /// schema, so no data-bearing tables are lost on a brand-new DB.
    pub async fn open(path: &str) -> Result<Self, SyncError> {
        let p = path.to_string();
        let result = tokio::task::spawn(async move { Builder::new_local(&p).build().await }).await;

        let db = match result {
            Ok(Ok(db)) => db,
            Ok(Err(e)) => return Err(SyncError::DatabaseError(e.to_string())),
            Err(join_err) => {
                if join_err.is_panic() {
                    tracing::warn!(
                        "turso panicked opening {path} — cleaning journal files and retrying"
                    );
                    // Delete stale journal files that may cause the panic,
                    // but NEVER delete the main database file (contains user data).
                    for suffix in &["-log", "-wal", "-shm"] {
                        let journal = format!("{path}{suffix}");
                        if std::path::Path::new(&journal).exists() {
                            let _ = std::fs::remove_file(&journal);
                            tracing::info!("Deleted journal file {journal}");
                        }
                    }
                    Builder::new_local(path).build().await?
                } else {
                    return Err(SyncError::DatabaseError(format!(
                        "Database open task cancelled: {join_err}"
                    )));
                }
            }
        };

        let me = Self { db };
        me.apply_db_pragmas().await?;
        Ok(me)
    }

    /// Set database-level pragmas (only needs to run once per database file).
    ///
    /// - `journal_mode = wal`: WAL mode allows concurrent reads during writes
    ///   and handles DDL safely (turso's MVCC mode crashes during DDL checkpoints)
    async fn apply_db_pragmas(&self) -> Result<(), SyncError> {
        let conn = self.db.connect()?;
        conn.pragma_update("journal_mode", "'wal'")
            .await
            .map_err(|e| {
                SyncError::DatabaseError(format!("Failed to set journal_mode=wal: {e}"))
            })?;
        debug!(
            "Applied database pragmas (journal_mode=wal, busy_timeout=5000, synchronous=NORMAL)"
        );
        Ok(())
    }

    /// Get a connection with per-connection pragmas applied.
    ///
    /// Every connection gets `busy_timeout` and `synchronous` set because these
    /// are per-connection pragmas in SQLite that don't persist across connections.
    pub async fn connect(&self) -> Result<Connection, SyncError> {
        let conn = self.db.connect()?;
        conn.execute("PRAGMA busy_timeout = 5000", ()).await?;
        conn.execute("PRAGMA synchronous = NORMAL", ()).await?;
        Ok(conn)
    }

    /// Execute a single write statement under a MVCC concurrent transaction.
    ///
    /// Uses `BEGIN IMMEDIATE` so multiple callers can write simultaneously
    /// without blocking each other. Retries automatically on busy/conflict errors
    /// (up to 5 attempts with yield between retries).
    pub async fn execute_write(
        &self,
        sql: &str,
        p: impl turso::params::IntoParams,
    ) -> Result<u64, SyncError> {
        // Convert to Params upfront so we can clone for retries
        let params: Params = p
            .into_params()
            .map_err(|e| SyncError::DatabaseError(format!("Param conversion: {e}")))?;

        let conn = self.connect().await?;
        for attempt in 0..5u32 {
            match conn.execute("BEGIN IMMEDIATE", ()).await {
                Ok(_) => {}
                Err(ref e) if is_retryable(e) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50 * (1 << attempt))).await;
                    continue;
                }
                Err(e) => return Err(SyncError::DatabaseError(e.to_string())),
            }

            let result = conn
                .execute(sql, params.clone())
                .await
                .and(conn.execute("COMMIT", ()).await);

            match result {
                Ok(n) => return Ok(n),
                Err(ref e) if is_retryable(e) => {
                    let _ = conn.execute("ROLLBACK", ()).await;
                    tokio::time::sleep(std::time::Duration::from_millis(50 * (1 << attempt))).await;
                }
                Err(e) => {
                    let _ = conn.execute("ROLLBACK", ()).await;
                    return Err(SyncError::DatabaseError(e.to_string()));
                }
            }
        }
        Err(SyncError::DatabaseError(format!(
            "Write failed after 5 retries: {sql}"
        )))
    }

    /// Execute multiple write statements atomically under a MVCC concurrent transaction.
    ///
    /// All statements share a single `BEGIN IMMEDIATE ... COMMIT` so they
    /// either all succeed or all roll back. Retries on busy errors.
    pub async fn execute_writes(&self, stmts: &[(&str, Params)]) -> Result<(), SyncError> {
        let conn = self.connect().await?;
        for attempt in 0..5u32 {
            match conn.execute("BEGIN IMMEDIATE", ()).await {
                Ok(_) => {}
                Err(ref e) if is_retryable(e) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50 * (1 << attempt))).await;
                    continue;
                }
                Err(e) => return Err(SyncError::DatabaseError(e.to_string())),
            }

            let mut ok = true;
            let mut last_err = None;
            for (sql, p) in stmts {
                match conn.execute(sql, p.clone()).await {
                    Ok(_) => {}
                    Err(e) => {
                        ok = false;
                        last_err = Some(e);
                        break;
                    }
                }
            }

            if ok {
                match conn.execute("COMMIT", ()).await {
                    Ok(_) => return Ok(()),
                    Err(ref e) if is_retryable(e) => {
                        let _ = conn.execute("ROLLBACK", ()).await;
                        tokio::time::sleep(std::time::Duration::from_millis(50 * (1 << attempt)))
                            .await;
                        continue;
                    }
                    Err(e) => {
                        let _ = conn.execute("ROLLBACK", ()).await;
                        return Err(SyncError::DatabaseError(e.to_string()));
                    }
                }
            }

            let _ = conn.execute("ROLLBACK", ()).await;
            if let Some(ref e) = last_err {
                if is_retryable(e) {
                    tokio::time::sleep(std::time::Duration::from_millis(50 * (1 << attempt))).await;
                    continue;
                }
                return Err(SyncError::DatabaseError(e.to_string()));
            }
        }
        Err(SyncError::DatabaseError(
            "Batch write failed after 5 retries".to_string(),
        ))
    }

    /// Run migrations if needed. Uses PRAGMA user_version to track schema version.
    pub async fn migrate(&self) -> Result<(), SyncError> {
        let conn = self.connect().await?;

        let mut rows = conn.query("PRAGMA user_version", ()).await?;
        let version: i64 = if let Some(row) = rows.next().await? {
            row.get::<i64>(0)?
        } else {
            0
        };

        if version < CURRENT_SCHEMA_VERSION {
            info!(
                current_version = version,
                target_version = CURRENT_SCHEMA_VERSION,
                "Running database migrations"
            );

            if version < 1 {
                conn.execute_batch(include_str!("../../../migrations/001_initial.sql"))
                    .await?;
                debug!("Applied migration 001_initial.sql");
            }

            if version < 2 {
                conn.execute_batch(include_str!("../../../migrations/002_p2p_company.sql"))
                    .await?;
                debug!("Applied migration 002_p2p_company.sql");
            }

            if version < 3 {
                conn.execute_batch(include_str!("../../../migrations/003_multi_company.sql"))
                    .await?;

                // Migrate existing peers from company_peers → company_peers_v2
                self.migrate_peers_to_v2().await?;

                // Add peer_id column to company table if it exists and doesn't have it
                self.add_company_peer_id_column().await?;

                debug!("Applied migration 003_multi_company.sql");
            }

            if version < 4 {
                // Add logo columns to company table (company DB)
                self.add_company_logo_columns().await?;
                // Add logo column to company_registry table (app DB)
                self.add_registry_logo_column().await?;
                debug!("Applied migration 004_company_logo");
            }

            if version < 5 {
                conn.execute_batch(include_str!("../../../migrations/005_vendor_plugins.sql"))
                    .await?;
                debug!("Applied migration 005_vendor_plugins.sql");
            }

            if version < 6 {
                conn.execute_batch(include_str!("../../../migrations/006_plugin_metadata.sql"))
                    .await?;
                debug!("Applied migration 006_plugin_metadata.sql");
            }

            if version < 7 {
                // Create product_variants table + indexes
                conn.execute_batch(include_str!("../../../migrations/007_product_variants.sql"))
                    .await?;

                // Add has_variants column (idempotent — ignore error if already exists)
                let _ = conn
                    .execute(
                        "ALTER TABLE products ADD COLUMN has_variants BOOLEAN NOT NULL DEFAULT 0",
                        (),
                    )
                    .await;

                // Rebuild platform_mappings to add variant_id and replace the unique index.
                // Turso/libsql cannot DROP INDEX on indexes created with CREATE UNIQUE INDEX,
                // so we recreate the table instead.
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS platform_mappings_new (
                        id               INTEGER PRIMARY KEY AUTOINCREMENT,
                        product_id       TEXT NOT NULL REFERENCES products(id),
                        platform         TEXT NOT NULL,
                        platform_item_id TEXT NOT NULL,
                        platform_sku     TEXT,
                        is_active        BOOLEAN NOT NULL DEFAULT TRUE,
                        deleted_at       TEXT,
                        updated_at       TEXT NOT NULL DEFAULT '',
                        variant_id       TEXT REFERENCES product_variants(id)
                    );
                    INSERT OR IGNORE INTO platform_mappings_new (id, product_id, platform, platform_item_id, platform_sku, is_active, deleted_at, updated_at)
                        SELECT id, product_id, platform, platform_item_id, platform_sku, is_active, deleted_at, updated_at FROM platform_mappings;
                    DROP TABLE platform_mappings;
                    ALTER TABLE platform_mappings_new RENAME TO platform_mappings;
                    CREATE UNIQUE INDEX IF NOT EXISTS idx_mappings_product_platform_variant
                        ON platform_mappings(product_id, platform, COALESCE(variant_id, ''));
                    CREATE UNIQUE INDEX IF NOT EXISTS idx_mappings_platform_item
                        ON platform_mappings(platform, platform_item_id);",
                )
                .await?;

                debug!("Applied migration 007_product_variants");
            }

            if version < 8 {
                // Repair: migration 7 rebuilt platform_mappings but may have
                // dropped the deleted_at/updated_at columns added by migration 2.
                // Add them back idempotently.
                let _ = conn
                    .execute(
                        "ALTER TABLE platform_mappings ADD COLUMN deleted_at TEXT",
                        (),
                    )
                    .await;
                let _ = conn
                    .execute(
                        "ALTER TABLE platform_mappings ADD COLUMN updated_at TEXT NOT NULL DEFAULT ''",
                        (),
                    )
                    .await;
                debug!("Applied migration 008_repair_mappings_columns");
            }

            if version < 9 {
                conn.execute_batch(include_str!(
                    "../../../migrations/009_vendor_listings_cache.sql"
                ))
                .await?;
                debug!("Applied migration 009_vendor_listings_cache.sql");
            }

            if version < 10 {
                self.migrate_always_variants().await?;
                debug!("Applied migration 010_always_variants");
            }

            if version < 11 {
                conn.execute_batch(include_str!("../../../migrations/011_cached_listings.sql"))
                    .await?;
                debug!("Applied migration 011_cached_listings.sql");
            }

            if version < 12 {
                // Idempotent — ignore error if columns already exist (e.g. from
                // a previous interrupted migration that didn't bump the version).
                let _ = conn
                    .execute(
                        "ALTER TABLE product_variants ADD COLUMN source_plugin_id TEXT",
                        (),
                    )
                    .await;
                let _ = conn
                    .execute(
                        "ALTER TABLE product_variants ADD COLUMN source_vendor_item_id TEXT",
                        (),
                    )
                    .await;
                debug!("Applied migration 012_variant_vendor_source.sql");
            }

            if version < 13 {
                conn.execute_batch(include_str!(
                    "../../../migrations/013_product_vendor_data.sql"
                ))
                .await?;

                // Backfill: create product_vendor_data rows from existing variants
                // that have source_plugin_id and source_vendor_item_id
                conn.execute_batch(
                    "INSERT OR IGNORE INTO product_vendor_data
                        (product_id, variant_id, plugin_id, vendor_item_id, title, quantity, sku, image_url, variant_attrs_json, fetched_at, updated_at)
                     SELECT
                        pv.product_id,
                        pv.id,
                        pv.source_plugin_id,
                        pv.source_vendor_item_id,
                        pv.name,
                        pv.quantity,
                        pv.sku,
                        pv.image_url,
                        COALESCE(pv.attributes_json, '{}'),
                        COALESCE(pv.created_at, datetime('now')),
                        datetime('now')
                     FROM product_variants pv
                     WHERE pv.source_plugin_id IS NOT NULL
                       AND pv.source_vendor_item_id IS NOT NULL",
                )
                .await?;

                // Enrich backfilled rows with price/url/extras from vendor_listings_cache.
                // libsql doesn't support correlated subqueries in UPDATE SET, so we
                // read matching cache rows in Rust and update one at a time.
                {
                    let mut cache_rows = conn
                        .query(
                            "SELECT vlc.plugin_id, vlc.vendor_item_id, vlc.price, vlc.currency,
                                    vlc.url, vlc.extras_json, vlc.group_key
                             FROM vendor_listings_cache vlc
                             INNER JOIN product_vendor_data pvd
                                ON vlc.plugin_id = pvd.plugin_id
                               AND vlc.vendor_item_id = pvd.vendor_item_id",
                            (),
                        )
                        .await?;

                    let mut enrichments = Vec::new();
                    while let Some(row) = cache_rows.next().await? {
                        enrichments.push((
                            row.get::<String>(0)?,         // plugin_id
                            row.get::<String>(1)?,         // vendor_item_id
                            row.get::<Option<f64>>(2)?,    // price
                            row.get::<Option<String>>(3)?, // currency
                            row.get::<Option<String>>(4)?, // url
                            row.get::<String>(5)?,         // extras_json
                            row.get::<Option<String>>(6)?, // group_key
                        ));
                    }

                    for (pid, vid, price, currency, url, extras_json, group_key) in enrichments {
                        conn.execute(
                            "UPDATE product_vendor_data SET price = ?1, currency = ?2, url = ?3, extras_json = ?4, group_key = ?5
                             WHERE plugin_id = ?6 AND vendor_item_id = ?7",
                            params![price, currency, url, extras_json, group_key, pid, vid],
                        )
                        .await?;
                    }
                }

                debug!("Applied migration 013_product_vendor_data.sql (with backfill)");
            }

            if version < 14 {
                // Dropship-aware inventory: per-plugin toggle + on-hand quantity
                let _ = conn
                    .execute(
                        "ALTER TABLE vendor_plugin_registry ADD COLUMN include_vendor_stock BOOLEAN NOT NULL DEFAULT 0",
                        (),
                    )
                    .await;
                let _ = conn
                    .execute(
                        "ALTER TABLE product_variants ADD COLUMN on_hand_quantity INTEGER NOT NULL DEFAULT 0",
                        (),
                    )
                    .await;
                // Backfill on_hand_quantity from existing quantity so nothing changes
                conn.execute(
                    "UPDATE product_variants SET on_hand_quantity = quantity",
                    (),
                )
                .await?;
                debug!("Applied migration 014: dropship inventory (include_vendor_stock + on_hand_quantity)");
            }

            if version < 15 {
                // Rebuild platform_mappings without AUTOINCREMENT (unsupported in MVCC mode).
                // INTEGER PRIMARY KEY still auto-generates IDs via max(rowid)+1.
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS platform_mappings_v2 (
                        id               INTEGER PRIMARY KEY,
                        product_id       TEXT NOT NULL REFERENCES products(id),
                        platform         TEXT NOT NULL,
                        platform_item_id TEXT NOT NULL,
                        platform_sku     TEXT,
                        is_active        BOOLEAN NOT NULL DEFAULT TRUE,
                        deleted_at       TEXT,
                        updated_at       TEXT NOT NULL DEFAULT '',
                        variant_id       TEXT REFERENCES product_variants(id)
                    );
                    INSERT OR IGNORE INTO platform_mappings_v2
                        SELECT id, product_id, platform, platform_item_id, platform_sku, is_active, deleted_at, updated_at, variant_id
                        FROM platform_mappings;
                    DROP TABLE platform_mappings;
                    ALTER TABLE platform_mappings_v2 RENAME TO platform_mappings;
                    CREATE UNIQUE INDEX IF NOT EXISTS idx_mappings_product_platform_variant
                        ON platform_mappings(product_id, platform, COALESCE(variant_id, ''));
                    CREATE UNIQUE INDEX IF NOT EXISTS idx_mappings_platform_item
                        ON platform_mappings(platform, platform_item_id);",
                )
                .await?;
                debug!("Applied migration 015: rebuild platform_mappings without AUTOINCREMENT for MVCC compat");
            }

            if version < 16 {
                // Rebuild all remaining tables that used AUTOINCREMENT (unsupported in MVCC mode).
                // INTEGER PRIMARY KEY still auto-generates IDs via max(rowid)+1.

                // 1. platform_snapshots
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS platform_snapshots_v2 (
                        id                  INTEGER PRIMARY KEY,
                        product_id          TEXT NOT NULL REFERENCES products(id),
                        platform            TEXT NOT NULL,
                        last_known_quantity INTEGER NOT NULL,
                        last_polled_at      TEXT NOT NULL,
                        last_pushed_at      TEXT,
                        version_tag         TEXT
                    );
                    INSERT OR IGNORE INTO platform_snapshots_v2 SELECT * FROM platform_snapshots;
                    DROP TABLE platform_snapshots;
                    ALTER TABLE platform_snapshots_v2 RENAME TO platform_snapshots;
                    CREATE UNIQUE INDEX IF NOT EXISTS idx_snapshots_product_platform
                        ON platform_snapshots(product_id, platform);",
                )
                .await?;

                // 2. sync_events
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS sync_events_v2 (
                        id              INTEGER PRIMARY KEY,
                        product_id      TEXT NOT NULL,
                        source_platform TEXT NOT NULL,
                        target_platform TEXT NOT NULL,
                        old_quantity    INTEGER NOT NULL,
                        new_quantity    INTEGER NOT NULL,
                        event_type      TEXT NOT NULL,
                        sync_cycle_id   TEXT NOT NULL,
                        error_message   TEXT,
                        created_at      TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    INSERT OR IGNORE INTO sync_events_v2 SELECT * FROM sync_events;
                    DROP TABLE sync_events;
                    ALTER TABLE sync_events_v2 RENAME TO sync_events;",
                )
                .await?;

                // 3. inventory_history
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS inventory_history_v2 (
                        id            INTEGER PRIMARY KEY,
                        product_id    TEXT NOT NULL REFERENCES products(id),
                        quantity      INTEGER NOT NULL,
                        sync_cycle_id TEXT NOT NULL,
                        recorded_at   TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    INSERT OR IGNORE INTO inventory_history_v2 SELECT * FROM inventory_history;
                    DROP TABLE inventory_history;
                    ALTER TABLE inventory_history_v2 RENAME TO inventory_history;
                    CREATE INDEX IF NOT EXISTS idx_inventory_history_product_time
                        ON inventory_history(product_id, recorded_at);",
                )
                .await?;

                // 4. listing_descriptions
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS listing_descriptions_v2 (
                        id               INTEGER PRIMARY KEY,
                        product_id       TEXT NOT NULL REFERENCES products(id),
                        platform         TEXT NOT NULL,
                        platform_item_id TEXT NOT NULL,
                        description_html TEXT,
                        description_text TEXT,
                        fetched_at       TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    INSERT OR IGNORE INTO listing_descriptions_v2 SELECT * FROM listing_descriptions;
                    DROP TABLE listing_descriptions;
                    ALTER TABLE listing_descriptions_v2 RENAME TO listing_descriptions;
                    CREATE UNIQUE INDEX IF NOT EXISTS idx_descriptions_product_platform
                        ON listing_descriptions(product_id, platform);",
                )
                .await?;

                // 5. listing_photos
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS listing_photos_v2 (
                        id               INTEGER PRIMARY KEY,
                        product_id       TEXT NOT NULL REFERENCES products(id),
                        platform         TEXT NOT NULL,
                        platform_item_id TEXT NOT NULL,
                        url              TEXT NOT NULL,
                        position         INTEGER NOT NULL DEFAULT 0,
                        width            INTEGER,
                        height           INTEGER,
                        alt_text         TEXT,
                        fetched_at       TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    INSERT OR IGNORE INTO listing_photos_v2 SELECT * FROM listing_photos;
                    DROP TABLE listing_photos;
                    ALTER TABLE listing_photos_v2 RENAME TO listing_photos;
                    CREATE INDEX IF NOT EXISTS idx_listing_photos_product_platform
                        ON listing_photos(product_id, platform);",
                )
                .await?;

                // 6. pricing_snapshots
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS pricing_snapshots_v2 (
                        id          INTEGER PRIMARY KEY,
                        product_id  TEXT NOT NULL REFERENCES products(id),
                        platform    TEXT NOT NULL,
                        amount      REAL NOT NULL,
                        currency    TEXT NOT NULL DEFAULT 'USD',
                        recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    INSERT OR IGNORE INTO pricing_snapshots_v2 SELECT * FROM pricing_snapshots;
                    DROP TABLE pricing_snapshots;
                    ALTER TABLE pricing_snapshots_v2 RENAME TO pricing_snapshots;
                    CREATE INDEX IF NOT EXISTS idx_pricing_snapshots_product_time
                        ON pricing_snapshots(product_id, recorded_at);",
                )
                .await?;

                // 7. vendor_listings_cache
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS vendor_listings_cache_v2 (
                        id                 INTEGER PRIMARY KEY,
                        plugin_id          TEXT NOT NULL,
                        vendor_item_id     TEXT NOT NULL,
                        title              TEXT NOT NULL,
                        price              REAL,
                        currency           TEXT,
                        quantity           INTEGER,
                        sku                TEXT,
                        image_url          TEXT,
                        url                TEXT,
                        extras_json        TEXT NOT NULL DEFAULT '{}',
                        group_key          TEXT,
                        variant_attrs_json TEXT NOT NULL DEFAULT '{}',
                        fetched_at         TEXT NOT NULL DEFAULT (datetime('now')),
                        UNIQUE(plugin_id, vendor_item_id)
                    );
                    INSERT OR IGNORE INTO vendor_listings_cache_v2 SELECT * FROM vendor_listings_cache;
                    DROP TABLE vendor_listings_cache;
                    ALTER TABLE vendor_listings_cache_v2 RENAME TO vendor_listings_cache;
                    CREATE INDEX IF NOT EXISTS idx_vlc_plugin ON vendor_listings_cache(plugin_id);",
                )
                .await?;

                debug!("Applied migration 016: rebuild all remaining AUTOINCREMENT tables for MVCC compat");
            }

            if version < 17 {
                conn.execute_batch(include_str!(
                    "../../../migrations/014_cached_platform_listings.sql"
                ))
                .await?;
                debug!("Applied migration 017: cached_platform_listings table");
            }

            // Set the new schema version
            conn.execute_batch(&format!(
                "PRAGMA user_version = {};",
                CURRENT_SCHEMA_VERSION
            ))
            .await?;

            info!("Migrations complete, schema version = {CURRENT_SCHEMA_VERSION}");
        } else {
            debug!(version, "Database schema is up to date");
        }

        Ok(())
    }

    // ── Products ──────────────────────────────────────────────

    pub async fn list_products(&self) -> Result<Vec<Product>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, canonical_sku, name, quantity, low_stock_threshold, is_tracked, created_at, updated_at, has_variants
                 FROM products ORDER BY name",
                (),
            )
            .await?;

        let mut products = Vec::new();
        while let Some(row) = rows.next().await? {
            products.push(parse_product(&row)?);
        }
        Ok(products)
    }

    pub async fn get_product(&self, id: &str) -> Result<Option<Product>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, canonical_sku, name, quantity, low_stock_threshold, is_tracked, created_at, updated_at, has_variants
                 FROM products WHERE id = ?1",
                params![id],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            Ok(Some(parse_product(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn insert_product(&self, product: &Product) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO products (id, canonical_sku, name, quantity, low_stock_threshold, is_tracked, has_variants)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                product.id.clone(),
                product.canonical_sku.clone(),
                product.name.clone(),
                product.quantity,
                product.low_stock_threshold,
                product.is_tracked,
                product.has_variants,
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn update_product_quantity(
        &self,
        product_id: &str,
        quantity: i64,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE products SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![quantity, product_id],
        )
        .await?;
        Ok(())
    }

    pub async fn update_product(
        &self,
        id: &str,
        name: &str,
        sku: &str,
        low_stock_threshold: Option<i64>,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE products SET name = ?1, canonical_sku = ?2, low_stock_threshold = ?3, updated_at = datetime('now') WHERE id = ?4",
            params![name, sku, low_stock_threshold, id],
        )
        .await?;
        Ok(())
    }

    pub async fn delete_product(&self, id: &str) -> Result<(), SyncError> {
        let p = to_params(params![id]);
        self.execute_writes(&[
            (
                "DELETE FROM platform_mappings WHERE product_id = ?1",
                p.clone(),
            ),
            (
                "DELETE FROM platform_snapshots WHERE product_id = ?1",
                p.clone(),
            ),
            (
                "DELETE FROM inventory_history WHERE product_id = ?1",
                p.clone(),
            ),
            (
                "DELETE FROM product_vendor_data WHERE product_id = ?1",
                p.clone(),
            ),
            (
                "DELETE FROM product_variants WHERE product_id = ?1",
                p.clone(),
            ),
            ("DELETE FROM products WHERE id = ?1", p),
        ])
        .await?;
        Ok(())
    }

    // ── Product Variants ───────────────────────────────────────

    pub async fn list_variants(&self, product_id: &str) -> Result<Vec<ProductVariant>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, product_id, sku, name, attributes_json, quantity, on_hand_quantity, image_url, sort_order, source_plugin_id, source_vendor_item_id, created_at, updated_at
                 FROM product_variants WHERE product_id = ?1 ORDER BY sort_order, name",
                params![product_id],
            )
            .await?;

        let mut variants = Vec::new();
        while let Some(row) = rows.next().await? {
            variants.push(parse_variant(&row)?);
        }
        Ok(variants)
    }

    pub async fn get_variant(&self, id: &str) -> Result<Option<ProductVariant>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, product_id, sku, name, attributes_json, quantity, on_hand_quantity, image_url, sort_order, source_plugin_id, source_vendor_item_id, created_at, updated_at
                 FROM product_variants WHERE id = ?1",
                params![id],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            Ok(Some(parse_variant(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn insert_variant(&self, variant: &ProductVariant) -> Result<(), SyncError> {
        let attrs_json = serde_json::to_string(&variant.attributes)
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;
        self.execute_write(
            "INSERT INTO product_variants (id, product_id, sku, name, attributes_json, quantity, on_hand_quantity, image_url, sort_order, source_plugin_id, source_vendor_item_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
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
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn update_variant(&self, variant: &ProductVariant) -> Result<(), SyncError> {
        let attrs_json = serde_json::to_string(&variant.attributes)
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;
        self.execute_write(
            "UPDATE product_variants SET sku = ?1, name = ?2, attributes_json = ?3, quantity = ?4, on_hand_quantity = ?5, image_url = ?6, sort_order = ?7, updated_at = datetime('now')
             WHERE id = ?8",
            params![
                variant.sku.clone(),
                variant.name.clone(),
                attrs_json,
                variant.quantity,
                variant.on_hand_quantity,
                variant.image_url.clone(),
                variant.sort_order as i64,
                variant.id.clone(),
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn update_variant_quantity(
        &self,
        variant_id: &str,
        qty: i64,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE product_variants SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![qty, variant_id],
        )
        .await?;
        Ok(())
    }

    pub async fn delete_variant(&self, id: &str) -> Result<(), SyncError> {
        let p = to_params(params![id]);
        self.execute_writes(&[
            (
                "DELETE FROM platform_mappings WHERE variant_id = ?1",
                p.clone(),
            ),
            ("DELETE FROM product_variants WHERE id = ?1", p),
        ])
        .await?;
        Ok(())
    }

    pub async fn delete_variants_for_product(&self, product_id: &str) -> Result<(), SyncError> {
        let p = to_params(params![product_id]);
        self.execute_writes(&[
            ("DELETE FROM platform_mappings WHERE variant_id IN (SELECT id FROM product_variants WHERE product_id = ?1)", p.clone()),
            ("DELETE FROM product_variants WHERE product_id = ?1", p),
        ]).await?;
        Ok(())
    }

    pub async fn recalc_product_quantity(&self, product_id: &str) -> Result<i64, SyncError> {
        let conn = self.connect().await?;

        // Step 1: Fetch each variant's on_hand_quantity and vendor stock contribution.
        // Done as a SELECT + per-row UPDATE because Turso/libSQL does not support
        // subqueries inside UPDATE SET clauses.
        let mut rows = conn
            .query(
                "SELECT pv.id, pv.on_hand_quantity, pv.source_plugin_id,
                        COALESCE(pvd.quantity, 0) AS vendor_qty,
                        COALESCE(vpr.include_vendor_stock, 0) AS include_vendor
                 FROM product_variants pv
                 LEFT JOIN product_vendor_data pvd
                   ON pvd.variant_id = pv.id AND pvd.plugin_id = pv.source_plugin_id
                 LEFT JOIN vendor_plugin_registry vpr
                   ON vpr.id = pv.source_plugin_id
                 WHERE pv.product_id = ?1",
                params![product_id],
            )
            .await?;

        let mut updates: Vec<(String, i64)> = Vec::new();
        while let Some(row) = rows.next().await? {
            let variant_id: String = row.get(0)?;
            let on_hand: i64 = row.get(1)?;
            let source_plugin: Option<String> = row.get::<Option<String>>(2)?;
            let vendor_qty: i64 = row.get(3)?;
            let include_vendor: i64 = row.get(4)?;

            let effective = if source_plugin.is_some() && include_vendor == 1 {
                on_hand + vendor_qty
            } else {
                on_hand
            };
            updates.push((variant_id, effective));
        }

        // Step 2: Update each variant's effective quantity + product total atomically
        let total: i64 = updates.iter().map(|(_, q)| q).sum();
        let mut stmts: Vec<(&str, Params)> = Vec::new();
        for (variant_id, quantity) in &updates {
            stmts.push((
                "UPDATE product_variants SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2",
                to_params(params![*quantity, variant_id.as_str()]),
            ));
        }
        stmts.push((
            "UPDATE products SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2",
            to_params(params![total, product_id]),
        ));
        self.execute_writes(&stmts).await?;

        Ok(total)
    }

    /// Recalculate quantities for all products that have variants sourced from the given plugin.
    /// Call after toggling include_vendor_stock or after a vendor sync.
    pub async fn recalc_products_for_plugin(&self, plugin_id: &str) -> Result<usize, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT DISTINCT product_id FROM product_variants WHERE source_plugin_id = ?1",
                params![plugin_id],
            )
            .await?;

        let mut product_ids = Vec::new();
        while let Some(row) = rows.next().await? {
            product_ids.push(row.get::<String>(0)?);
        }

        for pid in &product_ids {
            self.recalc_product_quantity(pid).await?;
        }
        Ok(product_ids.len())
    }

    /// Recalculate quantities for ALL products that have any vendor-sourced variants.
    /// Called once at startup to ensure persisted product_vendor_data is applied before
    /// the sync engine begins polling platforms.
    pub async fn recalc_all_vendor_sourced_products(&self) -> Result<usize, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT DISTINCT product_id FROM product_variants WHERE source_plugin_id IS NOT NULL",
                (),
            )
            .await?;

        let mut product_ids = Vec::new();
        while let Some(row) = rows.next().await? {
            product_ids.push(row.get::<String>(0)?);
        }

        for pid in &product_ids {
            self.recalc_product_quantity(pid).await?;
        }
        Ok(product_ids.len())
    }

    /// Set the include_vendor_stock flag on a vendor plugin.
    pub async fn set_plugin_include_vendor_stock(
        &self,
        plugin_id: &str,
        include: bool,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE vendor_plugin_registry SET include_vendor_stock = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![include as i64, plugin_id],
        )
        .await?;
        Ok(())
    }

    pub async fn list_all_variants(&self) -> Result<Vec<ProductVariant>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, product_id, sku, name, attributes_json, quantity, on_hand_quantity, image_url, sort_order, source_plugin_id, source_vendor_item_id, created_at, updated_at
                 FROM product_variants ORDER BY product_id, sort_order",
                (),
            )
            .await?;

        let mut variants = Vec::new();
        while let Some(row) = rows.next().await? {
            variants.push(parse_variant(&row)?);
        }
        Ok(variants)
    }

    pub async fn get_all_variant_skus(&self) -> Result<Vec<(String, String, String)>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, product_id, sku FROM product_variants ORDER BY product_id",
                (),
            )
            .await?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            result.push((
                row.get::<String>(0)?,
                row.get::<String>(1)?,
                row.get::<String>(2)?,
            ));
        }
        Ok(result)
    }

    // ── Platform Mappings ─────────────────────────────────────

    pub async fn list_mappings_for_product(
        &self,
        product_id: &str,
    ) -> Result<Vec<PlatformMapping>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, product_id, platform, platform_item_id, platform_sku, is_active, variant_id
                 FROM platform_mappings WHERE product_id = ?1 AND deleted_at IS NULL",
                params![product_id],
            )
            .await?;

        let mut mappings = Vec::new();
        while let Some(row) = rows.next().await? {
            mappings.push(parse_mapping(&row)?);
        }
        Ok(mappings)
    }

    pub async fn list_all_active_mappings(&self) -> Result<Vec<PlatformMapping>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, product_id, platform, platform_item_id, platform_sku, is_active, variant_id
                 FROM platform_mappings WHERE is_active = TRUE AND deleted_at IS NULL",
                (),
            )
            .await?;

        let mut mappings = Vec::new();
        while let Some(row) = rows.next().await? {
            mappings.push(parse_mapping(&row)?);
        }
        Ok(mappings)
    }

    pub async fn get_mapping_by_platform_item(
        &self,
        platform: &str,
        platform_item_id: &str,
    ) -> Result<Option<PlatformMapping>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, product_id, platform, platform_item_id, platform_sku, is_active, variant_id
                 FROM platform_mappings WHERE platform = ?1 AND platform_item_id = ?2 AND deleted_at IS NULL",
                params![platform, platform_item_id],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            Ok(Some(parse_mapping(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn insert_mapping(
        &self,
        product_id: &str,
        platform: &str,
        platform_item_id: &str,
        platform_sku: Option<&str>,
        variant_id: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO platform_mappings (product_id, platform, platform_item_id, platform_sku, variant_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            params![product_id, platform, platform_item_id, platform_sku, variant_id],
        )
        .await?;
        Ok(())
    }

    pub async fn delete_mapping(&self, id: i64) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE platform_mappings SET deleted_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )
        .await?;
        Ok(())
    }

    // ── Platform Snapshots ────────────────────────────────────

    pub async fn get_snapshot(
        &self,
        product_id: &str,
        platform: &str,
    ) -> Result<Option<PlatformSnapshot>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, product_id, platform, last_known_quantity, last_polled_at, last_pushed_at, version_tag
                 FROM platform_snapshots WHERE product_id = ?1 AND platform = ?2",
                params![product_id, platform],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            Ok(Some(PlatformSnapshot {
                id: row.get::<i64>(0)?,
                product_id: row.get::<String>(1)?,
                platform: Platform::from_str_loose(&row.get::<String>(2)?)
                    .unwrap_or(Platform::Ebay),
                last_known_quantity: row.get::<i64>(3)?,
                last_polled_at: row.get::<String>(4)?,
                last_pushed_at: row.get::<Option<String>>(5)?,
                version_tag: row.get::<Option<String>>(6)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn upsert_snapshot(
        &self,
        product_id: &str,
        platform: &str,
        quantity: i64,
        polled_at: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO platform_snapshots (product_id, platform, last_known_quantity, last_polled_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(product_id, platform) DO UPDATE
             SET last_known_quantity = ?3, last_polled_at = ?4",
            params![product_id, platform, quantity, polled_at],
        )
        .await?;
        Ok(())
    }

    pub async fn update_snapshot_pushed(
        &self,
        product_id: &str,
        platform: &str,
        quantity: i64,
        pushed_at: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE platform_snapshots
             SET last_known_quantity = ?1, last_pushed_at = ?2
             WHERE product_id = ?3 AND platform = ?4",
            params![quantity, pushed_at, product_id, platform],
        )
        .await?;
        Ok(())
    }

    // ── Sync Events ───────────────────────────────────────────

    pub async fn insert_sync_event(&self, event: &NewSyncEvent<'_>) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO sync_events (product_id, source_platform, target_platform, old_quantity, new_quantity, event_type, sync_cycle_id, error_message)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![event.product_id, event.source_platform, event.target_platform, event.old_quantity, event.new_quantity, event.event_type, event.sync_cycle_id, event.error_message],
        )
        .await?;
        Ok(())
    }

    pub async fn list_recent_sync_events(&self, limit: i64) -> Result<Vec<SyncEvent>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, product_id, source_platform, target_platform, old_quantity, new_quantity, event_type, sync_cycle_id, error_message, created_at
                 FROM sync_events ORDER BY created_at DESC LIMIT ?1",
                params![limit],
            )
            .await?;

        let mut events = Vec::new();
        while let Some(row) = rows.next().await? {
            events.push(SyncEvent {
                id: row.get::<i64>(0)?,
                product_id: row.get::<String>(1)?,
                source_platform: row.get::<String>(2)?,
                target_platform: row.get::<String>(3)?,
                old_quantity: row.get::<i64>(4)?,
                new_quantity: row.get::<i64>(5)?,
                event_type: row.get::<String>(6)?,
                sync_cycle_id: row.get::<String>(7)?,
                error_message: row.get::<Option<String>>(8)?,
                created_at: row.get::<String>(9)?,
            });
        }
        Ok(events)
    }

    pub async fn prune_old_sync_events(&self, days: i64) -> Result<u64, SyncError> {
        let result = self
            .execute_write(
                "DELETE FROM sync_events WHERE created_at < datetime('now', ?1)",
                params![format!("-{days} days")],
            )
            .await?;
        Ok(result)
    }

    // ── Auth Tokens ───────────────────────────────────────────

    pub async fn get_auth_token(&self, platform: &str) -> Result<Option<AuthToken>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT platform, access_token, refresh_token, expires_at, cookies, updated_at
                 FROM auth_tokens WHERE platform = ?1",
                params![platform],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            Ok(Some(AuthToken {
                platform: Platform::from_str_loose(&row.get::<String>(0)?)
                    .unwrap_or(Platform::Ebay),
                access_token: row.get::<String>(1)?,
                refresh_token: row.get::<Option<String>>(2)?,
                expires_at: row.get::<Option<String>>(3)?,
                cookies: row.get::<Option<String>>(4)?,
                updated_at: row.get::<String>(5)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn upsert_auth_token(
        &self,
        platform: &str,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_at: Option<&str>,
        cookies: Option<&str>,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO auth_tokens (platform, access_token, refresh_token, expires_at, cookies, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
             ON CONFLICT(platform) DO UPDATE
             SET access_token = ?2, refresh_token = ?3, expires_at = ?4, cookies = ?5, updated_at = datetime('now')",
            params![platform, access_token, refresh_token, expires_at, cookies],
        )
        .await?;
        Ok(())
    }

    // ── Inventory History ─────────────────────────────────────

    pub async fn record_inventory_history(
        &self,
        product_id: &str,
        quantity: i64,
        sync_cycle_id: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO inventory_history (product_id, quantity, sync_cycle_id)
             VALUES (?1, ?2, ?3)",
            params![product_id, quantity, sync_cycle_id],
        )
        .await?;
        Ok(())
    }

    pub async fn get_inventory_history(
        &self,
        product_id: &str,
        interval: Option<&str>,
    ) -> Result<Vec<InventoryHistoryEntry>, SyncError> {
        let conn = self.connect().await?;

        let (sql, use_param) = match interval {
            Some(_interval) => (
                "SELECT id, product_id, quantity, sync_cycle_id, recorded_at
                 FROM inventory_history
                 WHERE product_id = ?1 AND recorded_at >= datetime('now', ?2)
                 ORDER BY recorded_at ASC",
                true,
            ),
            None => (
                "SELECT id, product_id, quantity, sync_cycle_id, recorded_at
                 FROM inventory_history
                 WHERE product_id = ?1
                 ORDER BY recorded_at ASC",
                false,
            ),
        };

        let mut rows = if use_param {
            conn.query(sql, params![product_id, interval.unwrap_or("")])
                .await?
        } else {
            conn.query(sql, params![product_id]).await?
        };

        let mut entries = Vec::new();
        while let Some(row) = rows.next().await? {
            entries.push(InventoryHistoryEntry {
                id: row.get::<i64>(0)?,
                product_id: row.get::<String>(1)?,
                quantity: row.get::<i64>(2)?,
                sync_cycle_id: row.get::<String>(3)?,
                recorded_at: row.get::<String>(4)?,
            });
        }
        Ok(entries)
    }

    // ── Listing Descriptions ──────────────────────────────────

    pub async fn upsert_listing_description(
        &self,
        product_id: &str,
        platform: &str,
        platform_item_id: &str,
        description_html: Option<&str>,
        description_text: Option<&str>,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO listing_descriptions (product_id, platform, platform_item_id, description_html, description_text, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
             ON CONFLICT(product_id, platform) DO UPDATE
             SET platform_item_id = ?3, description_html = ?4, description_text = ?5, fetched_at = datetime('now')",
            params![product_id, platform, platform_item_id, description_html, description_text],
        )
        .await?;
        Ok(())
    }

    pub async fn get_listing_descriptions(
        &self,
        product_id: &str,
    ) -> Result<Vec<ListingDescription>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT platform, platform_item_id, description_html, description_text, fetched_at
                 FROM listing_descriptions WHERE product_id = ?1",
                params![product_id],
            )
            .await?;

        let mut descriptions = Vec::new();
        while let Some(row) = rows.next().await? {
            descriptions.push(ListingDescription {
                platform: Platform::from_str_loose(&row.get::<String>(0)?)
                    .unwrap_or(Platform::Ebay),
                platform_item_id: row.get::<String>(1)?,
                html: row.get::<Option<String>>(2)?,
                plain_text: row.get::<Option<String>>(3)?,
                fetched_at: row.get::<String>(4)?,
            });
        }
        Ok(descriptions)
    }

    // ── Listing Photos ───────────────────────────────────────

    pub async fn replace_listing_photos(
        &self,
        product_id: &str,
        platform: &str,
        platform_item_id: &str,
        photos: &[ListingPhoto],
    ) -> Result<(), SyncError> {
        let mut stmts: Vec<(&str, Params)> = vec![(
            "DELETE FROM listing_photos WHERE product_id = ?1 AND platform = ?2",
            to_params(params![product_id, platform]),
        )];
        for photo in photos {
            stmts.push((
                "INSERT INTO listing_photos (product_id, platform, platform_item_id, url, position, width, height, alt_text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                to_params(params![
                    product_id,
                    platform,
                    platform_item_id,
                    photo.url.clone(),
                    photo.position as i64,
                    photo.width.map(|w| w as i64),
                    photo.height.map(|h| h as i64),
                    photo.alt_text.clone(),
                ]),
            ));
        }
        self.execute_writes(&stmts).await?;
        Ok(())
    }

    pub async fn get_listing_photos(
        &self,
        product_id: &str,
        platform: Option<&str>,
    ) -> Result<Vec<(Platform, ListingPhoto)>, SyncError> {
        let conn = self.connect().await?;

        let (sql, use_platform) = match platform {
            Some(_) => (
                "SELECT platform, url, position, width, height, alt_text
                 FROM listing_photos WHERE product_id = ?1 AND platform = ?2
                 ORDER BY position",
                true,
            ),
            None => (
                "SELECT platform, url, position, width, height, alt_text
                 FROM listing_photos WHERE product_id = ?1
                 ORDER BY platform, position",
                false,
            ),
        };

        let mut rows = if use_platform {
            conn.query(sql, params![product_id, platform.unwrap_or("")])
                .await?
        } else {
            conn.query(sql, params![product_id]).await?
        };

        let mut photos = Vec::new();
        while let Some(row) = rows.next().await? {
            let plat = Platform::from_str_loose(&row.get::<String>(0)?).unwrap_or(Platform::Ebay);
            photos.push((
                plat,
                ListingPhoto {
                    url: row.get::<String>(1)?,
                    position: row.get::<i64>(2)? as i32,
                    width: row.get::<Option<i64>>(3)?.map(|v| v as u32),
                    height: row.get::<Option<i64>>(4)?.map(|v| v as u32),
                    alt_text: row.get::<Option<String>>(5)?,
                },
            ));
        }
        Ok(photos)
    }

    /// Get the first photo URL for every product (for grid thumbnails).
    /// Returns (product_id, url) pairs — one per product (lowest position).
    pub async fn get_all_product_first_photos(&self) -> Result<Vec<(String, String)>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT lp.product_id, lp.url
                 FROM listing_photos lp
                 INNER JOIN (
                     SELECT product_id, MIN(position) as min_pos
                     FROM listing_photos
                     GROUP BY product_id
                 ) first ON lp.product_id = first.product_id
                     AND lp.position = first.min_pos
                 GROUP BY lp.product_id",
                (),
            )
            .await?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            result.push((row.get::<String>(0)?, row.get::<String>(1)?));
        }
        Ok(result)
    }

    /// Upsert a single listing photo (used during fetch_all_listings to cache images).
    /// Deletes any existing photo at the same product+platform+position, then inserts.
    pub async fn save_listing_photo(
        &self,
        product_id: &str,
        platform: &str,
        platform_item_id: &str,
        url: &str,
        position: i32,
    ) -> Result<(), SyncError> {
        self.execute_writes(&[
            (
                "DELETE FROM listing_photos WHERE product_id = ?1 AND platform = ?2 AND position = ?3",
                to_params(params![product_id, platform, position as i64]),
            ),
            (
                "INSERT INTO listing_photos (product_id, platform, platform_item_id, url, position)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                to_params(params![product_id, platform, platform_item_id, url, position as i64]),
            ),
        ]).await?;
        Ok(())
    }

    // ── Pricing Snapshots ────────────────────────────────────

    pub async fn record_pricing_snapshot(
        &self,
        product_id: &str,
        platform: &str,
        amount: f64,
        currency: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO pricing_snapshots (product_id, platform, amount, currency)
             VALUES (?1, ?2, ?3, ?4)",
            params![product_id, platform, amount, currency],
        )
        .await?;
        Ok(())
    }

    pub async fn get_pricing_snapshots(
        &self,
        product_id: &str,
        interval: Option<&str>,
    ) -> Result<Vec<PricingSnapshot>, SyncError> {
        let conn = self.connect().await?;

        let (sql, use_interval) = match interval {
            Some(_) => (
                "SELECT id, product_id, platform, amount, currency, recorded_at
                 FROM pricing_snapshots
                 WHERE product_id = ?1 AND recorded_at >= datetime('now', ?2)
                 ORDER BY recorded_at ASC",
                true,
            ),
            None => (
                "SELECT id, product_id, platform, amount, currency, recorded_at
                 FROM pricing_snapshots
                 WHERE product_id = ?1
                 ORDER BY recorded_at ASC",
                false,
            ),
        };

        let mut rows = if use_interval {
            conn.query(sql, params![product_id, interval.unwrap_or("")])
                .await?
        } else {
            conn.query(sql, params![product_id]).await?
        };

        let mut snapshots = Vec::new();
        while let Some(row) = rows.next().await? {
            snapshots.push(PricingSnapshot {
                id: row.get::<i64>(0)?,
                product_id: row.get::<String>(1)?,
                platform: Platform::from_str_loose(&row.get::<String>(2)?)
                    .unwrap_or(Platform::Ebay),
                amount: row.get::<f64>(3)?,
                currency: row.get::<String>(4)?,
                recorded_at: row.get::<String>(5)?,
            });
        }
        Ok(snapshots)
    }

    pub async fn get_latest_prices(
        &self,
        product_id: &str,
    ) -> Result<Vec<PricingSnapshot>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT ps.id, ps.product_id, ps.platform, ps.amount, ps.currency, ps.recorded_at
                 FROM pricing_snapshots ps
                 INNER JOIN (
                     SELECT product_id, platform, MAX(recorded_at) as max_time
                     FROM pricing_snapshots
                     WHERE product_id = ?1
                     GROUP BY product_id, platform
                 ) latest ON ps.product_id = latest.product_id
                     AND ps.platform = latest.platform
                     AND ps.recorded_at = latest.max_time",
                params![product_id],
            )
            .await?;

        let mut prices = Vec::new();
        while let Some(row) = rows.next().await? {
            prices.push(PricingSnapshot {
                id: row.get::<i64>(0)?,
                product_id: row.get::<String>(1)?,
                platform: Platform::from_str_loose(&row.get::<String>(2)?)
                    .unwrap_or(Platform::Ebay),
                amount: row.get::<f64>(3)?,
                currency: row.get::<String>(4)?,
                recorded_at: row.get::<String>(5)?,
            });
        }
        Ok(prices)
    }

    /// Get the latest price per platform for ALL products (bulk query for product cards).
    pub async fn get_all_latest_prices(&self) -> Result<Vec<PricingSnapshot>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT ps.id, ps.product_id, ps.platform, ps.amount, ps.currency, ps.recorded_at
                 FROM pricing_snapshots ps
                 INNER JOIN (
                     SELECT product_id, platform, MAX(recorded_at) as max_time
                     FROM pricing_snapshots
                     GROUP BY product_id, platform
                 ) latest ON ps.product_id = latest.product_id
                     AND ps.platform = latest.platform
                     AND ps.recorded_at = latest.max_time",
                (),
            )
            .await?;

        let mut prices = Vec::new();
        while let Some(row) = rows.next().await? {
            prices.push(PricingSnapshot {
                id: row.get::<i64>(0)?,
                product_id: row.get::<String>(1)?,
                platform: Platform::from_str_loose(&row.get::<String>(2)?)
                    .unwrap_or(Platform::Ebay),
                amount: row.get::<f64>(3)?,
                currency: row.get::<String>(4)?,
                recorded_at: row.get::<String>(5)?,
            });
        }
        Ok(prices)
    }

    // ── Cached Full Listings ────────────────────────────────────

    /// Store a full listing as JSON for instant loading on product detail page.
    pub async fn save_cached_listing(
        &self,
        product_id: &str,
        platform: &str,
        platform_item_id: &str,
        listing_json: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT OR REPLACE INTO cached_listings (product_id, platform, platform_item_id, listing_json, fetched_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            params![product_id, platform, platform_item_id, listing_json],
        )
        .await?;
        Ok(())
    }

    /// Load all cached full listings for a product (instant, no API calls).
    pub async fn get_cached_listings(&self, product_id: &str) -> Result<Vec<String>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT listing_json FROM cached_listings WHERE product_id = ?1",
                params![product_id],
            )
            .await?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(row.get::<String>(0)?);
        }
        Ok(out)
    }

    // ── Cached Platform Listings (for Listings page) ──────────

    /// Load cached platform listings JSON for a given platform.
    pub async fn get_cached_platform_listings(
        &self,
        platform: &str,
    ) -> Result<Option<String>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT listings_json FROM cached_platform_listings WHERE platform = ?1",
                params![platform],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some(row.get::<String>(0)?))
        } else {
            Ok(None)
        }
    }

    /// Save platform listings JSON for a given platform.
    pub async fn save_cached_platform_listings(
        &self,
        platform: &str,
        listings_json: &str,
    ) -> Result<(), SyncError> {
        let conn = self.connect().await?;
        conn.execute(
            "INSERT INTO cached_platform_listings (platform, listings_json, cached_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(platform) DO UPDATE SET listings_json = ?2, cached_at = datetime('now')",
            params![platform, listings_json],
        )
        .await?;
        Ok(())
    }

    // ── Cache Freshness ───────────────────────────────────────

    /// Get the most recent fetched/recorded timestamp across listing_photos,
    /// listing_descriptions, and pricing_snapshots for a single product.
    pub async fn get_product_cache_freshness(
        &self,
        product_id: &str,
    ) -> Result<Option<String>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT MAX(ts) FROM (
                     SELECT MAX(fetched_at) AS ts FROM listing_photos WHERE product_id = ?1
                     UNION ALL
                     SELECT MAX(fetched_at) AS ts FROM listing_descriptions WHERE product_id = ?1
                     UNION ALL
                     SELECT MAX(recorded_at) AS ts FROM pricing_snapshots WHERE product_id = ?1
                 )",
                params![product_id],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(row.get::<Option<String>>(0)?)
        } else {
            Ok(None)
        }
    }

    /// Get cache freshness for ALL products (bulk query for product grid).
    /// Returns (product_id, most_recent_timestamp) pairs.
    pub async fn get_all_cache_freshness(&self) -> Result<Vec<(String, String)>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT product_id, MAX(ts) FROM (
                     SELECT product_id, MAX(fetched_at) AS ts FROM listing_photos GROUP BY product_id
                     UNION ALL
                     SELECT product_id, MAX(fetched_at) AS ts FROM listing_descriptions GROUP BY product_id
                     UNION ALL
                     SELECT product_id, MAX(recorded_at) AS ts FROM pricing_snapshots GROUP BY product_id
                 ) GROUP BY product_id",
                (),
            )
            .await?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            if let (Ok(pid), Ok(Some(ts))) = (row.get::<String>(0), row.get::<Option<String>>(1)) {
                result.push((pid, ts));
            }
        }
        Ok(result)
    }

    // ── XMR Processed Orders (sale deduplication) ───────────

    pub async fn is_xmr_order_processed(&self, order_id: &str) -> Result<bool, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT 1 FROM xmr_processed_orders WHERE order_id = ?1",
                params![order_id],
            )
            .await?;
        Ok(rows.next().await?.is_some())
    }

    pub async fn mark_xmr_order_processed(
        &self,
        order_id: &str,
        product_id: Option<&str>,
        listing_id: &str,
        quantity: i64,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT OR IGNORE INTO xmr_processed_orders (order_id, product_id, listing_id, quantity)
             VALUES (?1, ?2, ?3, ?4)",
            params![order_id, product_id, listing_id, quantity],
        )
        .await?;
        Ok(())
    }

    pub async fn update_snapshot_version_tag(
        &self,
        product_id: &str,
        platform: &str,
        version_tag: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE platform_snapshots SET version_tag = ?1
             WHERE product_id = ?2 AND platform = ?3",
            params![version_tag, product_id, platform],
        )
        .await?;
        Ok(())
    }

    // ── History Downsampling ─────────────────────────────────

    pub async fn downsample_old_history(&self, days: i64) -> Result<u64, SyncError> {
        let result = self
            .execute_write(
                "DELETE FROM inventory_history
             WHERE recorded_at < datetime('now', ?1)
             AND id NOT IN (
                 SELECT MIN(id) FROM inventory_history
                 WHERE recorded_at < datetime('now', ?1)
                 GROUP BY product_id, date(recorded_at)
             )",
                params![format!("-{days} days")],
            )
            .await?;
        Ok(result)
    }

    // ── Export / Import ──────────────────────────────────────

    pub async fn list_all_mappings(&self) -> Result<Vec<PlatformMapping>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, product_id, platform, platform_item_id, platform_sku, is_active, variant_id
                 FROM platform_mappings",
                (),
            )
            .await?;

        let mut mappings = Vec::new();
        while let Some(row) = rows.next().await? {
            mappings.push(parse_mapping(&row)?);
        }
        Ok(mappings)
    }

    pub async fn list_all_snapshots(&self) -> Result<Vec<PlatformSnapshot>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, product_id, platform, last_known_quantity, last_polled_at, last_pushed_at, version_tag
                 FROM platform_snapshots",
                (),
            )
            .await?;

        let mut snapshots = Vec::new();
        while let Some(row) = rows.next().await? {
            snapshots.push(PlatformSnapshot {
                id: row.get::<i64>(0)?,
                product_id: row.get::<String>(1)?,
                platform: Platform::from_str_loose(&row.get::<String>(2)?)
                    .unwrap_or(Platform::Ebay),
                last_known_quantity: row.get::<i64>(3)?,
                last_polled_at: row.get::<String>(4)?,
                last_pushed_at: row.get::<Option<String>>(5)?,
                version_tag: row.get::<Option<String>>(6)?,
            });
        }
        Ok(snapshots)
    }

    pub async fn list_xmr_processed_orders(&self) -> Result<Vec<XmrProcessedOrder>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT order_id, product_id, listing_id, quantity, processed_at
                 FROM xmr_processed_orders",
                (),
            )
            .await?;

        let mut orders = Vec::new();
        while let Some(row) = rows.next().await? {
            orders.push(XmrProcessedOrder {
                order_id: row.get::<String>(0)?,
                product_id: row.get::<Option<String>>(1)?,
                listing_id: row.get::<String>(2)?,
                quantity: row.get::<i64>(3)?,
                processed_at: row.get::<String>(4)?,
            });
        }
        Ok(orders)
    }

    // ── P2P Company ────────────────────────────────────────────

    pub async fn get_company(
        &self,
    ) -> Result<Option<(String, String, String, Option<String>, String)>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, name, secret, our_onion, role FROM company LIMIT 1",
                (),
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some((
                row.get::<String>(0)?,
                row.get::<String>(1)?,
                row.get::<String>(2)?,
                row.get::<Option<String>>(3)?,
                row.get::<String>(4)?,
            )))
        } else {
            Ok(None)
        }
    }

    pub async fn create_company(
        &self,
        id: &str,
        name: &str,
        secret: &str,
        role: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO company (id, name, secret, role) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, secret, role],
        )
        .await?;
        Ok(())
    }

    pub async fn update_company_secret(&self, secret: &str) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE company SET secret = ?1, updated_at = datetime('now')",
            params![secret],
        )
        .await?;
        Ok(())
    }

    pub async fn update_company_onion(&self, onion: &str) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE company SET our_onion = ?1, updated_at = datetime('now')",
            params![onion],
        )
        .await?;
        Ok(())
    }

    pub async fn delete_company(&self) -> Result<(), SyncError> {
        let p = to_params(());
        self.execute_writes(&[
            ("DELETE FROM company", p.clone()),
            ("DELETE FROM company_peers", p.clone()),
            ("DELETE FROM p2p_sync_state", p.clone()),
            ("DELETE FROM p2p_meta", p),
        ])
        .await?;
        Ok(())
    }

    // ── P2P Peers ─────────────────────────────────────────────

    pub async fn list_peers(
        &self,
    ) -> Result<Vec<(String, String, String, bool, String, Option<String>)>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT onion_address, name, role, is_authorized, added_at, last_seen_at
                 FROM company_peers ORDER BY added_at",
                (),
            )
            .await?;

        let mut peers = Vec::new();
        while let Some(row) = rows.next().await? {
            peers.push((
                row.get::<String>(0)?,
                row.get::<String>(1)?,
                row.get::<String>(2)?,
                row.get::<bool>(3)?,
                row.get::<String>(4)?,
                row.get::<Option<String>>(5)?,
            ));
        }
        Ok(peers)
    }

    pub async fn get_peer(
        &self,
        onion_address: &str,
    ) -> Result<Option<(String, String, String, bool)>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT onion_address, name, role, is_authorized FROM company_peers WHERE onion_address = ?1",
                params![onion_address],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some((
                row.get::<String>(0)?,
                row.get::<String>(1)?,
                row.get::<String>(2)?,
                row.get::<bool>(3)?,
            )))
        } else {
            Ok(None)
        }
    }

    pub async fn insert_peer(
        &self,
        onion_address: &str,
        name: &str,
        role: &str,
        is_authorized: bool,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT OR IGNORE INTO company_peers (onion_address, name, role, is_authorized)
             VALUES (?1, ?2, ?3, ?4)",
            params![onion_address, name, role, is_authorized],
        )
        .await?;
        Ok(())
    }

    pub async fn remove_peer(&self, onion_address: &str) -> Result<(), SyncError> {
        let p = to_params(params![onion_address]);
        self.execute_writes(&[
            (
                "DELETE FROM company_peers WHERE onion_address = ?1",
                p.clone(),
            ),
            ("DELETE FROM p2p_sync_state WHERE peer_address = ?1", p),
        ])
        .await?;
        Ok(())
    }

    pub async fn update_peer_last_seen(&self, onion_address: &str) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE company_peers SET last_seen_at = datetime('now') WHERE onion_address = ?1",
            params![onion_address],
        )
        .await?;
        Ok(())
    }

    pub async fn approve_peer(&self, onion_address: &str) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE company_peers SET is_authorized = 1 WHERE onion_address = ?1",
            params![onion_address],
        )
        .await?;
        Ok(())
    }

    pub async fn reject_peer(&self, onion_address: &str) -> Result<(), SyncError> {
        self.remove_peer(onion_address).await
    }

    pub async fn update_peer_role(&self, onion_address: &str, role: &str) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE company_peers SET role = ?1 WHERE onion_address = ?2",
            params![role, onion_address],
        )
        .await?;
        Ok(())
    }

    pub async fn update_company_role(&self, role: &str) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE company SET role = ?1, updated_at = datetime('now')",
            params![role],
        )
        .await?;
        Ok(())
    }

    // ── P2P Sync State ────────────────────────────────────────

    pub async fn get_sync_state(
        &self,
        peer_address: &str,
    ) -> Result<Option<(String, Option<String>, bool, Option<String>, i64)>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT peer_address, last_synced_at, last_sync_success, last_error, sync_count
                 FROM p2p_sync_state WHERE peer_address = ?1",
                params![peer_address],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some((
                row.get::<String>(0)?,
                row.get::<Option<String>>(1)?,
                row.get::<bool>(2)?,
                row.get::<Option<String>>(3)?,
                row.get::<i64>(4)?,
            )))
        } else {
            Ok(None)
        }
    }

    pub async fn update_sync_state(
        &self,
        peer_address: &str,
        success: bool,
        error: Option<&str>,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO p2p_sync_state (peer_address, last_synced_at, last_sync_success, last_error, sync_count)
             VALUES (?1, datetime('now'), ?2, ?3, 1)
             ON CONFLICT(peer_address) DO UPDATE
             SET last_synced_at = datetime('now'), last_sync_success = ?2, last_error = ?3, sync_count = sync_count + 1",
            params![peer_address, success, error],
        )
        .await?;
        Ok(())
    }

    // ── P2P Meta ──────────────────────────────────────────────

    pub async fn get_p2p_meta(&self, key: &str) -> Result<Option<String>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query("SELECT value FROM p2p_meta WHERE key = ?1", params![key])
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some(row.get::<String>(0)?))
        } else {
            Ok(None)
        }
    }

    pub async fn set_p2p_meta(&self, key: &str, value: &str) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO p2p_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )
        .await?;
        Ok(())
    }

    // ── Syncable Mappings (includes soft-deleted for P2P) ────

    pub async fn list_all_mappings_for_sync(
        &self,
    ) -> Result<
        Vec<(
            i64,
            String,
            String,
            String,
            Option<String>,
            bool,
            Option<String>,
            String,
            Option<String>,
        )>,
        SyncError,
    > {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, product_id, platform, platform_item_id, platform_sku, is_active, deleted_at, updated_at, variant_id
                 FROM platform_mappings",
                (),
            )
            .await?;

        let mut mappings = Vec::new();
        while let Some(row) = rows.next().await? {
            mappings.push((
                row.get::<i64>(0)?,
                row.get::<String>(1)?,
                row.get::<String>(2)?,
                row.get::<String>(3)?,
                row.get::<Option<String>>(4)?,
                row.get::<bool>(5)?,
                row.get::<Option<String>>(6)?,
                row.get::<String>(7)?,
                row.get::<Option<String>>(8)?,
            ));
        }
        Ok(mappings)
    }

    // ── Migration helpers ─────────────────────────────────────

    /// Migration 10: ensure every product has at least one variant row.
    /// Products with has_variants=0 and no variant rows get a "Default" variant
    /// created from their canonical_sku and quantity. Then has_variants is set to 1.
    async fn migrate_always_variants(&self) -> Result<(), SyncError> {
        let conn = self.connect().await?;

        // Find products that have no variant rows
        let mut rows = conn
            .query(
                "SELECT id, canonical_sku, quantity FROM products
                 WHERE id NOT IN (SELECT DISTINCT product_id FROM product_variants)",
                (),
            )
            .await?;

        let mut to_fix: Vec<(String, String, i64)> = Vec::new();
        while let Some(row) = rows.next().await? {
            to_fix.push((
                row.get::<String>(0)?,
                row.get::<String>(1)?,
                row.get::<i64>(2)?,
            ));
        }

        for (product_id, sku, quantity) in &to_fix {
            let variant_id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO product_variants (id, product_id, sku, name, attributes_json, quantity, sort_order)
                 VALUES (?1, ?2, ?3, 'Default', '{}', ?4, 0)",
                params![variant_id, product_id.clone(), sku.clone(), *quantity],
            )
            .await?;
        }

        // Set all products to has_variants = 1
        conn.execute(
            "UPDATE products SET has_variants = 1, updated_at = datetime('now') WHERE has_variants = 0",
            (),
        )
        .await?;

        if !to_fix.is_empty() {
            info!(
                count = to_fix.len(),
                "Created default variants for simple products"
            );
        }

        Ok(())
    }

    async fn migrate_peers_to_v2(&self) -> Result<(), SyncError> {
        let conn = self.connect().await?;
        // Check if the old company_peers table exists
        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='company_peers'",
                (),
            )
            .await?;
        if rows.next().await?.is_none() {
            return Ok(()); // No old peers table
        }

        // Check if there are any rows in company_peers
        let mut rows = conn.query("SELECT COUNT(*) FROM company_peers", ()).await?;
        let count: i64 = if let Some(row) = rows.next().await? {
            row.get::<i64>(0)?
        } else {
            0
        };

        if count == 0 {
            return Ok(());
        }

        // Migrate each peer with a generated UUID as peer_id
        let mut rows = conn
            .query(
                "SELECT onion_address, name, role, is_authorized, added_at, last_seen_at FROM company_peers",
                (),
            )
            .await?;

        let mut peers = Vec::new();
        while let Some(row) = rows.next().await? {
            peers.push((
                row.get::<String>(0)?,
                row.get::<String>(1)?,
                row.get::<String>(2)?,
                row.get::<bool>(3)?,
                row.get::<String>(4)?,
                row.get::<Option<String>>(5)?,
            ));
        }

        for (addr, name, role, is_auth, added, last_seen) in peers {
            let peer_id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT OR IGNORE INTO company_peers_v2 (peer_id, onion_address, name, role, is_authorized, added_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![peer_id, addr, name, role, is_auth, added, last_seen],
            )
            .await?;
        }

        info!("Migrated {} peers to company_peers_v2", count);
        Ok(())
    }

    async fn add_company_peer_id_column(&self) -> Result<(), SyncError> {
        let conn = self.connect().await?;
        // Check if company table exists
        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='company'",
                (),
            )
            .await?;
        if rows.next().await?.is_none() {
            return Ok(());
        }

        // Check if peer_id column already exists
        let mut rows = conn.query("PRAGMA table_info(company)", ()).await?;
        let mut has_peer_id = false;
        while let Some(row) = rows.next().await? {
            let col_name: String = row.get(1)?;
            if col_name == "peer_id" {
                has_peer_id = true;
                break;
            }
        }

        if !has_peer_id {
            conn.execute_batch("ALTER TABLE company ADD COLUMN peer_id TEXT NOT NULL DEFAULT ''")
                .await?;
            // Generate a peer_id for existing company row
            let peer_id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "UPDATE company SET peer_id = ?1 WHERE peer_id = ''",
                params![peer_id],
            )
            .await?;
            debug!("Added peer_id column to company table");
        }

        Ok(())
    }

    async fn add_company_logo_columns(&self) -> Result<(), SyncError> {
        let conn = self.connect().await?;
        // Try to add columns; ignore errors if table doesn't exist or column already exists
        let _ = conn
            .execute_batch("ALTER TABLE company ADD COLUMN logo TEXT")
            .await;
        let _ = conn
            .execute_batch("ALTER TABLE company ADD COLUMN logo_updated_at TEXT")
            .await;
        Ok(())
    }

    async fn add_registry_logo_column(&self) -> Result<(), SyncError> {
        let conn = self.connect().await?;
        // Try to add column; ignore errors if table doesn't exist or column already exists
        let _ = conn
            .execute_batch("ALTER TABLE company_registry ADD COLUMN logo TEXT")
            .await;
        Ok(())
    }

    // ── Company Registry (App DB) ─────────────────────────────

    pub async fn list_registered_companies(&self) -> Result<Vec<CompanyRegistryEntry>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, name, role, db_path, created_at, updated_at, logo FROM company_registry ORDER BY created_at",
                (),
            )
            .await?;

        let mut entries = Vec::new();
        while let Some(row) = rows.next().await? {
            entries.push(CompanyRegistryEntry {
                id: row.get::<String>(0)?,
                name: row.get::<String>(1)?,
                role: row.get::<String>(2)?,
                db_path: row.get::<String>(3)?,
                created_at: row.get::<String>(4)?,
                updated_at: row.get::<String>(5)?,
                logo: row.get::<Option<String>>(6)?,
            });
        }
        Ok(entries)
    }

    pub async fn register_company(
        &self,
        id: &str,
        name: &str,
        role: &str,
        secret_hash: &str,
        db_path: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO company_registry (id, name, role, secret_hash, db_path)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name, role, secret_hash, db_path],
        )
        .await?;
        Ok(())
    }

    pub async fn unregister_company(&self, id: &str) -> Result<(), SyncError> {
        let conn = self.connect().await?;
        conn.execute("DELETE FROM company_registry WHERE id = ?1", params![id])
            .await?;
        Ok(())
    }

    pub async fn update_registry_entry(
        &self,
        id: &str,
        name: &str,
        role: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE company_registry SET name = ?1, role = ?2, updated_at = datetime('now') WHERE id = ?3",
            params![name, role, id],
        )
        .await?;
        Ok(())
    }

    // ── App Settings (App DB) ─────────────────────────────────

    pub async fn get_app_setting(&self, key: &str) -> Result<Option<String>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT value FROM app_settings WHERE key = ?1",
                params![key],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some(row.get::<String>(0)?))
        } else {
            Ok(None)
        }
    }

    pub async fn set_app_setting(&self, key: &str, value: &str) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )
        .await?;
        Ok(())
    }

    // ── Company Config (Company DB) ───────────────────────────

    pub async fn get_company_config(&self) -> Result<Option<(String, String)>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT config_json, updated_at FROM company_config WHERE id = 1",
                (),
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some((row.get::<String>(0)?, row.get::<String>(1)?)))
        } else {
            Ok(None)
        }
    }

    pub async fn set_company_config(
        &self,
        config_json: &str,
        updated_at: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO company_config (id, config_json, updated_at)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET config_json = ?1, updated_at = ?2",
            params![config_json, updated_at],
        )
        .await?;
        Ok(())
    }

    // ── Peers V2 (with stable peer_id) ────────────────────────

    pub async fn list_peers_v2(
        &self,
    ) -> Result<Vec<(String, String, String, String, bool, String, Option<String>)>, SyncError>
    {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT peer_id, onion_address, name, role, is_authorized, added_at, last_seen_at
                 FROM company_peers_v2 ORDER BY added_at",
                (),
            )
            .await?;

        let mut peers = Vec::new();
        while let Some(row) = rows.next().await? {
            peers.push((
                row.get::<String>(0)?,
                row.get::<String>(1)?,
                row.get::<String>(2)?,
                row.get::<String>(3)?,
                row.get::<bool>(4)?,
                row.get::<String>(5)?,
                row.get::<Option<String>>(6)?,
            ));
        }
        Ok(peers)
    }

    pub async fn get_peer_v2(
        &self,
        peer_id: &str,
    ) -> Result<Option<(String, String, String, String, bool)>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT peer_id, onion_address, name, role, is_authorized FROM company_peers_v2 WHERE peer_id = ?1",
                params![peer_id],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some((
                row.get::<String>(0)?,
                row.get::<String>(1)?,
                row.get::<String>(2)?,
                row.get::<String>(3)?,
                row.get::<bool>(4)?,
            )))
        } else {
            Ok(None)
        }
    }

    pub async fn get_peer_by_onion(
        &self,
        onion_address: &str,
    ) -> Result<Option<(String, String, String, String, bool)>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT peer_id, onion_address, name, role, is_authorized FROM company_peers_v2 WHERE onion_address = ?1",
                params![onion_address],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some((
                row.get::<String>(0)?,
                row.get::<String>(1)?,
                row.get::<String>(2)?,
                row.get::<String>(3)?,
                row.get::<bool>(4)?,
            )))
        } else {
            Ok(None)
        }
    }

    pub async fn insert_peer_v2(
        &self,
        peer_id: &str,
        onion_address: &str,
        name: &str,
        role: &str,
        is_authorized: bool,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT OR IGNORE INTO company_peers_v2 (peer_id, onion_address, name, role, is_authorized)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![peer_id, onion_address, name, role, is_authorized],
        )
        .await?;
        Ok(())
    }

    pub async fn remove_peer_v2(&self, peer_id: &str) -> Result<(), SyncError> {
        // Read the onion address for sync state cleanup
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT onion_address FROM company_peers_v2 WHERE peer_id = ?1",
                params![peer_id],
            )
            .await?;
        let addr: Option<String> = if let Some(row) = rows.next().await? {
            Some(row.get(0)?)
        } else {
            None
        };
        drop(rows);
        drop(conn);

        // Write: delete peer and optionally its sync state
        if let Some(addr) = addr {
            self.execute_writes(&[
                (
                    "DELETE FROM p2p_sync_state WHERE peer_address = ?1",
                    to_params(params![addr]),
                ),
                (
                    "DELETE FROM company_peers_v2 WHERE peer_id = ?1",
                    to_params(params![peer_id]),
                ),
            ])
            .await?;
        } else {
            self.execute_write(
                "DELETE FROM company_peers_v2 WHERE peer_id = ?1",
                params![peer_id],
            )
            .await?;
        }
        Ok(())
    }

    pub async fn update_peer_onion_address(
        &self,
        peer_id: &str,
        new_onion: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE company_peers_v2 SET onion_address = ?1 WHERE peer_id = ?2",
            params![new_onion, peer_id],
        )
        .await?;
        Ok(())
    }

    pub async fn update_peer_last_seen_v2(&self, peer_id: &str) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE company_peers_v2 SET last_seen_at = datetime('now') WHERE peer_id = ?1",
            params![peer_id],
        )
        .await?;
        Ok(())
    }

    pub async fn approve_peer_v2(&self, peer_id: &str) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE company_peers_v2 SET is_authorized = 1 WHERE peer_id = ?1",
            params![peer_id],
        )
        .await?;
        Ok(())
    }

    pub async fn reject_peer_v2(&self, peer_id: &str) -> Result<(), SyncError> {
        self.remove_peer_v2(peer_id).await
    }

    pub async fn update_peer_role_v2(&self, peer_id: &str, role: &str) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE company_peers_v2 SET role = ?1 WHERE peer_id = ?2",
            params![role, peer_id],
        )
        .await?;
        Ok(())
    }

    /// Get the company's own peer_id
    pub async fn get_company_peer_id(&self) -> Result<Option<String>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query("SELECT peer_id FROM company LIMIT 1", ())
            .await?;
        if let Some(row) = rows.next().await? {
            let peer_id: String = row.get(0)?;
            if peer_id.is_empty() {
                Ok(None)
            } else {
                Ok(Some(peer_id))
            }
        } else {
            Ok(None)
        }
    }

    /// Set the company's own peer_id
    pub async fn set_company_peer_id(&self, peer_id: &str) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE company SET peer_id = ?1, updated_at = datetime('now')",
            params![peer_id],
        )
        .await?;
        Ok(())
    }

    // ── Company Logo ──────────────────────────────────────────

    pub async fn get_company_logo(&self) -> Result<Option<(String, String)>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query("SELECT logo, logo_updated_at FROM company LIMIT 1", ())
            .await?;
        if let Some(row) = rows.next().await? {
            let logo: Option<String> = row.get(0)?;
            let updated_at: Option<String> = row.get(1)?;
            match (logo, updated_at) {
                (Some(l), Some(u)) if !l.is_empty() => Ok(Some((l, u))),
                _ => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    pub async fn set_company_logo(&self, logo: &str, updated_at: &str) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE company SET logo = ?1, logo_updated_at = ?2, updated_at = datetime('now')",
            params![logo, updated_at],
        )
        .await?;
        Ok(())
    }

    pub async fn update_registry_logo(&self, id: &str, logo: &str) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE company_registry SET logo = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![logo, id],
        )
        .await?;
        Ok(())
    }

    // ── Vendor Plugin Registry ────────────────────────────────

    pub async fn list_registry_plugins(&self) -> Result<Vec<VendorPluginRow>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, plugin_file, display_name, description, files_json, version,
                        config_fields, config_json, status, submitted_by, approved_by,
                        category, icon, include_vendor_stock, created_at, updated_at
                 FROM vendor_plugin_registry ORDER BY display_name",
                (),
            )
            .await?;

        let mut plugins = Vec::new();
        while let Some(row) = rows.next().await? {
            plugins.push(parse_vendor_plugin_row(&row)?);
        }
        Ok(plugins)
    }

    pub async fn get_registry_plugin(
        &self,
        id: &str,
    ) -> Result<Option<VendorPluginRow>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, plugin_file, display_name, description, files_json, version,
                        config_fields, config_json, status, submitted_by, approved_by,
                        category, icon, include_vendor_stock, created_at, updated_at
                 FROM vendor_plugin_registry WHERE id = ?1",
                params![id],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            Ok(Some(parse_vendor_plugin_row(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn find_registry_plugin_by_name(
        &self,
        display_name: &str,
    ) -> Result<Option<VendorPluginRow>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, plugin_file, display_name, description, files_json, version,
                        config_fields, config_json, status, submitted_by, approved_by,
                        category, icon, include_vendor_stock, created_at, updated_at
                 FROM vendor_plugin_registry WHERE display_name = ?1 LIMIT 1",
                params![display_name],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            Ok(Some(parse_vendor_plugin_row(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn upsert_registry_plugin(&self, plugin: &VendorPluginRow) -> Result<(), SyncError> {
        let conn = self.connect().await?;
        conn.execute(
            "INSERT INTO vendor_plugin_registry
                (id, plugin_file, display_name, description, code, files_json, version,
                 config_fields, config_json, status, submitted_by, approved_by,
                 category, icon, include_vendor_stock, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, '', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(id) DO UPDATE SET
                plugin_file = ?2, display_name = ?3, description = ?4, files_json = ?5,
                version = ?6, config_fields = ?7, config_json = ?8, status = ?9,
                submitted_by = ?10, approved_by = ?11, category = ?12, icon = ?13,
                include_vendor_stock = ?14, updated_at = ?16",
            params![
                plugin.id.clone(),
                plugin.plugin_file.clone(),
                plugin.display_name.clone(),
                plugin.description.clone(),
                plugin.files_json.clone(),
                plugin.version.clone(),
                plugin.config_fields.clone(),
                plugin.config_json.clone(),
                plugin.status.clone(),
                plugin.submitted_by.clone(),
                plugin.approved_by.clone(),
                plugin.category.clone(),
                plugin.icon.clone(),
                plugin.include_vendor_stock as i64,
                plugin.created_at.clone(),
                plugin.updated_at.clone(),
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn set_plugin_status(
        &self,
        id: &str,
        status: &str,
        approved_by: Option<&str>,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE vendor_plugin_registry SET status = ?1, approved_by = ?2, updated_at = datetime('now') WHERE id = ?3",
            params![status, approved_by, id],
        )
        .await?;
        Ok(())
    }

    pub async fn set_plugin_config(&self, id: &str, config_json: &str) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE vendor_plugin_registry SET config_json = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![config_json, id],
        )
        .await?;
        Ok(())
    }

    pub async fn delete_registry_plugin(&self, id: &str) -> Result<(), SyncError> {
        let p = to_params(params![id]);
        self.execute_writes(&[
            (
                "DELETE FROM vendor_plugin_installs WHERE plugin_id = ?1",
                p.clone(),
            ),
            ("DELETE FROM vendor_plugin_registry WHERE id = ?1", p),
        ])
        .await?;
        Ok(())
    }

    // ── Vendor Plugin Installs (local only) ──────────────────

    pub async fn list_installed_plugins(&self) -> Result<Vec<VendorPluginInstall>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT plugin_id, installed, enabled, installed_at
                 FROM vendor_plugin_installs ORDER BY installed_at",
                (),
            )
            .await?;

        let mut installs = Vec::new();
        while let Some(row) = rows.next().await? {
            installs.push(VendorPluginInstall {
                plugin_id: row.get::<String>(0)?,
                installed: row.get::<bool>(1)?,
                enabled: row.get::<bool>(2)?,
                installed_at: row.get::<String>(3)?,
            });
        }
        Ok(installs)
    }

    pub async fn install_plugin(&self, plugin_id: &str) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO vendor_plugin_installs (plugin_id, installed, enabled)
             VALUES (?1, 1, 1)
             ON CONFLICT(plugin_id) DO UPDATE SET installed = 1, enabled = 1",
            params![plugin_id],
        )
        .await?;
        Ok(())
    }

    pub async fn uninstall_plugin(&self, plugin_id: &str) -> Result<(), SyncError> {
        self.execute_write(
            "DELETE FROM vendor_plugin_installs WHERE plugin_id = ?1",
            params![plugin_id],
        )
        .await?;
        Ok(())
    }

    pub async fn set_plugin_enabled(
        &self,
        plugin_id: &str,
        enabled: bool,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE vendor_plugin_installs SET enabled = ?1 WHERE plugin_id = ?2",
            params![enabled, plugin_id],
        )
        .await?;
        Ok(())
    }

    // ── Vendor Listings Cache ─────────────────────────────────

    /// Replace all cached listings for a plugin (transactional).
    pub async fn save_vendor_listings_cache(
        &self,
        plugin_id: &str,
        listings: &[VendorListingCache],
    ) -> Result<(), SyncError> {
        let conn = self.connect().await?;
        conn.execute("BEGIN IMMEDIATE", ()).await?;

        conn.execute(
            "DELETE FROM vendor_listings_cache WHERE plugin_id = ?1",
            params![plugin_id],
        )
        .await?;

        // Deduplicate by (plugin_id, vendor_item_id) — last occurrence wins.
        // Some vendor plugins may return duplicate SKUs across product lines,
        // which would violate the UNIQUE constraint and fail the entire transaction.
        let mut seen = std::collections::HashSet::new();
        let mut deduped: Vec<&VendorListingCache> = Vec::with_capacity(listings.len());
        for l in listings.iter().rev() {
            if seen.insert(&l.vendor_item_id) {
                deduped.push(l);
            }
        }
        deduped.reverse();

        for l in deduped {
            let extras_json = serde_json::to_string(&l.extras)
                .map_err(|e| SyncError::DatabaseError(e.to_string()))?;
            let variant_attrs_json = serde_json::to_string(&l.variant_attributes)
                .map_err(|e| SyncError::DatabaseError(e.to_string()))?;
            conn.execute(
                "INSERT OR REPLACE INTO vendor_listings_cache
                 (plugin_id, vendor_item_id, title, price, currency, quantity, sku, image_url, url, extras_json, group_key, variant_attrs_json, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    plugin_id,
                    l.vendor_item_id.clone(),
                    l.title.clone(),
                    l.price,
                    l.currency.clone(),
                    l.quantity,
                    l.sku.clone(),
                    l.image_url.clone(),
                    l.url.clone(),
                    extras_json,
                    l.group_key.clone(),
                    variant_attrs_json,
                    l.fetched_at.clone(),
                ],
            )
            .await?;
        }

        conn.execute("COMMIT", ()).await?;
        Ok(())
    }

    /// Load all cached listings for a plugin.
    pub async fn get_vendor_listings_cache(
        &self,
        plugin_id: &str,
    ) -> Result<Vec<VendorListingCache>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT plugin_id, vendor_item_id, title, price, currency, quantity, sku,
                        image_url, url, extras_json, group_key, variant_attrs_json, fetched_at
                 FROM vendor_listings_cache WHERE plugin_id = ?1
                 ORDER BY title",
                params![plugin_id],
            )
            .await?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            let extras_json: String = row.get::<String>(9)?;
            let variant_attrs_json: String = row.get::<String>(11)?;
            out.push(VendorListingCache {
                plugin_id: row.get::<String>(0)?,
                vendor_item_id: row.get::<String>(1)?,
                title: row.get::<String>(2)?,
                price: row.get::<Option<f64>>(3)?,
                currency: row.get::<Option<String>>(4)?,
                quantity: row.get::<Option<i64>>(5)?,
                sku: row.get::<Option<String>>(6)?,
                image_url: row.get::<Option<String>>(7)?,
                url: row.get::<Option<String>>(8)?,
                extras: serde_json::from_str(&extras_json).unwrap_or_default(),
                group_key: row.get::<Option<String>>(10)?,
                variant_attributes: serde_json::from_str(&variant_attrs_json).unwrap_or_default(),
                fetched_at: row.get::<String>(12)?,
            });
        }
        Ok(out)
    }

    /// Get cache metadata for a plugin: (last_fetched_at, count).
    pub async fn get_vendor_cache_meta(
        &self,
        plugin_id: &str,
    ) -> Result<Option<(String, i64)>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT MAX(fetched_at), COUNT(*) FROM vendor_listings_cache WHERE plugin_id = ?1",
                params![plugin_id],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            let fetched_at: Option<String> = row.get::<Option<String>>(0)?;
            let count: i64 = row.get::<i64>(1)?;
            if let Some(ts) = fetched_at {
                return Ok(Some((ts, count)));
            }
        }
        Ok(None)
    }

    /// Delete all cached listings for a plugin.
    pub async fn clear_vendor_listings_cache(&self, plugin_id: &str) -> Result<(), SyncError> {
        self.execute_write(
            "DELETE FROM vendor_listings_cache WHERE plugin_id = ?1",
            params![plugin_id],
        )
        .await?;
        Ok(())
    }

    // ── Product Vendor Data (permanent per-product) ─────────

    /// Save vendor listing data permanently for a product variant.
    pub async fn save_product_vendor_data(
        &self,
        product_id: &str,
        variant_id: &str,
        plugin_id: &str,
        listing: &VendorListingCache,
    ) -> Result<(), SyncError> {
        let extras_json = serde_json::to_string(&listing.extras)
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;
        let variant_attrs_json = serde_json::to_string(&listing.variant_attributes)
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;
        self.execute_write(
            "INSERT OR REPLACE INTO product_vendor_data
             (product_id, variant_id, plugin_id, vendor_item_id, title, price, currency, quantity, sku, image_url, url, extras_json, group_key, variant_attrs_json, fetched_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, datetime('now'))",
            params![
                product_id,
                variant_id,
                plugin_id,
                listing.vendor_item_id.clone(),
                listing.title.clone(),
                listing.price,
                listing.currency.clone(),
                listing.quantity,
                listing.sku.clone(),
                listing.image_url.clone(),
                listing.url.clone(),
                extras_json,
                listing.group_key.clone(),
                variant_attrs_json,
                listing.fetched_at.clone(),
            ],
        )
        .await?;
        Ok(())
    }

    /// Read permanent vendor data for a product. Returns (plugin_display_name, variant_id, listing_data) tuples.
    pub async fn get_product_vendor_data(
        &self,
        product_id: &str,
    ) -> Result<Vec<(String, Option<String>, VendorListingCache)>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT COALESCE(vpr.display_name, 'Vendor'),
                        pvd.plugin_id, pvd.vendor_item_id, pvd.title, pvd.price, pvd.currency,
                        pvd.quantity, pvd.sku, pvd.image_url, pvd.url, pvd.extras_json,
                        pvd.group_key, pvd.variant_attrs_json, pvd.fetched_at, pvd.variant_id
                 FROM product_vendor_data pvd
                 LEFT JOIN vendor_plugin_registry vpr ON pvd.plugin_id = vpr.id
                 WHERE pvd.product_id = ?1
                 ORDER BY pvd.title",
                params![product_id],
            )
            .await?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            let extras_json: String = row.get::<String>(10)?;
            let variant_attrs_json: String = row.get::<String>(12)?;
            out.push((
                row.get::<String>(0)?,
                row.get::<Option<String>>(14)?,
                VendorListingCache {
                    plugin_id: row.get::<String>(1)?,
                    vendor_item_id: row.get::<String>(2)?,
                    title: row.get::<String>(3)?,
                    price: row.get::<Option<f64>>(4)?,
                    currency: row.get::<Option<String>>(5)?,
                    quantity: row.get::<Option<i64>>(6)?,
                    sku: row.get::<Option<String>>(7)?,
                    image_url: row.get::<Option<String>>(8)?,
                    url: row.get::<Option<String>>(9)?,
                    extras: serde_json::from_str(&extras_json).unwrap_or_default(),
                    group_key: row.get::<Option<String>>(11)?,
                    variant_attributes: serde_json::from_str(&variant_attrs_json)
                        .unwrap_or_default(),
                    fetched_at: row.get::<String>(13)?,
                },
            ));
        }
        Ok(out)
    }

    /// After a vendor sync, update any product_vendor_data rows that match
    /// the freshly-synced cache entries (by plugin_id + vendor_item_id).
    pub async fn update_product_vendor_data_from_cache(
        &self,
        plugin_id: &str,
        listings: &[VendorListingCache],
    ) -> Result<usize, SyncError> {
        if listings.is_empty() {
            return Ok(0);
        }

        let mut stmts: Vec<(&str, Params)> = Vec::with_capacity(listings.len());
        for l in listings {
            let extras_json = serde_json::to_string(&l.extras)
                .map_err(|e| SyncError::DatabaseError(e.to_string()))?;
            let variant_attrs_json = serde_json::to_string(&l.variant_attributes)
                .map_err(|e| SyncError::DatabaseError(e.to_string()))?;
            stmts.push((
                "UPDATE product_vendor_data SET
                    title = ?1, price = ?2, currency = ?3, quantity = ?4, sku = ?5,
                    image_url = ?6, url = ?7, extras_json = ?8, group_key = ?9,
                    variant_attrs_json = ?10, fetched_at = ?11, updated_at = datetime('now')
                 WHERE plugin_id = ?12 AND vendor_item_id = ?13",
                to_params(params![
                    l.title.clone(),
                    l.price,
                    l.currency.clone(),
                    l.quantity,
                    l.sku.clone(),
                    l.image_url.clone(),
                    l.url.clone(),
                    extras_json,
                    l.group_key.clone(),
                    variant_attrs_json,
                    l.fetched_at.clone(),
                    plugin_id,
                    l.vendor_item_id.clone(),
                ]),
            ));
        }
        self.execute_writes(&stmts).await?;
        Ok(listings.len())
    }

    /// Find vendor listings for a product.
    /// Primary source: product_vendor_data table (permanent).
    /// Fallback for legacy products: construct from variant data.
    pub async fn get_vendor_listings_for_product(
        &self,
        product_id: &str,
    ) -> Result<Vec<(String, Option<String>, VendorListingCache)>, SyncError> {
        // Step 1: Read from product_vendor_data (the permanent store)
        let out = self.get_product_vendor_data(product_id).await?;
        if !out.is_empty() {
            tracing::info!(
                product_id,
                matched = out.len(),
                "Vendor listings from product_vendor_data"
            );
            return Ok(out);
        }

        // Step 2: Fallback for legacy products — construct from variant data
        let conn = self.connect().await?;
        let mut vrows = conn
            .query(
                "SELECT sku, name, quantity, image_url, attributes_json,
                        source_plugin_id, source_vendor_item_id, id
                 FROM product_variants WHERE product_id = ?1",
                params![product_id],
            )
            .await?;

        struct VariantInfo {
            id: String,
            sku: String,
            name: String,
            quantity: i64,
            image_url: Option<String>,
            attributes: std::collections::HashMap<String, String>,
            source_plugin_id: Option<String>,
            source_vendor_item_id: Option<String>,
        }

        let mut variants = Vec::new();
        let mut source_ids: Vec<(String, String)> = Vec::new();

        while let Some(row) = vrows.next().await? {
            let source_plugin_id: Option<String> = row.get::<Option<String>>(5)?;
            let source_vendor_item_id: Option<String> = row.get::<Option<String>>(6)?;
            let attrs_json: String = row
                .get::<Option<String>>(4)?
                .unwrap_or_else(|| "{}".to_string());

            if let (Some(ref pid), Some(ref vid)) = (&source_plugin_id, &source_vendor_item_id) {
                source_ids.push((pid.clone(), vid.clone()));
            }

            variants.push(VariantInfo {
                id: row.get::<String>(7)?,
                sku: row.get::<String>(0)?,
                name: row.get::<String>(1)?,
                quantity: row.get::<i64>(2)?,
                image_url: row.get::<Option<String>>(3)?,
                attributes: serde_json::from_str(&attrs_json).unwrap_or_default(),
                source_plugin_id,
                source_vendor_item_id,
            });
        }

        if variants.is_empty() {
            return Ok(Vec::new());
        }

        // If no variants have vendor source info, this product wasn't imported from a vendor.
        // Don't try to construct fake vendor listings from manually-created variants.
        if source_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Determine plugin name
        let plugin_name = if !source_ids.is_empty() {
            let plugin_id = &source_ids[0].0;
            let mut prows = conn
                .query(
                    "SELECT display_name FROM vendor_plugin_registry WHERE id = ?1",
                    params![plugin_id.clone()],
                )
                .await?;
            if let Some(row) = prows.next().await? {
                row.get::<String>(0)?
            } else {
                "Vendor".to_string()
            }
        } else {
            let mut prows = conn
                .query(
                    "SELECT display_name FROM vendor_plugin_registry ORDER BY updated_at DESC LIMIT 1",
                    (),
                )
                .await?;
            if let Some(row) = prows.next().await? {
                row.get::<String>(0)?
            } else {
                "Vendor".to_string()
            }
        };

        let mut out = Vec::new();
        for v in &variants {
            let vid = v
                .source_vendor_item_id
                .clone()
                .unwrap_or_else(|| v.name.clone());
            let pid = v.source_plugin_id.clone().unwrap_or_default();
            out.push((
                plugin_name.clone(),
                Some(v.id.clone()),
                VendorListingCache {
                    plugin_id: pid,
                    vendor_item_id: vid,
                    title: v.name.clone(),
                    price: None,
                    currency: None,
                    quantity: Some(v.quantity),
                    sku: Some(v.sku.clone()),
                    image_url: v.image_url.clone(),
                    url: None,
                    extras: std::collections::HashMap::new(),
                    group_key: None,
                    variant_attributes: v.attributes.clone(),
                    fetched_at: String::new(),
                },
            ));
        }

        tracing::info!(
            product_id,
            matched = out.len(),
            "Vendor listings (legacy fallback)"
        );
        Ok(out)
    }

    // ── Export / Import ──────────────────────────────────────

    pub async fn import_data(&self, data: &ExportData) -> Result<(), SyncError> {
        let conn = self.connect().await?;

        conn.execute("BEGIN IMMEDIATE", ()).await?;

        for product in &data.products {
            conn.execute(
                "INSERT OR REPLACE INTO products (id, canonical_sku, name, quantity, low_stock_threshold, is_tracked, has_variants, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    product.id.clone(),
                    product.canonical_sku.clone(),
                    product.name.clone(),
                    product.quantity,
                    product.low_stock_threshold,
                    product.is_tracked,
                    product.has_variants,
                    product.created_at.clone(),
                    product.updated_at.clone(),
                ],
            )
            .await?;
        }

        for variant in &data.product_variants {
            let attrs_json = serde_json::to_string(&variant.attributes)
                .map_err(|e| SyncError::DatabaseError(e.to_string()))?;
            conn.execute(
                "INSERT OR REPLACE INTO product_variants (id, product_id, sku, name, attributes_json, quantity, image_url, sort_order, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    variant.id.clone(),
                    variant.product_id.clone(),
                    variant.sku.clone(),
                    variant.name.clone(),
                    attrs_json,
                    variant.quantity,
                    variant.image_url.clone(),
                    variant.sort_order as i64,
                    variant.created_at.clone(),
                    variant.updated_at.clone(),
                ],
            )
            .await?;
        }

        for mapping in &data.platform_mappings {
            conn.execute(
                "INSERT OR IGNORE INTO platform_mappings (product_id, platform, platform_item_id, platform_sku, is_active, variant_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    mapping.product_id.clone(),
                    mapping.platform.as_str(),
                    mapping.platform_item_id.clone(),
                    mapping.platform_sku.clone(),
                    mapping.is_active,
                    mapping.variant_id.clone(),
                ],
            )
            .await?;
        }

        for snapshot in &data.platform_snapshots {
            conn.execute(
                "INSERT OR REPLACE INTO platform_snapshots (product_id, platform, last_known_quantity, last_polled_at, last_pushed_at, version_tag)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    snapshot.product_id.clone(),
                    snapshot.platform.as_str(),
                    snapshot.last_known_quantity,
                    snapshot.last_polled_at.clone(),
                    snapshot.last_pushed_at.clone(),
                    snapshot.version_tag.clone(),
                ],
            )
            .await?;
        }

        for order in &data.xmr_processed_orders {
            conn.execute(
                "INSERT OR IGNORE INTO xmr_processed_orders (order_id, product_id, listing_id, quantity, processed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    order.order_id.clone(),
                    order.product_id.clone(),
                    order.listing_id.clone(),
                    order.quantity,
                    order.processed_at.clone(),
                ],
            )
            .await?;
        }

        // Fixup: ensure every product has at least one variant (always-variants model)
        {
            let mut orphan_rows = conn
                .query(
                    "SELECT id, canonical_sku, quantity FROM products
                     WHERE id NOT IN (SELECT DISTINCT product_id FROM product_variants)",
                    (),
                )
                .await?;
            let mut orphans: Vec<(String, String, i64)> = Vec::new();
            while let Some(row) = orphan_rows.next().await? {
                orphans.push((
                    row.get::<String>(0)?,
                    row.get::<String>(1)?,
                    row.get::<i64>(2)?,
                ));
            }
            for (pid, sku, qty) in &orphans {
                let vid = uuid::Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO product_variants (id, product_id, sku, name, attributes_json, quantity, sort_order)
                     VALUES (?1, ?2, ?3, 'Default', '{}', ?4, 0)",
                    params![vid, pid.clone(), sku.clone(), *qty],
                )
                .await?;
            }
            conn.execute(
                "UPDATE products SET has_variants = 1, updated_at = datetime('now') WHERE has_variants = 0",
                (),
            )
            .await?;
        }

        conn.execute("COMMIT", ()).await?;

        Ok(())
    }
}

fn parse_vendor_plugin_row(row: &turso::Row) -> Result<VendorPluginRow, SyncError> {
    Ok(VendorPluginRow {
        id: row.get::<String>(0)?,
        plugin_file: row.get::<String>(1)?,
        display_name: row.get::<String>(2)?,
        description: row.get::<String>(3)?,
        files_json: row.get::<Option<String>>(4)?.unwrap_or_default(),
        version: row.get::<String>(5)?,
        config_fields: row.get::<Option<String>>(6)?,
        config_json: row.get::<Option<String>>(7)?,
        status: row.get::<String>(8)?,
        submitted_by: row.get::<Option<String>>(9)?,
        approved_by: row.get::<Option<String>>(10)?,
        category: row
            .get::<Option<String>>(11)?
            .unwrap_or_else(|| "vendor".into()),
        icon: row.get::<Option<String>>(12)?,
        include_vendor_stock: row.get::<Option<i64>>(13)?.unwrap_or(0) != 0,
        created_at: row.get::<String>(14)?,
        updated_at: row.get::<String>(15)?,
    })
}

fn parse_product(row: &turso::Row) -> Result<Product, SyncError> {
    Ok(Product {
        id: row.get::<String>(0)?,
        canonical_sku: row.get::<String>(1)?,
        name: row.get::<String>(2)?,
        quantity: row.get::<i64>(3)?,
        low_stock_threshold: row.get::<Option<i64>>(4)?,
        is_tracked: row.get::<bool>(5)?,
        created_at: row.get::<String>(6)?,
        updated_at: row.get::<String>(7)?,
        has_variants: row.get::<Option<bool>>(8)?.unwrap_or(false),
    })
}

fn parse_mapping(row: &turso::Row) -> Result<PlatformMapping, SyncError> {
    Ok(PlatformMapping {
        id: row.get::<i64>(0)?,
        product_id: row.get::<String>(1)?,
        platform: Platform::from_str_loose(&row.get::<String>(2)?).unwrap_or(Platform::Ebay),
        platform_item_id: row.get::<String>(3)?,
        platform_sku: row.get::<Option<String>>(4)?,
        is_active: row.get::<bool>(5)?,
        variant_id: row.get::<Option<String>>(6)?,
    })
}

fn parse_variant(row: &turso::Row) -> Result<ProductVariant, SyncError> {
    let attrs_json: String = row
        .get::<Option<String>>(4)?
        .unwrap_or_else(|| "{}".to_string());
    let attributes: std::collections::HashMap<String, String> =
        serde_json::from_str(&attrs_json).unwrap_or_default();
    Ok(ProductVariant {
        id: row.get::<String>(0)?,
        product_id: row.get::<String>(1)?,
        sku: row.get::<String>(2)?,
        name: row.get::<String>(3)?,
        attributes,
        quantity: row.get::<i64>(5)?,
        on_hand_quantity: row.get::<i64>(6)?,
        image_url: row.get::<Option<String>>(7)?,
        sort_order: row.get::<i64>(8)? as i32,
        source_plugin_id: row.get::<Option<String>>(9)?,
        source_vendor_item_id: row.get::<Option<String>>(10)?,
        created_at: row.get::<String>(11)?,
        updated_at: row.get::<String>(12)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_db_open_and_migrate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();

        // Verify the schema version
        let conn = db.connect().await.unwrap();
        let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
        let version: i64 = rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn test_product_crud() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();

        let product = Product {
            id: "test-001".to_string(),
            canonical_sku: "SKU-001".to_string(),
            name: "Test Widget".to_string(),
            quantity: 10,
            low_stock_threshold: Some(3),
            is_tracked: true,
            has_variants: false,
            created_at: String::new(),
            updated_at: String::new(),
        };

        db.insert_product(&product).await.unwrap();

        let fetched = db.get_product("test-001").await.unwrap().unwrap();
        assert_eq!(fetched.name, "Test Widget");
        assert_eq!(fetched.quantity, 10);

        db.update_product_quantity("test-001", 7).await.unwrap();
        let fetched = db.get_product("test-001").await.unwrap().unwrap();
        assert_eq!(fetched.quantity, 7);

        let products = db.list_products().await.unwrap();
        assert_eq!(products.len(), 1);

        db.delete_product("test-001").await.unwrap();
        let fetched = db.get_product("test-001").await.unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_mapping_crud() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();

        let product = Product {
            id: "p1".to_string(),
            canonical_sku: "SKU-P1".to_string(),
            name: "Product 1".to_string(),
            quantity: 5,
            low_stock_threshold: None,
            is_tracked: true,
            has_variants: false,
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.insert_product(&product).await.unwrap();

        db.insert_mapping("p1", "ebay", "EBAY-123", Some("SKU-P1"), "v1")
            .await
            .unwrap();
        db.insert_mapping("p1", "squarespace", "SQ-456", None, "v1")
            .await
            .unwrap();

        let mappings = db.list_mappings_for_product("p1").await.unwrap();
        assert_eq!(mappings.len(), 2);

        let active = db.list_all_active_mappings().await.unwrap();
        assert_eq!(active.len(), 2);

        let found = db
            .get_mapping_by_platform_item("ebay", "EBAY-123")
            .await
            .unwrap();
        assert!(found.is_some());

        db.delete_mapping(mappings[0].id).await.unwrap();
        let mappings = db.list_mappings_for_product("p1").await.unwrap();
        assert_eq!(mappings.len(), 1);
    }

    #[tokio::test]
    async fn test_snapshot_operations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();

        let product = Product {
            id: "p1".to_string(),
            canonical_sku: "SKU-P1".to_string(),
            name: "Product 1".to_string(),
            quantity: 10,
            low_stock_threshold: None,
            is_tracked: true,
            has_variants: false,
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.insert_product(&product).await.unwrap();

        db.upsert_snapshot("p1", "ebay", 10, "2025-01-01 00:00:00")
            .await
            .unwrap();

        let snap = db.get_snapshot("p1", "ebay").await.unwrap().unwrap();
        assert_eq!(snap.last_known_quantity, 10);

        // Update via upsert
        db.upsert_snapshot("p1", "ebay", 8, "2025-01-01 00:05:00")
            .await
            .unwrap();
        let snap = db.get_snapshot("p1", "ebay").await.unwrap().unwrap();
        assert_eq!(snap.last_known_quantity, 8);
    }

    #[tokio::test]
    async fn test_inventory_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();

        let product = Product {
            id: "p1".to_string(),
            canonical_sku: "SKU-P1".to_string(),
            name: "Product 1".to_string(),
            quantity: 10,
            low_stock_threshold: None,
            is_tracked: true,
            has_variants: false,
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.insert_product(&product).await.unwrap();

        db.record_inventory_history("p1", 10, "cycle-1")
            .await
            .unwrap();
        db.record_inventory_history("p1", 8, "cycle-2")
            .await
            .unwrap();
        db.record_inventory_history("p1", 5, "cycle-3")
            .await
            .unwrap();

        let history = db.get_inventory_history("p1", None).await.unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].quantity, 10);
        assert_eq!(history[2].quantity, 5);
    }
}
