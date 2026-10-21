use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};
use turso::params::Params;
use turso::{params, Builder, Connection, Database, Error as TursoError};

use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;

use crate::config::CompanyRegistryEntry;
use crate::crypto::PluginSecretStore;
use crate::error::SyncError;
use crate::types::{
    AuthToken, Dimensions, ExportData, InventoryHistoryEntry, ListingDescription, ListingPhoto,
    NewSyncEvent, Platform, PlatformMapping, PlatformSnapshot, PricingSnapshot, Product,
    ProductVariant, StockEvent, StockEventKind, SyncEvent, VendorListingCache, VendorPluginInstall,
    VendorPluginRow, Weight, XmrProcessedOrder,
};

const CURRENT_SCHEMA_VERSION: i64 = 27;

/// Convert any IntoParams value into a cloneable Params.
/// Panics on conversion failure (should not happen with valid params).
fn to_params(p: impl turso::params::IntoParams) -> Params {
    p.into_params().expect("param conversion should not fail")
}

/// Parse an exact-decimal money column (stored as TEXT, N-6.1).
fn parse_decimal(col: &str, s: &str) -> Result<Decimal, SyncError> {
    s.parse::<Decimal>()
        .map_err(|e| SyncError::DatabaseError(format!("invalid decimal in {col}: {e}")))
}

/// Parse an optional exact-decimal money column (stored as TEXT, N-6.1).
fn parse_decimal_opt(col: &str, s: Option<String>) -> Result<Option<Decimal>, SyncError> {
    s.map(|v| parse_decimal(col, &v)).transpose()
}

/// Bind a money amount for storage: the exact decimal text (N-6.1).
fn decimal_param(d: &Decimal) -> String {
    d.to_string()
}

/// Bind an optional money amount for storage (N-6.1).
fn decimal_param_opt(d: Option<&Decimal>) -> Option<String> {
    d.map(|v| v.to_string())
}

/// Bind an `images_json`/`weight_json`/`dimensions_json` column (N-7.2).
fn json_param<T: serde::Serialize>(v: &T) -> Option<String> {
    serde_json::to_string(v).ok()
}

/// Bind an optional `weight_json`/`dimensions_json` column (N-7.2).
fn json_param_opt<T: serde::Serialize>(v: Option<&T>) -> Option<String> {
    v.and_then(|x| serde_json::to_string(x).ok())
}

/// Parse an `images_json`/`weight_json`/`dimensions_json` column (N-7.2).
/// Corrupt or missing JSON degrades to the default, never an error: a bad
/// image list must not fail a whole catalog read.
fn parse_json_col<T>(s: Option<String>) -> T
where
    T: Default + for<'de> serde::Deserialize<'de>,
{
    s.as_deref()
        .and_then(|v| serde_json::from_str(v).ok())
        .unwrap_or_default()
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

/// Suffixes of the files turso keeps beside a database: the logical log, the
/// write-ahead log and the WAL index.
const JOURNAL_SUFFIXES: &[&str] = &["-log", "-wal", "-shm"];

/// Open a turso database, recovering once from a panic during open.
///
/// turso can panic opening a file whose journal files don't match it (e.g.
/// incompatible automatic indexes from an older schema). The WAL can hold
/// committed transactions that were never checkpointed into the main file, so
/// the journal files are moved into a timestamped backup directory next to the
/// database — never deleted — before a single retry. If the retry fails too,
/// that failure is returned.
async fn open_database<F, Fut>(path: &str, build: F) -> Result<Database, SyncError>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Database, TursoError>> + Send + 'static,
{
    if let Some(db) = build_catching_panic(build(path.to_string())).await? {
        return Ok(db);
    }

    let backup = move_journal_files_aside(Path::new(path)).map_err(|e| {
        SyncError::DatabaseError(format!(
            "turso panicked opening {path} and its journal files could not be moved aside: {e}"
        ))
    })?;
    let moved_to = match &backup {
        Some(dir) => {
            tracing::error!(
                backup = %dir.display(),
                "turso panicked opening {path}; moved its journal files to {} and retrying. \
                 Writes still in the WAL are not in the reopened database — keep that \
                 directory until they have been recovered",
                dir.display()
            );
            format!("after moving its journal files to {}", dir.display())
        }
        None => {
            tracing::error!("turso panicked opening {path}, which has no journal files; retrying");
            "with no journal files present".to_string()
        }
    };

    build_catching_panic(build(path.to_string()))
        .await?
        .ok_or_else(|| {
            SyncError::DatabaseError(format!("turso panicked opening {path} again {moved_to}"))
        })
}

/// Run a database build on its own task, so a panic inside turso comes back as
/// `Ok(None)` instead of unwinding through the caller.
async fn build_catching_panic<Fut>(build: Fut) -> Result<Option<Database>, SyncError>
where
    Fut: std::future::Future<Output = Result<Database, TursoError>> + Send + 'static,
{
    match tokio::task::spawn(build).await {
        Ok(result) => Ok(Some(result?)),
        Err(join_err) if join_err.is_panic() => Ok(None),
        Err(join_err) => Err(SyncError::DatabaseError(format!(
            "Database open task cancelled: {join_err}"
        ))),
    }
}

/// Move a database's journal files into a new `<name>.journal-backup-<UTC
/// timestamp>` directory beside it, leaving the main file where it is.
///
/// Returns the backup directory, or `None` when there were no journal files.
fn move_journal_files_aside(db_path: &Path) -> std::io::Result<Option<PathBuf>> {
    let journals: Vec<PathBuf> = JOURNAL_SUFFIXES
        .iter()
        .map(|suffix| {
            let mut name = db_path.as_os_str().to_owned();
            name.push(suffix);
            PathBuf::from(name)
        })
        .filter(|journal| journal.exists())
        .collect();
    if journals.is_empty() {
        return Ok(None);
    }

    let parent = match db_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let name = db_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "database".to_string());
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");

    let mut backup = parent.join(format!("{name}.journal-backup-{stamp}"));
    let mut attempt = 1;
    while let Err(e) = std::fs::create_dir(&backup) {
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(e);
        }
        backup = parent.join(format!("{name}.journal-backup-{stamp}-{attempt}"));
        attempt += 1;
    }

    for journal in journals {
        // The backup sits in the same directory, so this is a rename on one
        // filesystem and the bytes are never copied or dropped.
        let file_name = journal.file_name().unwrap_or_default();
        std::fs::rename(&journal, backup.join(file_name))?;
    }
    Ok(Some(backup))
}

/// Whether two paths name the same database file.
///
/// Both paths are canonicalized, so `./nisaba.db` and its absolute form (or a
/// symlink to it) compare equal. A path that can't be resolved — usually one
/// that doesn't exist — is compared as written.
pub fn is_same_db_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Delete a company's database file after the company has left the registry.
///
/// Companies migrated from the single-company layout are registered with the
/// app database's own path, because their data lives in it. Deleting that file
/// would take the company registry and every other company's registration with
/// it, so when `company_db` is the app database nothing is deleted and this
/// returns `Ok(false)`. A file that is already gone counts as deleted.
pub fn remove_company_db_file(company_db: &Path, app_db: &Path) -> std::io::Result<bool> {
    if is_same_db_file(company_db, app_db) {
        return Ok(false);
    }
    match std::fs::remove_file(company_db) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e),
    }
}

pub struct Db {
    db: Database,
    path: String,
}

/// An XMR Bazaar order consumed by a sync cycle: the sale it carried has been
/// applied to canonical stock, so the order may be marked processed.
#[derive(Debug, Clone)]
pub struct ConsumedXmrOrder {
    pub order_id: String,
    pub product_id: Option<String>,
    pub listing_id: String,
    pub quantity: i64,
}

impl Db {
    /// Open (or create) a local database at the given path.
    ///
    /// If turso panics during open, the journal files are moved to a backup
    /// directory beside the database and the open is retried once (see
    /// `open_database`). The main database file is never touched. The caller
    /// must run `migrate()` afterwards.
    pub async fn open(path: &str) -> Result<Self, SyncError> {
        let db = open_database(
            path,
            |p| async move { Builder::new_local(&p).build().await },
        )
        .await?;

        let me = Self {
            db,
            path: path.to_string(),
        };
        me.apply_db_pragmas().await?;
        Ok(me)
    }

    /// The path this database was opened with, as given to [`Db::open`].
    pub fn path(&self) -> &str {
        &self.path
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
    /// Returns the statement's own affected-row count. The transaction is
    /// committed only when the statement succeeded; a failed statement is
    /// rolled back, never committed.
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

            // Run the statement first and only COMMIT when it succeeded.
            // Chaining with `.and(...)` would evaluate the COMMIT eagerly —
            // committing even a failed statement — and would report the
            // COMMIT's row count instead of the statement's.
            let result = match conn.execute(sql, params.clone()).await {
                Ok(affected) => conn.execute("COMMIT", ()).await.map(|_| affected),
                Err(e) => Err(e),
            };

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

    /// Execute multiple owned write statements atomically under a MVCC concurrent transaction.
    ///
    /// Like [`Db::execute_writes`], but takes owned SQL strings so a caller can plan a
    /// batch dynamically (the P2P merge does this: it decides every write up front, then
    /// applies them all in one transaction so a failing write rolls the whole batch
    /// back). All statements share a single `BEGIN IMMEDIATE ... COMMIT`: either all
    /// succeed or all roll back. Retries on busy/conflict errors.
    pub async fn execute_write_batch(&self, stmts: Vec<(String, Params)>) -> Result<(), SyncError> {
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
            for (sql, p) in &stmts {
                match conn.execute(sql.as_str(), p.clone()).await {
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
    /// Apply every migration step the DB has not seen yet.
    ///
    /// Each step runs in its own `BEGIN IMMEDIATE ... COMMIT` transaction via
    /// [`migrate_step`](Self::migrate_step), which bumps `PRAGMA user_version`
    /// to the step number atomically with the schema change. A crash anywhere
    /// before the step's `COMMIT` rolls the whole step back, leaving the
    /// database at the previous version and the step re-runnable.
    /// Run migrations if needed, writing a timestamped backup first when any
    /// migration step is pending and backups are enabled.
    ///
    /// A failed pre-migration backup aborts the migration: this is fail
    /// closed on purpose — the data-safety gate (N-8.1) promises no migration
    /// ever runs without a backup on disk. Disable via
    /// [`BackupConfig::enabled`] (TOML `database.backup_enabled`) for
    /// environments where the operator takes backups another way.
    pub async fn migrate_with_backup(&self, cfg: &BackupConfig) -> Result<(), SyncError> {
        let start = self.schema_version().await?;
        if start >= CURRENT_SCHEMA_VERSION {
            debug!(version = start, "Database schema is up to date");
            return Ok(());
        }

        if cfg.enabled {
            let backup = self
                .backup_now(cfg, BackupReason::PreMigration)
                .await
                .map_err(|e| {
                    SyncError::DatabaseError(format!(
                        "pre-migration backup failed, refusing to migrate: {e}"
                    ))
                })?;
            info!(
                backup = %backup.display(),
                from_version = start,
                target_version = CURRENT_SCHEMA_VERSION,
                "Wrote pre-migration database backup"
            );
        } else {
            warn!(
                "Automatic backups are disabled (database.backup_enabled = false); \
                 migrating without a pre-migration backup"
            );
        }

        info!(
            current_version = start,
            target_version = CURRENT_SCHEMA_VERSION,
            "Running database migrations"
        );
        for step in (start + 1)..=CURRENT_SCHEMA_VERSION {
            self.migrate_step(step).await?;
        }
        info!("Migrations complete, schema version = {CURRENT_SCHEMA_VERSION}");
        Ok(())
    }

    /// Run migrations if needed, with the default backup policy (enabled,
    /// keep the last 7, backups next to the database). See
    /// [`migrate_with_backup`](Self::migrate_with_backup).
    pub async fn migrate(&self) -> Result<(), SyncError> {
        self.migrate_with_backup(&BackupConfig::default()).await
    }

    /// Current `PRAGMA user_version` of this database.
    async fn schema_version(&self) -> Result<i64, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn.query("PRAGMA user_version", ()).await?;
        if let Some(row) = rows.next().await? {
            Ok(row.get::<i64>(0)?)
        } else {
            Ok(0)
        }
    }

    /// Apply one migration step inside its own transaction, bumping
    /// `PRAGMA user_version` to the step number atomically with the schema
    /// change.
    ///
    /// SQLite rolls `PRAGMA user_version = N` back with the enclosing
    /// transaction, so a crash anywhere before `COMMIT` leaves the database at
    /// the previous version and the step re-runnable. Steps already applied
    /// are skipped.
    async fn migrate_step(&self, step: i64) -> Result<(), SyncError> {
        if self.schema_version().await? >= step {
            return Ok(());
        }

        let conn = self.connect().await?;
        for attempt in 0..3u32 {
            match conn.execute("BEGIN IMMEDIATE", ()).await {
                Ok(_) => break,
                Err(ref e) if is_retryable(e) && attempt < 2 => {
                    tokio::time::sleep(std::time::Duration::from_millis(100 * (1 << attempt)))
                        .await;
                }
                Err(e) => {
                    return Err(SyncError::DatabaseError(format!(
                        "Migration step {step}: failed to begin transaction: {e}"
                    )));
                }
            }
        }

        if let Err(e) = self.run_migration_step(&conn, step).await {
            let _ = conn.execute("ROLLBACK", ()).await;
            return Err(e);
        }

        conn.execute(&format!("PRAGMA user_version = {step}"), ())
            .await
            .map_err(|e| {
                SyncError::DatabaseError(format!(
                    "Migration step {step}: user_version bump failed: {e}"
                ))
            })?;

        match conn.execute("COMMIT", ()).await {
            Ok(_) => {
                debug!(step, "Applied migration step");
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(SyncError::DatabaseError(format!(
                    "Migration step {step}: commit failed: {e}"
                )))
            }
        }
    }

    /// The body of a single migration step. Runs inside the transaction opened
    /// by [`migrate_step`](Self::migrate_step); never call it directly.
    ///
    /// Two things about the numbering are worth knowing before adding a step,
    /// because neither is obvious from `migrations/`:
    ///
    /// * **A file-backed step's number is its schema version.** Step N runs
    ///   when `version < N`, and a step whose whole body is SQL lives in
    ///   `migrations/NNN_<name>.sql` with that same N — the `NNN_` prefix and
    ///   the `if version < N` gate must agree, so `migrations/` is the
    ///   complete, unambiguous list of file-backed steps.
    /// * **Not every step has a `.sql` file, and the `008` gap is deliberate.**
    ///   Steps that are idempotent `ALTER TABLE`s or data repairs are written
    ///   inline in Rust instead — `004` (company logo columns), `008`
    ///   (repairing the columns migration 007 dropped when it rebuilt
    ///   `platform_mappings`), `010`, `012`, `014`–`016`, `018`–`019`, `021`, `022`,
    ///   `023` and `025` are all inline. The inline step is the single authority for these
    ///   numbers: an orphan `.sql` file with an inline step's number must not
    ///   exist (three such orphans were deleted by N-3.2).
    ///
    /// Adding a step means bumping [`CURRENT_SCHEMA_VERSION`] and appending a
    /// new match arm here. Never edit or renumber a step that shipped.
    async fn run_migration_step(&self, conn: &Connection, step: i64) -> Result<(), SyncError> {
        match step {
            1 => {
                conn.execute_batch(include_str!("../../../migrations/001_initial.sql"))
                    .await?;
                debug!("Applied migration 001_initial.sql");
            }
            2 => {
                conn.execute_batch(include_str!("../../../migrations/002_p2p_company.sql"))
                    .await?;
                debug!("Applied migration 002_p2p_company.sql");
            }
            3 => {
                conn.execute_batch(include_str!("../../../migrations/003_multi_company.sql"))
                    .await?;

                // Migrate existing peers from company_peers → company_peers_v2
                self.migrate_peers_to_v2(conn).await?;

                // Add peer_id column to company table if it exists and doesn't have it
                self.add_company_peer_id_column(conn).await?;

                debug!("Applied migration 003_multi_company.sql");
            }
            4 => {
                // Add logo columns to company table (company DB)
                self.add_company_logo_columns(conn).await?;
                // Add logo column to company_registry table (app DB)
                self.add_registry_logo_column(conn).await?;
                debug!("Applied migration 004_company_logo");
            }
            5 => {
                conn.execute_batch(include_str!("../../../migrations/005_vendor_plugins.sql"))
                    .await?;
                debug!("Applied migration 005_vendor_plugins.sql");
            }
            6 => {
                conn.execute_batch(include_str!("../../../migrations/006_plugin_metadata.sql"))
                    .await?;
                debug!("Applied migration 006_plugin_metadata.sql");
            }
            7 => {
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
            8 => {
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
            9 => {
                conn.execute_batch(include_str!(
                    "../../../migrations/009_vendor_listings_cache.sql"
                ))
                .await?;
                debug!("Applied migration 009_vendor_listings_cache.sql");
            }
            10 => {
                self.migrate_always_variants(conn).await?;
                debug!("Applied migration 010_always_variants");
            }
            11 => {
                conn.execute_batch(include_str!("../../../migrations/011_cached_listings.sql"))
                    .await?;
                debug!("Applied migration 011_cached_listings.sql");
            }
            12 => {
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
                debug!("Applied migration 012: product_variants vendor source columns");
            }
            13 => {
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
                            row.get::<String>(0)?, // plugin_id
                            row.get::<String>(1)?, // vendor_item_id
                            // REAL at bootstrap time; migration 024 converts
                            // the column to TEXT afterwards.
                            row.get::<Option<f64>>(2)?.and_then(Decimal::from_f64), // price
                            row.get::<Option<String>>(3)?,                          // currency
                            row.get::<Option<String>>(4)?,                          // url
                            row.get::<String>(5)?,                                  // extras_json
                            row.get::<Option<String>>(6)?,                          // group_key
                        ));
                    }

                    for (pid, vid, price, currency, url, extras_json, group_key) in enrichments {
                        conn.execute(
                            "UPDATE product_vendor_data SET price = ?1, currency = ?2, url = ?3, extras_json = ?4, group_key = ?5
                             WHERE plugin_id = ?6 AND vendor_item_id = ?7",
                            params![
                                decimal_param_opt(price.as_ref()),
                                currency,
                                url,
                                extras_json,
                                group_key,
                                pid,
                                vid
                            ],
                        )
                        .await?;
                    }
                }

                debug!("Applied migration 013_product_vendor_data.sql (with backfill)");
            }
            14 => {
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
            15 => {
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
            16 => {
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
            17 => {
                conn.execute_batch(include_str!(
                    "../../../migrations/017_cached_platform_listings.sql"
                ))
                .await?;
                debug!("Applied migration 017: cached_platform_listings table");
            }
            18 => {
                // Cached fetch allowlist per registry plugin (see VendorPluginRow). NULL
                // means "not computed yet", which is exactly right for every existing row.
                let _ = conn
                    .execute(
                        "ALTER TABLE vendor_plugin_registry ADD COLUMN allowed_hosts TEXT",
                        (),
                    )
                    .await;
                debug!("Applied migration 018: vendor_plugin_registry.allowed_hosts");
            }
            19 => {
                // Per-install plugin time limit (local, never synced). NULL = default.
                let _ = conn
                    .execute(
                        "ALTER TABLE vendor_plugin_installs ADD COLUMN timeout_minutes INTEGER",
                        (),
                    )
                    .await;
                debug!("Applied migration 019: vendor_plugin_installs.timeout_minutes");
            }
            20 => {
                // Stock ledger: the stock_ledger table holds every stock change as
                // a uniquely keyed event; canonical quantity is the sum of deltas.
                conn.execute_batch(include_str!("../../../migrations/020_stock_ledger.sql"))
                    .await?;
                // Backfill one opening event per existing variant so the ledger
                // sum equals the quantities already stored. The backfill runs on
                // this step's connection so it commits or rolls back with the
                // step; `backfill_ledger_opening_events` is the same statement
                // for out-of-migration use.
                conn.execute(
                    "INSERT OR IGNORE INTO stock_ledger
                        (event_key, product_id, variant_id, event_kind, delta)
                     SELECT 'opening:' || pv.id, pv.product_id, pv.id, 'opening', pv.quantity
                     FROM product_variants pv",
                    (),
                )
                .await?;
                debug!("Applied migration 020: stock_ledger table + opening backfill");
            }
            21 => {
                // Variant-aware snapshots: one row per (product, platform,
                // variant) so restock detection and push decisions are keyed
                // per variant listing instead of per product. The old
                // (product, platform) rows are kept as the product-level
                // (variant_id = '') rows, so products without variant mappings
                // behave exactly as before.
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS platform_snapshots_v2 (
                        id                  INTEGER PRIMARY KEY,
                        product_id          TEXT NOT NULL REFERENCES products(id),
                        platform            TEXT NOT NULL,
                        variant_id          TEXT NOT NULL DEFAULT '',
                        last_known_quantity INTEGER NOT NULL,
                        last_polled_at      TEXT NOT NULL,
                        last_pushed_at      TEXT,
                        version_tag         TEXT
                    );
                    INSERT OR IGNORE INTO platform_snapshots_v2
                        (id, product_id, platform, variant_id, last_known_quantity, last_polled_at, last_pushed_at, version_tag)
                        SELECT id, product_id, platform, '', last_known_quantity, last_polled_at, last_pushed_at, version_tag
                        FROM platform_snapshots;
                    DROP TABLE platform_snapshots;
                    ALTER TABLE platform_snapshots_v2 RENAME TO platform_snapshots;
                    CREATE UNIQUE INDEX IF NOT EXISTS idx_snapshots_product_platform_variant
                        ON platform_snapshots(product_id, platform, variant_id);",
                )
                .await?;
                debug!("Applied migration 021: variant-aware platform_snapshots");
            }
            22 => {
                // Approval-bound code fingerprint (see crate::plugins). NULL means the
                // row was never reviewed under the binding (legacy rows, new imports,
                // P2P arrivals awaiting review).
                let _ = conn
                    .execute(
                        "ALTER TABLE vendor_plugin_registry ADD COLUMN code_hash TEXT",
                        (),
                    )
                    .await;
                debug!("Applied migration 022: vendor_plugin_registry.code_hash");
            }
            23 => {
                // Partial unique indexes for platform_mappings (N-4.5):
                // `delete_mapping` soft-deletes, but the unique indexes from
                // steps 007/015 span *all* rows, tombstones included — so
                // re-mapping a listing whose old mapping was deleted hit
                // UNIQUE and could never succeed. Rebuild the table (the
                // step 007/015 pattern: turso cannot DROP INDEX) and recreate
                // both unique indexes as partial, covering only live rows
                // (`WHERE deleted_at IS NULL`). Tombstones are preserved:
                // the partial indexes are strictly weaker than the old ones,
                // so every existing row still fits.
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS platform_mappings_partial (
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
                    INSERT INTO platform_mappings_partial
                        (id, product_id, platform, platform_item_id, platform_sku, is_active, deleted_at, updated_at, variant_id)
                        SELECT id, product_id, platform, platform_item_id, platform_sku, is_active, deleted_at, updated_at, variant_id
                        FROM platform_mappings;
                    DROP TABLE platform_mappings;
                    ALTER TABLE platform_mappings_partial RENAME TO platform_mappings;
                    CREATE UNIQUE INDEX IF NOT EXISTS idx_mappings_product_platform_variant
                        ON platform_mappings(product_id, platform, COALESCE(variant_id, ''))
                        WHERE deleted_at IS NULL;
                    CREATE UNIQUE INDEX IF NOT EXISTS idx_mappings_platform_item
                        ON platform_mappings(platform, platform_item_id)
                        WHERE deleted_at IS NULL;",
                )
                .await?;
                debug!("Applied migration 023: partial unique indexes on platform_mappings");
            }
            24 => {
                // Decimal money end to end (N-6.1): price/amount columns were
                // REAL, so a computed price could be stored and re-serialized
                // as 0.30000000000000004. They are now TEXT holding the exact
                // decimal string. SQLite's REAL affinity would coerce TEXT
                // back to REAL on write, so the columns must be rebuilt, not
                // just left in place. Existing REAL values convert via CAST,
                // which renders the shortest round-trip text ("19.99").
                //
                // The _v2 rebuild pattern follows migrations 016/021/023
                // (turso cannot DROP INDEX, but DROP TABLE + RENAME works).
                //
                // Idempotency: migrate() can run twice on one database (the
                // stock-ledger backfill test resets user_version and
                // re-migrates). A second run would collide: turso keeps the
                // autoindex name across ALTER TABLE RENAME, so recreating
                // product_vendor_data_v2 errors with
                // `sqlite_autoindex_product_vendor_data_v2_1 already exists`.
                // Skip the rebuild when the columns are already TEXT.
                let mut pragma_rows = conn
                    .query(
                        "SELECT type FROM pragma_table_info('pricing_snapshots') WHERE name = 'amount'",
                        (),
                    )
                    .await?;
                let already_decimal = match pragma_rows.next().await? {
                    Some(row) => row.get::<String>(0)?.eq_ignore_ascii_case("TEXT"),
                    None => false,
                };
                drop(pragma_rows);
                if already_decimal {
                    debug!("Skipping migration 024: money columns are already TEXT");
                } else {
                    conn.execute_batch(
                        "CREATE TABLE IF NOT EXISTS pricing_snapshots_v2 (
                        id          INTEGER PRIMARY KEY,
                        product_id  TEXT NOT NULL REFERENCES products(id),
                        platform    TEXT NOT NULL,
                        amount      TEXT NOT NULL,
                        currency    TEXT NOT NULL DEFAULT 'USD',
                        recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    INSERT OR IGNORE INTO pricing_snapshots_v2
                        (id, product_id, platform, amount, currency, recorded_at)
                        SELECT id, product_id, platform, CAST(amount AS TEXT), currency, recorded_at
                        FROM pricing_snapshots;
                    DROP TABLE pricing_snapshots;
                    ALTER TABLE pricing_snapshots_v2 RENAME TO pricing_snapshots;
                    CREATE INDEX IF NOT EXISTS idx_pricing_snapshots_product_time
                        ON pricing_snapshots(product_id, recorded_at);",
                    )
                    .await?;

                    conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS vendor_listings_cache_v2 (
                        id                 INTEGER PRIMARY KEY,
                        plugin_id          TEXT NOT NULL,
                        vendor_item_id     TEXT NOT NULL,
                        title              TEXT NOT NULL,
                        price              TEXT,
                        currency           TEXT,
                        quantity           INTEGER,
                        sku                TEXT,
                        image_url          TEXT,
                        url                TEXT,
                        extras_json        TEXT NOT NULL DEFAULT '{}',
                        group_key          TEXT,
                        variant_attrs_json TEXT NOT NULL DEFAULT '{}',
                        fetched_at         TEXT NOT NULL DEFAULT (datetime('now'))
                    );
                    INSERT OR IGNORE INTO vendor_listings_cache_v2
                        (id, plugin_id, vendor_item_id, title, price, currency, quantity, sku,
                         image_url, url, extras_json, group_key, variant_attrs_json, fetched_at)
                        SELECT id, plugin_id, vendor_item_id, title, CAST(price AS TEXT), currency, quantity, sku,
                         image_url, url, extras_json, group_key, variant_attrs_json, fetched_at
                        FROM vendor_listings_cache;
                    DROP TABLE vendor_listings_cache;
                    ALTER TABLE vendor_listings_cache_v2 RENAME TO vendor_listings_cache;
                    -- The UNIQUE(plugin_id, vendor_item_id) table constraint is
                    -- recreated as a *named* unique index: an inline UNIQUE
                    -- would mint sqlite_autoindex_vendor_listings_cache_v2_1,
                    -- which still exists under that name from migration 016's
                    -- rebuild (turso does not rename indexes on
                    -- ALTER TABLE RENAME), and the CREATE TABLE would fail.
                    -- INSERT OR REPLACE conflict detection works the same on
                    -- a unique index.
                    CREATE UNIQUE INDEX IF NOT EXISTS idx_vlc_plugin_item
                        ON vendor_listings_cache(plugin_id, vendor_item_id);
                    CREATE INDEX IF NOT EXISTS idx_vlc_plugin ON vendor_listings_cache(plugin_id);",
                )
                .await?;

                    conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS product_vendor_data_v2 (
                        product_id         TEXT NOT NULL,
                        variant_id         TEXT,
                        plugin_id          TEXT NOT NULL,
                        vendor_item_id     TEXT NOT NULL,
                        title              TEXT NOT NULL,
                        price              TEXT,
                        currency           TEXT,
                        quantity           INTEGER,
                        sku                TEXT,
                        image_url          TEXT,
                        url                TEXT,
                        extras_json        TEXT NOT NULL DEFAULT '{}',
                        group_key          TEXT,
                        variant_attrs_json TEXT NOT NULL DEFAULT '{}',
                        fetched_at         TEXT NOT NULL DEFAULT (datetime('now')),
                        updated_at         TEXT NOT NULL DEFAULT (datetime('now')),
                        PRIMARY KEY (product_id, plugin_id, vendor_item_id)
                    );
                    INSERT OR IGNORE INTO product_vendor_data_v2
                        (product_id, variant_id, plugin_id, vendor_item_id, title, price, currency,
                         quantity, sku, image_url, url, extras_json, group_key, variant_attrs_json,
                         fetched_at, updated_at)
                        SELECT product_id, variant_id, plugin_id, vendor_item_id, title, CAST(price AS TEXT), currency,
                         quantity, sku, image_url, url, extras_json, group_key, variant_attrs_json,
                         fetched_at, updated_at
                        FROM product_vendor_data;
                    DROP TABLE product_vendor_data;
                    ALTER TABLE product_vendor_data_v2 RENAME TO product_vendor_data;
                    CREATE INDEX IF NOT EXISTS idx_pvd_product ON product_vendor_data(product_id);
                    CREATE INDEX IF NOT EXISTS idx_pvd_plugin_item ON product_vendor_data(plugin_id, vendor_item_id);",
                )
                .await?;
                    debug!("Applied migration 024: decimal money columns (REAL -> TEXT)");
                }
            }
            25 => {
                // VendorListing freeze (N-7.2): `images: string[]` replaces the
                // single `image_url`, and UPC/GTIN, brand, MPN, weight and
                // dimensions become typed columns instead of `extras` strings.
                // Purely additive — the old `image_url` column stays and is
                // folded into `images` on read, so no row is rewritten.
                for table in ["vendor_listings_cache", "product_vendor_data"] {
                    for ddl in [
                        format!(
                            "ALTER TABLE {table} ADD COLUMN images_json TEXT NOT NULL DEFAULT '[]'"
                        ),
                        format!("ALTER TABLE {table} ADD COLUMN upc TEXT"),
                        format!("ALTER TABLE {table} ADD COLUMN brand TEXT"),
                        format!("ALTER TABLE {table} ADD COLUMN mpn TEXT"),
                        format!("ALTER TABLE {table} ADD COLUMN weight_json TEXT"),
                        format!("ALTER TABLE {table} ADD COLUMN dimensions_json TEXT"),
                    ] {
                        // Idempotent like the other inline ALTER steps: a
                        // re-run after a crash finds the column already there.
                        let _ = conn.execute(&ddl, ()).await;
                    }
                }
                debug!("Applied migration 025: vendor listing images + typed fields");
            }
            26 => {
                // Per-plugin KV storage behind `Nisaba.store` (N-7.3).
                conn.execute_batch(include_str!("../../../migrations/026_plugin_kv_store.sql"))
                    .await?;
                debug!("Applied migration 026_plugin_kv_store.sql");
            }
            27 => {
                // Host-owned incremental-sync state for `fetchListings({ since,
                // cursor })` (N-7.4).
                conn.execute_batch(include_str!(
                    "../../../migrations/027_vendor_sync_state.sql"
                ))
                .await?;
                debug!("Applied migration 027_vendor_sync_state.sql");
            }
            _ => {}
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

    /// Delete a product and every row that belongs to it, in one transaction.
    ///
    /// Every table keyed by `product_id` is cleaned up — including
    /// `listing_descriptions`, `listing_photos`, `pricing_snapshots` and
    /// `cached_listings`, which earlier versions of this method left behind as
    /// orphans pointing at a product that no longer exists.
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
                "DELETE FROM listing_descriptions WHERE product_id = ?1",
                p.clone(),
            ),
            (
                "DELETE FROM listing_photos WHERE product_id = ?1",
                p.clone(),
            ),
            (
                "DELETE FROM pricing_snapshots WHERE product_id = ?1",
                p.clone(),
            ),
            (
                "DELETE FROM cached_listings WHERE product_id = ?1",
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

    /// Write a variant received from a P2P peer, keeping the peer's `updated_at`.
    ///
    /// Merges are last-writer-wins on `updated_at`, so the timestamp is the edit's, not
    /// the merge's: stamping `now` here makes the receiver's copy look newer than the
    /// sender's, it flows back on the next sync, and the two peers rewrite each other
    /// forever — while a stale copy with a fresh stamp can beat a genuine edit made on
    /// the other side. The source-plugin link is written when `variant` carries one; a
    /// `None` (from a peer too old to send it) leaves the local link alone.
    pub async fn upsert_variant_from_peer(
        &self,
        variant: &ProductVariant,
    ) -> Result<(), SyncError> {
        let attrs_json = serde_json::to_string(&variant.attributes)
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;
        self.execute_write(
            "INSERT INTO product_variants (id, product_id, sku, name, attributes_json, quantity, on_hand_quantity, image_url, sort_order, source_plugin_id, source_vendor_item_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
             ON CONFLICT(id) DO UPDATE SET sku = ?3, name = ?4, attributes_json = ?5, quantity = ?6,
                on_hand_quantity = ?7, image_url = ?8, sort_order = ?9,
                source_plugin_id = COALESCE(?10, source_plugin_id),
                source_vendor_item_id = COALESCE(?11, source_vendor_item_id),
                updated_at = ?12",
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
                variant.updated_at.clone(),
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

    // ── Stock ledger (N-4.1) ────────────────────────────────────────
    // ── Stock ledger (N-4.1) ────────────────────────────────────────
    //
    // Every stock change is a row in `stock_ledger` with a UNIQUE `event_key`:
    // sales are keyed `sale:<platform>:<order_id>`, every other kind carries a
    // UUID (or the deterministic `opening:<variant_id>` of the backfilled
    // starting balance). Canonical quantity per variant is the SUM of its
    // events' deltas; `product_variants.quantity` stays a cached copy refreshed
    // by [`Db::refresh_variant_quantity_from_ledger`].

    /// Record one stock event. Idempotent: if an event with the same
    /// `event_key` is already in the ledger (a retried sale, a re-run
    /// backfill), the row is left alone and this returns `false`.
    pub async fn record_stock_event(&self, event: &StockEvent) -> Result<bool, SyncError> {
        // INSERT OR IGNORE ... then SELECT changes() on the same connection,
        // inside one transaction, to tell an ignored duplicate from a new row.
        let sql = "INSERT OR IGNORE INTO stock_ledger
            (event_key, product_id, variant_id, event_kind, delta, platform, source_order_id, note)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";
        let p = to_params(params![
            event.event_key.clone(),
            event.product_id.clone(),
            event.variant_id.clone(),
            event.kind.as_str(),
            event.delta,
            event.platform.clone(),
            event.source_order_id.clone(),
            event.note.clone(),
        ]);

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

            let insert_changed: Result<i64, TursoError> = async {
                conn.execute(sql, p.clone()).await?;
                let mut rows = conn.query("SELECT changes()", ()).await?;
                let n: i64 = rows
                    .next()
                    .await?
                    .ok_or_else(|| TursoError::Error("changes() returned no rows".into()))?
                    .get(0)?;
                Ok(n)
            }
            .await;
            let commit = match insert_changed {
                Ok(n) => conn.execute("COMMIT", ()).await.map(|_| n),
                Err(e) => Err(e),
            };

            match commit {
                Ok(n) => return Ok(n > 0),
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
        Err(SyncError::DatabaseError(
            "record_stock_event failed after 5 retries".to_string(),
        ))
    }

    /// Canonical quantity for a variant: the sum of its ledger events' deltas.
    /// A variant with no events has canonical quantity 0.
    pub async fn canonical_variant_quantity(&self, variant_id: &str) -> Result<i64, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT COALESCE(SUM(delta), 0) FROM stock_ledger WHERE variant_id = ?1",
                params![variant_id],
            )
            .await?;
        let qty: i64 = rows
            .next()
            .await?
            .ok_or_else(|| SyncError::DatabaseError("SUM query returned no rows".into()))?
            .get(0)?;
        Ok(qty)
    }

    /// All ledger events for one variant, oldest first.
    pub async fn stock_events_for_variant(
        &self,
        variant_id: &str,
    ) -> Result<Vec<StockEvent>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, event_key, product_id, variant_id, event_kind, delta, platform, source_order_id, note, created_at
                 FROM stock_ledger WHERE variant_id = ?1 ORDER BY id",
                params![variant_id],
            )
            .await?;
        let mut events = Vec::new();
        while let Some(row) = rows.next().await? {
            events.push(parse_stock_event(&row)?);
        }
        Ok(events)
    }

    /// Recompute a variant's cached `quantity` from its ledger events and
    /// return the canonical value.
    pub async fn refresh_variant_quantity_from_ledger(
        &self,
        variant_id: &str,
    ) -> Result<i64, SyncError> {
        let canonical = self.canonical_variant_quantity(variant_id).await?;
        self.execute_write(
            "UPDATE product_variants SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![canonical, variant_id],
        )
        .await?;
        Ok(canonical)
    }

    /// Backfill migration helper: one `opening` event per variant, keyed
    /// `opening:<variant_id>`, with the variant's currently stored quantity.
    /// INSERT OR IGNORE makes re-runs (e.g. an interrupted migration that did
    /// not bump `user_version`) a no-op instead of double-counting.
    pub async fn backfill_ledger_opening_events(&self) -> Result<(), SyncError> {
        let conn = self.connect().await?;
        conn.execute(
            "INSERT OR IGNORE INTO stock_ledger
                (event_key, product_id, variant_id, event_kind, delta)
             SELECT 'opening:' || pv.id, pv.product_id, pv.id, 'opening', pv.quantity
             FROM product_variants pv",
            (),
        )
        .await?;
        Ok(())
    }

    /// Delete a variant and every row that belongs to it, in one transaction —
    /// its platform mappings and its vendor data, so no orphans remain.
    pub async fn delete_variant(&self, id: &str) -> Result<(), SyncError> {
        let p = to_params(params![id]);
        self.execute_writes(&[
            (
                "DELETE FROM platform_mappings WHERE variant_id = ?1",
                p.clone(),
            ),
            (
                "DELETE FROM product_vendor_data WHERE variant_id = ?1",
                p.clone(),
            ),
            ("DELETE FROM product_variants WHERE id = ?1", p),
        ])
        .await?;
        Ok(())
    }

    /// Delete all of a product's variants and the rows that belong to them, in
    /// one transaction — their platform mappings and vendor data, so no
    /// orphans remain.
    pub async fn delete_variants_for_product(&self, product_id: &str) -> Result<(), SyncError> {
        let p = to_params(params![product_id]);
        self.execute_writes(&[
            ("DELETE FROM platform_mappings WHERE variant_id IN (SELECT id FROM product_variants WHERE product_id = ?1)", p.clone()),
            ("DELETE FROM product_vendor_data WHERE product_id = ?1", p.clone()),
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
                        COALESCE(vpr.include_vendor_stock, 0) AS include_vendor,
                        pv.quantity
                 FROM product_variants pv
                 LEFT JOIN product_vendor_data pvd
                   ON pvd.variant_id = pv.id AND pvd.plugin_id = pv.source_plugin_id
                 LEFT JOIN vendor_plugin_registry vpr
                   ON vpr.id = pv.source_plugin_id
                 WHERE pv.product_id = ?1",
                params![product_id],
            )
            .await?;

        let mut total: i64 = 0;
        let mut updates: Vec<(String, i64)> = Vec::new();
        while let Some(row) = rows.next().await? {
            let variant_id: String = row.get(0)?;
            let on_hand: i64 = row.get(1)?;
            let source_plugin: Option<String> = row.get::<Option<String>>(2)?;
            let vendor_qty: i64 = row.get(3)?;
            let include_vendor: i64 = row.get(4)?;
            let current: i64 = row.get(5)?;

            let effective = if source_plugin.is_some() && include_vendor == 1 {
                on_hand + vendor_qty
            } else {
                on_hand
            };
            total += effective;
            // Only rows whose quantity actually changes are written: `updated_at` is the
            // last-writer-wins clock for P2P merges, and re-stamping unchanged rows makes
            // them look like fresh edits to every peer.
            if effective != current {
                updates.push((variant_id, effective));
            }
        }

        let mut rows = conn
            .query(
                "SELECT quantity FROM products WHERE id = ?1",
                params![product_id],
            )
            .await?;
        let product_current: Option<i64> = match rows.next().await? {
            Some(row) => Some(row.get(0)?),
            None => None,
        };

        // Step 2: Update the changed variants + product total atomically
        let mut stmts: Vec<(&str, Params)> = Vec::new();
        for (variant_id, quantity) in &updates {
            stmts.push((
                "UPDATE product_variants SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2",
                to_params(params![*quantity, variant_id.as_str()]),
            ));
        }
        if product_current != Some(total) {
            stmts.push((
                "UPDATE products SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2",
                to_params(params![total, product_id]),
            ));
        }
        if !stmts.is_empty() {
            self.execute_writes(&stmts).await?;
        }
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

    /// The live (non-deleted) mapping for one product on one platform for one
    /// variant, if any. This is the row the partial unique index
    /// `idx_mappings_product_platform_variant` protects, so a `Some` here
    /// means a new mapping for the same triple would fail with UNIQUE.
    pub async fn get_mapping_for_product_platform_variant(
        &self,
        product_id: &str,
        platform: &str,
        variant_id: &str,
    ) -> Result<Option<PlatformMapping>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, product_id, platform, platform_item_id, platform_sku, is_active, variant_id
                 FROM platform_mappings
                 WHERE product_id = ?1 AND platform = ?2
                   AND COALESCE(variant_id, '') = COALESCE(?3, '')
                   AND deleted_at IS NULL",
                params![product_id, platform, variant_id],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            Ok(Some(parse_mapping(&row)?))
        } else {
            Ok(None)
        }
    }

    /// Record a listing that was just created on a platform: insert its
    /// mapping and, when `listing_json` is given, its `cached_listings`
    /// record, in a single transaction.
    ///
    /// The two writes either both land or both roll back, so the local
    /// record of a created listing can never exist without its mapping (or
    /// vice versa). The live listing itself is created by the caller *after*
    /// a pre-flight check (see
    /// [`get_mapping_for_product_platform_variant`](Self::get_mapping_for_product_platform_variant)):
    /// no platform adapter can delete a listing it created, so the mapping
    /// conflict that used to strand an unmapped live listing is rejected
    /// before anything is created remotely, not discovered afterwards.
    pub async fn record_created_listing(
        &self,
        product_id: &str,
        platform: &str,
        platform_item_id: &str,
        platform_sku: Option<&str>,
        variant_id: &str,
        listing_json: Option<&str>,
    ) -> Result<(), SyncError> {
        let mapping = (
            "INSERT INTO platform_mappings (product_id, platform, platform_item_id, platform_sku, variant_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            to_params(params![
                product_id,
                platform,
                platform_item_id,
                platform_sku,
                variant_id
            ]),
        );
        match listing_json {
            Some(json) => {
                self.execute_writes(&[
                    mapping,
                    (
                        "INSERT INTO cached_listings (product_id, platform, platform_item_id, listing_json, fetched_at)
                         VALUES (?1, ?2, ?3, ?4, datetime('now'))",
                        to_params(params![product_id, platform, platform_item_id, json]),
                    ),
                ])
                .await
            }
            None => {
                let (sql, p) = mapping;
                self.execute_writes(&[(sql, p)]).await
            }
        }
    }

    // ── Platform Snapshots ────────────────────────────────────

    pub async fn get_snapshot(
        &self,
        product_id: &str,
        platform: &str,
        variant_id: Option<&str>,
    ) -> Result<Option<PlatformSnapshot>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT id, product_id, platform, variant_id, last_known_quantity, last_polled_at, last_pushed_at, version_tag
                 FROM platform_snapshots WHERE product_id = ?1 AND platform = ?2 AND variant_id = ?3",
                params![product_id, platform, variant_id.unwrap_or("")],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            let stored_variant: String = row.get::<String>(3)?;
            Ok(Some(PlatformSnapshot {
                id: row.get::<i64>(0)?,
                product_id: row.get::<String>(1)?,
                platform: Platform::from_str_loose(&row.get::<String>(2)?)
                    .unwrap_or(Platform::Ebay),
                variant_id: (!stored_variant.is_empty()).then_some(stored_variant),
                last_known_quantity: row.get::<i64>(4)?,
                last_polled_at: row.get::<String>(5)?,
                last_pushed_at: row.get::<Option<String>>(6)?,
                version_tag: row.get::<Option<String>>(7)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn upsert_snapshot(
        &self,
        product_id: &str,
        platform: &str,
        variant_id: Option<&str>,
        quantity: i64,
        polled_at: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO platform_snapshots (product_id, platform, variant_id, last_known_quantity, last_polled_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(product_id, platform, variant_id) DO UPDATE
             SET last_known_quantity = ?4, last_polled_at = ?5",
            params![
                product_id,
                platform,
                variant_id.unwrap_or(""),
                quantity,
                polled_at
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn update_snapshot_pushed(
        &self,
        product_id: &str,
        platform: &str,
        variant_id: Option<&str>,
        quantity: i64,
        pushed_at: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE platform_snapshots
             SET last_known_quantity = ?1, last_pushed_at = ?2
             WHERE product_id = ?3 AND platform = ?4 AND variant_id = ?5",
            params![
                quantity,
                pushed_at,
                product_id,
                platform,
                variant_id.unwrap_or("")
            ],
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

    /// Write a platform auth token received from a P2P peer, keeping the peer's
    /// `updated_at` — see [`Db::upsert_variant_from_peer`] for why a merge must not stamp
    /// `now`. For tokens the stale-copy-wins case is an auth failure: a token one peer just
    /// refreshed can be overwritten by the other peer's re-stamped old one.
    pub async fn upsert_auth_token_from_peer(
        &self,
        platform: &str,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_at: Option<&str>,
        cookies: Option<&str>,
        updated_at: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO auth_tokens (platform, access_token, refresh_token, expires_at, cookies, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(platform) DO UPDATE
             SET access_token = ?2, refresh_token = ?3, expires_at = ?4, cookies = ?5, updated_at = ?6",
            params![platform, access_token, refresh_token, expires_at, cookies, updated_at],
        )
        .await?;
        Ok(())
    }

    // ── Inventory History ─────────────────────────────────────

    /// Record a product's quantity in the history time series — but only
    /// when it differs from the most recently recorded quantity for that
    /// product. History is a record of *changes*: writing a row for every
    /// sync cycle even when nothing moved would add ~288 identical rows per
    /// product per day at the default 5-minute schedule and drown the actual
    /// changes. The first observation of a product always records a row.
    pub async fn record_inventory_history(
        &self,
        product_id: &str,
        quantity: i64,
        sync_cycle_id: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO inventory_history (product_id, quantity, sync_cycle_id)
             SELECT ?1, ?2, ?3
             WHERE NOT EXISTS (
                 SELECT 1 FROM inventory_history
                 WHERE product_id = ?1
                   AND quantity = ?2
                   AND id = (SELECT MAX(id) FROM inventory_history
                             WHERE product_id = ?1)
             )",
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
        amount: Decimal,
        currency: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO pricing_snapshots (product_id, platform, amount, currency)
             VALUES (?1, ?2, ?3, ?4)",
            params![product_id, platform, decimal_param(&amount), currency],
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
                amount: parse_decimal("pricing_snapshots.amount", &row.get::<String>(3)?)?,
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
                amount: parse_decimal("pricing_snapshots.amount", &row.get::<String>(3)?)?,
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
                amount: parse_decimal("pricing_snapshots.amount", &row.get::<String>(3)?)?,
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

    /// Update a product's canonical quantity and mark the XMR orders whose
    /// sales that write applied as processed, all in a single transaction.
    ///
    /// The order marks and the stock write share one `BEGIN IMMEDIATE ...
    /// COMMIT`: either both land or neither does. Marking an order processed
    /// in an earlier, separate write is what used to lose sales — a cycle
    /// that failed between the mark and the quantity update left the order
    /// consumed while the stock was never decremented, so the next cycle
    /// skipped the sale. Pass `None` for `new_quantity` when the sale was
    /// folded into the resolution without changing the quantity (e.g. stock
    /// was already at zero): the orders are still consumed, in their own
    /// transaction, so they cannot be re-applied by the next cycle.
    pub async fn apply_stock_and_consume_xmr_sales(
        &self,
        product_id: &str,
        new_quantity: Option<i64>,
        sales: &[ConsumedXmrOrder],
    ) -> Result<(), SyncError> {
        let mut stmts: Vec<(&str, Params)> = Vec::with_capacity(sales.len() + 1);
        if let Some(quantity) = new_quantity {
            stmts.push((
                "UPDATE products SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2",
                to_params(params![quantity, product_id]),
            ));
        }
        for sale in sales {
            stmts.push((
                "INSERT OR IGNORE INTO xmr_processed_orders (order_id, product_id, listing_id, quantity)
                 VALUES (?1, ?2, ?3, ?4)",
                to_params(params![
                    sale.order_id.as_str(),
                    sale.product_id.as_deref(),
                    sale.listing_id.as_str(),
                    sale.quantity
                ]),
            ));
        }
        self.execute_writes(&stmts).await
    }

    /// Variant-group sibling of [`Db::apply_stock_and_consume_xmr_sales`]:
    /// update a variant's canonical quantity and mark the XMR orders whose
    /// sales that write applied as processed, all in a single transaction.
    /// Pass `None` for `new_quantity` when the sale was folded into the
    /// resolution without changing the quantity: the orders are still
    /// consumed, in their own transaction.
    pub async fn apply_variant_stock_and_consume_xmr_sales(
        &self,
        variant_id: &str,
        new_quantity: Option<i64>,
        sales: &[ConsumedXmrOrder],
    ) -> Result<(), SyncError> {
        let mut stmts: Vec<(&str, Params)> = Vec::with_capacity(sales.len() + 1);
        if let Some(quantity) = new_quantity {
            stmts.push((
                "UPDATE product_variants SET quantity = ?1, updated_at = datetime('now') WHERE id = ?2",
                to_params(params![quantity, variant_id]),
            ));
        }
        for sale in sales {
            stmts.push((
                "INSERT OR IGNORE INTO xmr_processed_orders (order_id, product_id, listing_id, quantity)
                 VALUES (?1, ?2, ?3, ?4)",
                to_params(params![
                    sale.order_id.as_str(),
                    sale.product_id.as_deref(),
                    sale.listing_id.as_str(),
                    sale.quantity
                ]),
            ));
        }
        self.execute_writes(&stmts).await
    }

    pub async fn update_snapshot_version_tag(
        &self,
        product_id: &str,
        platform: &str,
        variant_id: Option<&str>,
        version_tag: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE platform_snapshots SET version_tag = ?1
             WHERE product_id = ?2 AND platform = ?3 AND variant_id = ?4",
            params![version_tag, product_id, platform, variant_id.unwrap_or("")],
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
                "SELECT id, product_id, platform, variant_id, last_known_quantity, last_polled_at, last_pushed_at, version_tag
                 FROM platform_snapshots",
                (),
            )
            .await?;

        let mut snapshots = Vec::new();
        while let Some(row) = rows.next().await? {
            let stored_variant: String = row.get::<String>(3)?;
            snapshots.push(PlatformSnapshot {
                id: row.get::<i64>(0)?,
                product_id: row.get::<String>(1)?,
                platform: Platform::from_str_loose(&row.get::<String>(2)?)
                    .unwrap_or(Platform::Ebay),
                variant_id: (!stored_variant.is_empty()).then_some(stored_variant),
                last_known_quantity: row.get::<i64>(4)?,
                last_polled_at: row.get::<String>(5)?,
                last_pushed_at: row.get::<Option<String>>(6)?,
                version_tag: row.get::<Option<String>>(7)?,
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
    async fn migrate_always_variants(&self, conn: &Connection) -> Result<(), SyncError> {
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

    async fn migrate_peers_to_v2(&self, conn: &Connection) -> Result<(), SyncError> {
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

    async fn add_company_peer_id_column(&self, conn: &Connection) -> Result<(), SyncError> {
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

    async fn add_company_logo_columns(&self, conn: &Connection) -> Result<(), SyncError> {
        // Try to add columns; ignore errors if table doesn't exist or column already exists
        let _ = conn
            .execute_batch("ALTER TABLE company ADD COLUMN logo TEXT")
            .await;
        let _ = conn
            .execute_batch("ALTER TABLE company ADD COLUMN logo_updated_at TEXT")
            .await;
        Ok(())
    }

    async fn add_registry_logo_column(&self, conn: &Connection) -> Result<(), SyncError> {
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
                        category, icon, include_vendor_stock, created_at, updated_at,
                        allowed_hosts, code_hash
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
                        category, icon, include_vendor_stock, created_at, updated_at,
                        allowed_hosts, code_hash
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
                        category, icon, include_vendor_stock, created_at, updated_at,
                        allowed_hosts, code_hash
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
                 category, icon, include_vendor_stock, created_at, updated_at, allowed_hosts,
                 code_hash)
             VALUES (?1, ?2, ?3, ?4, '', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
             ON CONFLICT(id) DO UPDATE SET
                plugin_file = ?2, display_name = ?3, description = ?4, files_json = ?5,
                version = ?6, config_fields = ?7, config_json = ?8, status = ?9,
                submitted_by = ?10, approved_by = ?11, category = ?12, icon = ?13,
                include_vendor_stock = ?14, updated_at = ?16, allowed_hosts = ?17,
                code_hash = ?18",
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
                plugin.allowed_hosts_json.clone(),
                plugin.code_hash.clone(),
            ],
        )
        .await?;
        Ok(())
    }

    /// Cache a registry plugin's fetch allowlist, as computed from its files (see
    /// [`VendorPluginRow::allowed_hosts_json`]).
    pub async fn set_registry_plugin_allowed_hosts(
        &self,
        id: &str,
        allowed_hosts_json: Option<&str>,
    ) -> Result<(), SyncError> {
        let conn = self.connect().await?;
        conn.execute(
            "UPDATE vendor_plugin_registry SET allowed_hosts = ?1 WHERE id = ?2",
            params![allowed_hosts_json.map(str::to_string), id],
        )
        .await?;
        Ok(())
    }

    /// Record an admin review decision on a registry plugin, binding the decision to
    /// the plugin's current code fingerprint: any later re-import or P2P merge whose
    /// code or fetch allowlist no longer matches this hash resets the plugin to
    /// `pending` (see [`crate::plugins::reconcile_plugin_approval`]). A rejection is
    /// bound the same way, so a changed re-submission comes back as `pending`.
    pub async fn review_registry_plugin(
        &self,
        id: &str,
        status: &str,
        reviewed_by: Option<&str>,
    ) -> Result<(), SyncError> {
        let code_hash = match self.get_registry_plugin(id).await? {
            Some(row) => {
                crate::plugins::plugin_code_fingerprint(&row.files_json, &row.allowed_hosts_json)
            }
            None => None,
        };
        self.execute_write(
            "UPDATE vendor_plugin_registry SET status = ?1, approved_by = ?2, code_hash = ?3, updated_at = datetime('now') WHERE id = ?4",
            params![status, reviewed_by, code_hash, id],
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

    /// Store a plugin's config map, routing `secret: true` values to the
    /// secret store (the OS keyring in production) instead of `config_json`.
    ///
    /// Which keys are secret comes from the plugin's own `config_fields`
    /// metadata. An empty secret value deletes the stored entry — that is how
    /// a secret is cleared from the config form. If the secret store is
    /// unavailable the value is kept in `config_json` (the old behaviour) so
    /// the plugin keeps working; a warning is logged.
    pub async fn set_plugin_config_split(
        &self,
        plugin_id: &str,
        config: &HashMap<String, String>,
        secrets: &dyn PluginSecretStore,
    ) -> Result<(), SyncError> {
        let secret_keys = match self.get_registry_plugin(plugin_id).await? {
            Some(plugin) => secret_config_keys(plugin.config_fields.as_deref()),
            None => HashSet::new(),
        };
        let mut plain = HashMap::new();
        for (key, value) in config {
            if secret_keys.contains(key) {
                if value.is_empty() {
                    if let Err(e) = secrets.delete(plugin_id, key) {
                        warn!(plugin = %plugin_id, key, "Failed to delete plugin secret: {e}");
                    }
                } else if let Err(e) = secrets.store(plugin_id, key, value) {
                    warn!(plugin = %plugin_id, key,
                           "Keyring unavailable, keeping plugin secret in config_json: {e}");
                    plain.insert(key.clone(), value.clone());
                }
            } else {
                plain.insert(key.clone(), value.clone());
            }
        }
        let config_json = serde_json::to_string(&plain).map_err(|e| {
            SyncError::ConfigError(format!("Failed to serialise plugin config: {e}"))
        })?;
        self.set_plugin_config(plugin_id, &config_json).await
    }

    /// Load a plugin's effective config: non-secret values from `config_json`
    /// plus `secret: true` values injected from the secret store.
    ///
    /// Any plaintext secrets still sitting in `config_json` from before this
    /// mechanism existed are moved to the secret store and scrubbed from the
    /// database on this first load. Secret-store failures degrade to whatever
    /// `config_json` holds (a warning is logged) so plugins keep running on
    /// machines without an OS keyring.
    pub async fn get_plugin_config_merged(
        &self,
        plugin_id: &str,
        secrets: &dyn PluginSecretStore,
    ) -> Result<HashMap<String, String>, SyncError> {
        self.migrate_plugin_secrets_for(plugin_id, secrets).await?;
        let plugin = self
            .get_registry_plugin(plugin_id)
            .await?
            .ok_or_else(|| SyncError::ConfigError(format!("Plugin not found: {plugin_id}")))?;
        let mut config: HashMap<String, String> = plugin
            .config_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        for key in secret_config_keys(plugin.config_fields.as_deref()) {
            match secrets.load(plugin_id, &key) {
                Ok(Some(value)) => {
                    config.insert(key, value);
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(plugin = %plugin_id, key,
                           "Failed to load plugin secret from keyring: {e}");
                }
            }
        }
        Ok(config)
    }

    /// Move one plugin's plaintext `secret: true` values out of `config_json`
    /// into the secret store, scrubbing them from the database. Returns the
    /// number of values moved.
    ///
    /// A value is only removed from `config_json` after the secret store
    /// reads it back identically; on any failure the plaintext copy is kept
    /// (it may be the only one).
    pub async fn migrate_plugin_secrets_for(
        &self,
        plugin_id: &str,
        secrets: &dyn PluginSecretStore,
    ) -> Result<usize, SyncError> {
        let plugin = match self.get_registry_plugin(plugin_id).await? {
            Some(plugin) => plugin,
            None => return Ok(0),
        };
        let secret_keys = secret_config_keys(plugin.config_fields.as_deref());
        if secret_keys.is_empty() {
            return Ok(0);
        }
        let config: HashMap<String, String> = plugin
            .config_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        let mut moved = 0;
        for key in &secret_keys {
            let Some(value) = config.get(key) else {
                continue;
            };
            if value.is_empty() {
                continue;
            }
            match secrets.store(plugin_id, key, value) {
                Ok(()) => match secrets.load(plugin_id, key) {
                    Ok(Some(saved)) if saved == *value => moved += 1,
                    _ => warn!(plugin = %plugin_id, key,
                               "Keyring did not return the secret just stored; keeping plaintext copy"),
                },
                Err(e) => warn!(plugin = %plugin_id, key,
                                "Keyring unavailable, keeping plaintext plugin secret in config_json: {e}"),
            }
        }
        if moved > 0 {
            let scrubbed = strip_secret_values(
                plugin.config_fields.as_deref(),
                plugin.config_json.as_deref(),
            )
            .unwrap_or_else(|| "{}".to_string());
            self.set_plugin_config(plugin_id, &scrubbed).await?;
            info!(plugin = %plugin_id, moved,
                   "Migrated plugin secrets from config_json to the secret store");
        }
        Ok(moved)
    }

    /// Move every registry plugin's plaintext `secret: true` values into the
    /// secret store. Returns the total number of values moved.
    pub async fn migrate_plugin_secrets_to_keyring(
        &self,
        secrets: &dyn PluginSecretStore,
    ) -> Result<usize, SyncError> {
        let mut total = 0;
        for plugin in self.list_registry_plugins().await? {
            total += self.migrate_plugin_secrets_for(&plugin.id, secrets).await?;
        }
        Ok(total)
    }

    pub async fn delete_registry_plugin(&self, id: &str) -> Result<(), SyncError> {
        self.delete_registry_plugin_with_secrets(id, &crate::crypto::OsKeyringPluginSecrets)
            .await
    }

    /// Delete a registry plugin, also removing its secrets from the secret
    /// store so nothing lingers in the OS keyring after removal.
    pub async fn delete_registry_plugin_with_secrets(
        &self,
        id: &str,
        secrets: &dyn PluginSecretStore,
    ) -> Result<(), SyncError> {
        if let Some(plugin) = self.get_registry_plugin(id).await? {
            for key in secret_config_keys(plugin.config_fields.as_deref()) {
                if let Err(e) = secrets.delete(id, &key) {
                    warn!(plugin = %id, key, "Failed to delete plugin secret from keyring: {e}");
                }
            }
        }
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
                "SELECT plugin_id, installed, enabled, installed_at, timeout_minutes
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
                timeout_minutes: row.get::<Option<i64>>(4)?,
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
        // A removed plugin's cursors must not linger for a later re-install.
        self.clear_plugin_store(plugin_id).await?;
        // Same for the host's incremental-sync state: a re-install starts
        // from a full pull, never a stale cursor.
        self.clear_vendor_sync_state(plugin_id).await?;
        Ok(())
    }

    /// Read one key from a plugin's private KV store (`Nisaba.store.get`).
    ///
    /// The store lives in the company database, so keys are scoped to
    /// plugin + company by construction: a plugin can only ever read its own
    /// rows, and the same plugin under another company reads another table.
    pub async fn plugin_store_get(
        &self,
        plugin_id: &str,
        key: &str,
    ) -> Result<Option<String>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT value FROM plugin_kv_store WHERE plugin_id = ?1 AND key = ?2",
                params![plugin_id, key],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some(row.get::<String>(0)?))
        } else {
            Ok(None)
        }
    }

    /// Write one key in a plugin's private KV store (`Nisaba.store.set`).
    ///
    /// Upserts. The per-plugin size cap is enforced by the plugin runtime
    /// before this is called, so the rejection message can name the API.
    pub async fn plugin_store_set(
        &self,
        plugin_id: &str,
        key: &str,
        value: &str,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT INTO plugin_kv_store (plugin_id, key, value, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT (plugin_id, key) DO UPDATE
             SET value = excluded.value, updated_at = datetime('now')",
            params![plugin_id, key, value],
        )
        .await?;
        Ok(())
    }

    /// Delete one key from a plugin's private KV store
    /// (`Nisaba.store.delete`). Returns whether the key existed.
    pub async fn plugin_store_delete(&self, plugin_id: &str, key: &str) -> Result<bool, SyncError> {
        let removed = self
            .execute_write(
                "DELETE FROM plugin_kv_store WHERE plugin_id = ?1 AND key = ?2",
                params![plugin_id, key],
            )
            .await?;
        Ok(removed > 0)
    }

    /// Total stored bytes (key + value, UTF-8) for one plugin. The plugin
    /// runtime charges `Nisaba.store.set` writes against the per-plugin cap
    /// using this number.
    pub async fn plugin_store_total_bytes(&self, plugin_id: &str) -> Result<u64, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT COALESCE(SUM(LENGTH(key) + LENGTH(value)), 0)
                 FROM plugin_kv_store WHERE plugin_id = ?1",
                params![plugin_id],
            )
            .await?;
        let total: i64 = rows
            .next()
            .await?
            .map(|row| row.get::<i64>(0))
            .transpose()?
            .unwrap_or(0);
        Ok(total.max(0) as u64)
    }

    /// Drop a plugin's whole KV store. Called on uninstall; also useful when a
    /// plugin's stored cursor is known to be stale.
    pub async fn clear_plugin_store(&self, plugin_id: &str) -> Result<(), SyncError> {
        self.execute_write(
            "DELETE FROM plugin_kv_store WHERE plugin_id = ?1",
            params![plugin_id],
        )
        .await?;
        Ok(())
    }

    /// Set this machine's time limit for a plugin (`None` restores the default).
    pub async fn set_plugin_timeout(
        &self,
        plugin_id: &str,
        timeout_minutes: Option<i64>,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "UPDATE vendor_plugin_installs SET timeout_minutes = ?1 WHERE plugin_id = ?2",
            params![timeout_minutes, plugin_id],
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
                 (plugin_id, vendor_item_id, title, price, currency, quantity, sku, image_url, url, extras_json, group_key, variant_attrs_json, fetched_at,
                  images_json, upc, brand, mpn, weight_json, dimensions_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                         ?14, ?15, ?16, ?17, ?18, ?19)",
                params![
                    plugin_id,
                    l.vendor_item_id.clone(),
                    l.title.clone(),
                    decimal_param_opt(l.price.as_ref()),
                    l.currency.clone(),
                    l.quantity,
                    l.sku.clone(),
                    // Legacy column, kept NULL on new rows (N-7.2): `images_json`
                    // is authoritative; the old value is folded on read.
                    Option::<String>::None,
                    l.url.clone(),
                    extras_json,
                    l.group_key.clone(),
                    variant_attrs_json,
                    l.fetched_at.clone(),
                    json_param(&l.images),
                    l.upc.clone(),
                    l.brand.clone(),
                    l.mpn.clone(),
                    json_param_opt(l.weight.as_ref()),
                    json_param_opt(l.dimensions.as_ref()),
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
                        image_url, url, extras_json, group_key, variant_attrs_json, fetched_at,
                        images_json, upc, brand, mpn, weight_json, dimensions_json
                 FROM vendor_listings_cache WHERE plugin_id = ?1
                 ORDER BY title",
                params![plugin_id],
            )
            .await?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            let extras_json: String = row.get::<String>(9)?;
            let variant_attrs_json: String = row.get::<String>(11)?;
            let image_url: Option<String> = row.get::<Option<String>>(7)?;
            // N-7.2: rows written before the freeze only have `image_url`;
            // fold it into `images` so readers see one shape.
            let mut images: Vec<String> = parse_json_col(row.get::<Option<String>>(13)?);
            if images.is_empty() {
                images = image_url.clone().into_iter().collect();
            }
            out.push(VendorListingCache {
                plugin_id: row.get::<String>(0)?,
                vendor_item_id: row.get::<String>(1)?,
                title: row.get::<String>(2)?,
                price: parse_decimal_opt(
                    "vendor_listings_cache.price",
                    row.get::<Option<String>>(3)?,
                )?,
                currency: row.get::<Option<String>>(4)?,
                quantity: row.get::<Option<i64>>(5)?,
                sku: row.get::<Option<String>>(6)?,
                images,
                image_url,
                url: row.get::<Option<String>>(8)?,
                upc: row.get::<Option<String>>(14)?,
                brand: row.get::<Option<String>>(15)?,
                mpn: row.get::<Option<String>>(16)?,
                weight: parse_json_col::<Option<Weight>>(row.get::<Option<String>>(17)?),
                dimensions: parse_json_col::<Option<Dimensions>>(row.get::<Option<String>>(18)?),
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

    /// Merge an incremental delta into the cached catalog (N-7.4), in one
    /// transaction: `changes` are upserted by `(plugin_id, vendor_item_id)`
    /// and `deleted` ids are removed. Rows the delta does not mention keep
    /// their stored `fetched_at` — only genuinely changed rows get a new one.
    pub async fn merge_vendor_listings_cache(
        &self,
        plugin_id: &str,
        changes: &[VendorListingCache],
        deleted: &[String],
    ) -> Result<(), SyncError> {
        let conn = self.connect().await?;
        conn.execute("BEGIN IMMEDIATE", ()).await?;

        // Deduplicate by (plugin_id, vendor_item_id) — last occurrence wins,
        // mirroring save_vendor_listings_cache.
        let mut seen = std::collections::HashSet::new();
        let mut deduped: Vec<&VendorListingCache> = Vec::with_capacity(changes.len());
        for l in changes.iter().rev() {
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
                 (plugin_id, vendor_item_id, title, price, currency, quantity, sku, image_url, url, extras_json, group_key, variant_attrs_json, fetched_at,
                  images_json, upc, brand, mpn, weight_json, dimensions_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                         ?14, ?15, ?16, ?17, ?18, ?19)",
                params![
                    plugin_id,
                    l.vendor_item_id.clone(),
                    l.title.clone(),
                    decimal_param_opt(l.price.as_ref()),
                    l.currency.clone(),
                    l.quantity,
                    l.sku.clone(),
                    // Legacy column, kept NULL on new rows (N-7.2).
                    Option::<String>::None,
                    l.url.clone(),
                    extras_json,
                    l.group_key.clone(),
                    variant_attrs_json,
                    l.fetched_at.clone(),
                    json_param(&l.images),
                    l.upc.clone(),
                    l.brand.clone(),
                    l.mpn.clone(),
                    json_param_opt(l.weight.as_ref()),
                    json_param_opt(l.dimensions.as_ref()),
                ],
            )
            .await?;
        }

        for id in deleted {
            conn.execute(
                "DELETE FROM vendor_listings_cache WHERE plugin_id = ?1 AND vendor_item_id = ?2",
                params![plugin_id, id.as_str()],
            )
            .await?;
        }

        conn.execute("COMMIT", ()).await?;
        Ok(())
    }

    /// Incremental-sync state for a plugin: `(last_since, cursor)` as recorded
    /// by the last successful sync. `None` means the plugin has never synced
    /// (or the state was cleared), so the next run is a full pull.
    pub async fn get_vendor_sync_state(
        &self,
        plugin_id: &str,
    ) -> Result<Option<(String, Option<String>)>, SyncError> {
        let conn = self.connect().await?;
        let mut rows = conn
            .query(
                "SELECT last_since, cursor FROM vendor_sync_state WHERE plugin_id = ?1",
                params![plugin_id],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some((row.get::<String>(0)?, row.get::<Option<String>>(1)?)))
        } else {
            Ok(None)
        }
    }

    /// Record the host's incremental-sync state after a successful sync run:
    /// `last_since` is the run's start timestamp, `cursor` the cursor the
    /// plugin returned (`None` when it returned a full catalog or no cursor).
    pub async fn set_vendor_sync_state(
        &self,
        plugin_id: &str,
        last_since: &str,
        cursor: Option<&str>,
    ) -> Result<(), SyncError> {
        self.execute_write(
            "INSERT OR REPLACE INTO vendor_sync_state (plugin_id, last_since, cursor, updated_at)
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%d %H:%M:%S', 'now'))",
            params![plugin_id, last_since, cursor],
        )
        .await?;
        Ok(())
    }

    /// Clear a plugin's incremental-sync state. Called on uninstall so a
    /// re-install starts from a full pull, never a stale cursor.
    pub async fn clear_vendor_sync_state(&self, plugin_id: &str) -> Result<(), SyncError> {
        self.execute_write(
            "DELETE FROM vendor_sync_state WHERE plugin_id = ?1",
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
             (product_id, variant_id, plugin_id, vendor_item_id, title, price, currency, quantity, sku, image_url, url, extras_json, group_key, variant_attrs_json, fetched_at, updated_at,
              images_json, upc, brand, mpn, weight_json, dimensions_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, datetime('now'),
                     ?16, ?17, ?18, ?19, ?20, ?21)",
            params![
                product_id,
                variant_id,
                plugin_id,
                listing.vendor_item_id.clone(),
                listing.title.clone(),
                decimal_param_opt(listing.price.as_ref()),
                listing.currency.clone(),
                listing.quantity,
                listing.sku.clone(),
                // Legacy column, kept NULL on new rows (N-7.2).
                Option::<String>::None,
                listing.url.clone(),
                extras_json,
                listing.group_key.clone(),
                variant_attrs_json,
                listing.fetched_at.clone(),
                json_param(&listing.images),
                listing.upc.clone(),
                listing.brand.clone(),
                listing.mpn.clone(),
                json_param_opt(listing.weight.as_ref()),
                json_param_opt(listing.dimensions.as_ref()),
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
                        pvd.group_key, pvd.variant_attrs_json, pvd.fetched_at, pvd.variant_id,
                        pvd.images_json, pvd.upc, pvd.brand, pvd.mpn, pvd.weight_json,
                        pvd.dimensions_json
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
            let image_url: Option<String> = row.get::<Option<String>>(8)?;
            // N-7.2: pre-freeze rows only have `image_url`; fold it in.
            let mut images: Vec<String> = parse_json_col(row.get::<Option<String>>(15)?);
            if images.is_empty() {
                images = image_url.clone().into_iter().collect();
            }
            out.push((
                row.get::<String>(0)?,
                row.get::<Option<String>>(14)?,
                VendorListingCache {
                    plugin_id: row.get::<String>(1)?,
                    vendor_item_id: row.get::<String>(2)?,
                    title: row.get::<String>(3)?,
                    price: parse_decimal_opt(
                        "product_vendor_data.price",
                        row.get::<Option<String>>(4)?,
                    )?,
                    currency: row.get::<Option<String>>(5)?,
                    quantity: row.get::<Option<i64>>(6)?,
                    sku: row.get::<Option<String>>(7)?,
                    images,
                    image_url,
                    url: row.get::<Option<String>>(9)?,
                    upc: row.get::<Option<String>>(16)?,
                    brand: row.get::<Option<String>>(17)?,
                    mpn: row.get::<Option<String>>(18)?,
                    weight: parse_json_col::<Option<Weight>>(row.get::<Option<String>>(19)?),
                    dimensions: parse_json_col::<Option<Dimensions>>(
                        row.get::<Option<String>>(20)?,
                    ),
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
                    variant_attrs_json = ?10, fetched_at = ?11, updated_at = datetime('now'),
                    images_json = ?14, upc = ?15, brand = ?16, mpn = ?17,
                    weight_json = ?18, dimensions_json = ?19
                 WHERE plugin_id = ?12 AND vendor_item_id = ?13",
                to_params(params![
                    l.title.clone(),
                    decimal_param_opt(l.price.as_ref()),
                    l.currency.clone(),
                    l.quantity,
                    l.sku.clone(),
                    // Legacy column, kept NULL (N-7.2).
                    Option::<String>::None,
                    l.url.clone(),
                    extras_json,
                    l.group_key.clone(),
                    variant_attrs_json,
                    l.fetched_at.clone(),
                    plugin_id,
                    l.vendor_item_id.clone(),
                    json_param(&l.images),
                    l.upc.clone(),
                    l.brand.clone(),
                    l.mpn.clone(),
                    json_param_opt(l.weight.as_ref()),
                    json_param_opt(l.dimensions.as_ref()),
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
                    images: v.image_url.clone().into_iter().collect(),
                    image_url: v.image_url.clone(),
                    url: None,
                    upc: None,
                    brand: None,
                    mpn: None,
                    weight: None,
                    dimensions: None,
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

// ── N-8.1: automatic database backups ─────────────────────────────────────
// Backups land next to the database (`<db-dir>/backups/`) and never leave
// the machine. A backup is the database file plus its journal companions
// (`-wal`, `-shm`, `-log`) copied under a timestamped name, so it is a
// restorable set even when the WAL holds uncheckpointed pages.

/// What triggered a backup. Recorded in the backup file name so a human
/// browsing the backup directory can tell pre-migration snapshots from
/// scheduled ones at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupReason {
    /// Written by [`Db::migrate_with_backup`] before running migrations.
    PreMigration,
    /// Written by the scheduled backup task.
    Scheduled,
    /// Written by an explicit user action ("Back up now").
    Manual,
    /// Safety snapshot of the live database taken just before a restore.
    PreRestore,
}

impl BackupReason {
    pub fn as_str(self) -> &'static str {
        match self {
            BackupReason::PreMigration => "pre-migration",
            BackupReason::Scheduled => "scheduled",
            BackupReason::Manual => "manual",
            BackupReason::PreRestore => "pre-restore",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "pre-migration" => Some(BackupReason::PreMigration),
            "scheduled" => Some(BackupReason::Scheduled),
            "manual" => Some(BackupReason::Manual),
            "pre-restore" => Some(BackupReason::PreRestore),
            _ => None,
        }
    }
}

/// Policy controlling automatic database backups (N-8.1).
#[derive(Debug, Clone)]
pub struct BackupConfig {
    /// Master switch for *automatic* backups. When false,
    /// [`Db::migrate_with_backup`] skips the pre-migration backup and the
    /// scheduled task stays idle. An explicit [`Db::backup_now`] call (the
    /// UI's "Back up now", or the pre-restore safety snapshot) still works.
    pub enabled: bool,
    /// How many of the newest backups to keep. Pruning always keeps at
    /// least the newest one, even if this is 0.
    pub keep_last: usize,
    /// Seconds between scheduled backups. 0 disables the schedule
    /// (pre-migration backups still run while `enabled`).
    pub interval_secs: u64,
    /// Directory backups land in. `None` means `<db-dir>/backups`.
    pub dir: Option<PathBuf>,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_last: 7,
            interval_secs: 24 * 3600,
            dir: None,
        }
    }
}

impl BackupConfig {
    /// Build the core backup policy from the `[database]` TOML section.
    pub fn from_database_config(c: &crate::config::DatabaseConfig) -> Self {
        Self {
            enabled: c.backup_enabled,
            keep_last: c.backup_keep_last,
            interval_secs: c.backup_interval_hours.saturating_mul(3600),
            dir: None,
        }
    }
}

/// One timestamped backup file, as listed for the UI.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackupInfo {
    /// File name, e.g. `nisaba-20261021T091500.123Z-pre-migration.db`.
    pub name: String,
    /// Full path to the backup `.db` file.
    pub path: String,
    /// When the backup was written (RFC 3339, UTC).
    pub created_at: String,
    /// Size of the `.db` file in bytes (journal companions not included).
    pub size_bytes: u64,
    /// The [`BackupReason`] that produced it, or `"unknown"` for foreign files.
    pub reason: String,
}

/// Millisecond-precision UTC timestamp; file names sort chronologically.
const BACKUP_TS_FORMAT: &str = "%Y%m%dT%H%M%S%.3fZ";

/// File stem used for backup names: the database file's own stem (`nisaba`
/// for `nisaba.db`), so backups read
/// `nisaba-20261021T091500.123Z-pre-migration.db`.
fn db_file_stem(db_path: &Path) -> String {
    db_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("nisaba")
        .to_string()
}

/// The `.db` file plus its journal companions (`-wal`, `-shm`, `-log`).
fn backup_file_set(db: &Path) -> Vec<PathBuf> {
    let mut out = vec![db.to_path_buf()];
    let base = db.as_os_str().to_owned();
    for suffix in JOURNAL_SUFFIXES {
        let mut s = base.clone();
        s.push(suffix);
        out.push(PathBuf::from(s));
    }
    out
}

/// Copy a database file plus its existing journal companions to the
/// destination stem.
fn copy_file_set(src_db: &Path, dest_db: &Path) -> std::io::Result<()> {
    std::fs::copy(src_db, dest_db)?;
    let src_base = src_db.as_os_str().to_owned();
    let dest_base = dest_db.as_os_str().to_owned();
    for suffix in JOURNAL_SUFFIXES {
        let mut s = src_base.clone();
        s.push(suffix);
        let s = PathBuf::from(s);
        if s.exists() {
            let mut d = dest_base.clone();
            d.push(suffix);
            std::fs::copy(&s, PathBuf::from(d))?;
        }
    }
    Ok(())
}

/// Delete all but the newest `keep_last` backups of `stem` in `dir` (file
/// names sort chronologically). Journal companions of pruned backups are
/// deleted too. Always keeps at least the newest one.
fn prune_backup_dir(dir: &Path, stem: &str, keep_last: usize) -> Result<(), SyncError> {
    let keep = keep_last.max(1);
    let entries = std::fs::read_dir(dir).map_err(|e| {
        SyncError::DatabaseError(format!("cannot list backup dir {}: {e}", dir.display()))
    })?;
    let prefix = format!("{stem}-");
    let mut dbs: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .filter_map(|n| n.into_string().ok())
        .filter(|n| n.starts_with(&prefix) && n.ends_with(".db"))
        .collect();
    dbs.sort();
    if dbs.len() <= keep {
        return Ok(());
    }
    for name in dbs.iter().take(dbs.len() - keep) {
        for p in backup_file_set(&dir.join(name)) {
            if p.exists() {
                std::fs::remove_file(&p).map_err(|e| {
                    SyncError::DatabaseError(format!(
                        "cannot prune old backup {}: {e}",
                        p.display()
                    ))
                })?;
            }
        }
        debug!(backup = %name, "Pruned old database backup");
    }
    Ok(())
}

/// Copy the database file plus any journal files into `dir` under a
/// timestamped name, then prune to `keep_last`. Returns the `.db` path.
fn write_backup_files(
    src: &Path,
    dir: &Path,
    reason: &str,
    keep_last: usize,
) -> Result<PathBuf, SyncError> {
    std::fs::create_dir_all(dir).map_err(|e| {
        SyncError::DatabaseError(format!("cannot create backup dir {}: {e}", dir.display()))
    })?;
    let stem = db_file_stem(src);
    let ts = chrono::Utc::now().format(BACKUP_TS_FORMAT).to_string();
    // Same-millisecond collisions (tests, rapid retries) get a numeric suffix.
    let mut n = 0u32;
    let dest_db = loop {
        let name = if n == 0 {
            format!("{stem}-{ts}-{reason}.db")
        } else {
            format!("{stem}-{ts}-{reason}-{n}.db")
        };
        let candidate = dir.join(&name);
        if !candidate.exists() {
            break candidate;
        }
        n += 1;
    };
    copy_file_set(src, &dest_db).map_err(|e| {
        SyncError::DatabaseError(format!(
            "backup copy {} -> {}: {e}",
            src.display(),
            dest_db.display()
        ))
    })?;
    prune_backup_dir(dir, &stem, keep_last)?;
    info!(backup = %dest_db.display(), "Wrote database backup");
    Ok(dest_db)
}

/// Recover the [`BackupReason`] recorded in a backup file name
/// (`<stem>-<ts>Z-<reason>[-<n>].db`); `"unknown"` for anything else.
fn backup_reason_from_name(name: &str, stem: &str) -> String {
    let rest = match name.strip_prefix(&format!("{stem}-")) {
        Some(r) => r,
        None => return "unknown".to_string(),
    };
    let rest = rest.strip_suffix(".db").unwrap_or(rest);
    // The timestamp ends at the last 'Z'; the reason follows it.
    let tail = match rest.rsplit_once('Z') {
        Some((_, t)) => t,
        None => return "unknown".to_string(),
    };
    let reason = tail.strip_prefix('-').unwrap_or(tail);
    // Drop a numeric disambiguator suffix ("manual-2" -> "manual").
    let reason = match reason.rsplit_once('-') {
        Some((r, n)) if n.chars().all(|c| c.is_ascii_digit()) => r,
        _ => reason,
    };
    if BackupReason::from_str(reason).is_some() {
        reason.to_string()
    } else {
        "unknown".to_string()
    }
}

fn read_backup_list(dir: &Path, stem: &str) -> Result<Vec<BackupInfo>, SyncError> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => {
            return Err(SyncError::DatabaseError(format!(
                "cannot list backup dir {}: {e}",
                dir.display()
            )))
        }
    };
    let prefix = format!("{stem}-");
    for entry in entries.filter_map(|e| e.ok()) {
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if !(name.starts_with(&prefix) && name.ends_with(".db")) {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let created_at = meta
            .modified()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
            .unwrap_or_default();
        out.push(BackupInfo {
            reason: backup_reason_from_name(&name, stem),
            path: dir.join(&name).display().to_string(),
            name,
            created_at,
            size_bytes: meta.len(),
        });
    }
    // File names sort chronologically; newest first.
    out.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(out)
}

/// Copy the backup file set over the live database paths. Journal
/// companions the backup does not have are deleted from the live side, so a
/// stale WAL can never shadow the restored file.
fn swap_in_backup(live_db: &Path, backup_db: &Path) -> Result<(), SyncError> {
    copy_file_set(backup_db, live_db).map_err(|e| {
        SyncError::DatabaseError(format!(
            "restore copy {} -> {}: {e}",
            backup_db.display(),
            live_db.display()
        ))
    })?;
    let live_base = live_db.as_os_str().to_owned();
    let backup_base = backup_db.as_os_str().to_owned();
    for suffix in JOURNAL_SUFFIXES {
        let mut b = backup_base.clone();
        b.push(suffix);
        if Path::new(&b).exists() {
            continue;
        }
        let mut l = live_base.clone();
        l.push(suffix);
        let l = PathBuf::from(l);
        if l.exists() {
            std::fs::remove_file(&l).map_err(|e| {
                SyncError::DatabaseError(format!(
                    "cannot remove stale journal {}: {e}",
                    l.display()
                ))
            })?;
        }
    }
    Ok(())
}

impl Db {
    /// Directory this database's backups land in: `<db-dir>/backups` unless
    /// the config overrides it. Created on demand by [`Db::backup_now`].
    pub fn backup_dir(&self, cfg: &BackupConfig) -> PathBuf {
        if let Some(dir) = &cfg.dir {
            return dir.clone();
        }
        match Path::new(&self.path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            Some(parent) => parent.join("backups"),
            None => PathBuf::from("backups"),
        }
    }

    /// Write a timestamped backup of this database into the backup directory
    /// and prune older backups to `keep_last`. Returns the backup `.db` path.
    ///
    /// A best-effort `wal_checkpoint(TRUNCATE)` runs first so the main file
    /// holds as much as possible even where the engine ignores the pragma;
    /// the journal files are copied alongside the main file anyway, so the
    /// backup stays a restorable set. This is the explicit entry point (the
    /// UI's "Back up now", the scheduler, the pre-restore safety snapshot):
    /// it writes regardless of [`BackupConfig::enabled`]; the automatic
    /// triggers check the flag themselves.
    pub async fn backup_now(
        &self,
        cfg: &BackupConfig,
        reason: BackupReason,
    ) -> Result<PathBuf, SyncError> {
        if let Ok(conn) = self.connect().await {
            let _ = conn.execute("PRAGMA wal_checkpoint(TRUNCATE)", ()).await;
        }
        let src = PathBuf::from(&self.path);
        let dir = self.backup_dir(cfg);
        let keep_last = cfg.keep_last.max(1);
        let reason_str = reason.as_str().to_string();
        tokio::task::spawn_blocking(move || write_backup_files(&src, &dir, &reason_str, keep_last))
            .await
            .map_err(|e| SyncError::DatabaseError(format!("backup task failed: {e}")))?
    }

    /// List this database's backups, newest first. A missing backup
    /// directory lists as empty; stray files never fail the listing.
    pub async fn list_backups(&self, cfg: &BackupConfig) -> Result<Vec<BackupInfo>, SyncError> {
        let dir = self.backup_dir(cfg);
        let stem = db_file_stem(Path::new(&self.path));
        tokio::task::spawn_blocking(move || read_backup_list(&dir, &stem))
            .await
            .map_err(|e| SyncError::DatabaseError(format!("listing backups failed: {e}")))?
    }

    /// Replace the live database file with a backup.
    ///
    /// The backup must live inside this database's backup directory (path
    /// traversal is rejected) and be a `.db` file. A `pre-restore` backup of
    /// the current file is taken first, so a bad restore is itself
    /// recoverable. Journal files beside the live database are synced to the
    /// backup's set, so a stale WAL can never shadow the restored file.
    ///
    /// The process MUST restart afterwards: turso keeps the old file's pages
    /// cached in already-open connections. The UI restarts the app once this
    /// returns `Ok`.
    pub async fn restore_backup(&self, cfg: &BackupConfig, backup: &Path) -> Result<(), SyncError> {
        let backup_canon = backup
            .canonicalize()
            .map_err(|e| SyncError::DatabaseError(format!("backup {}: {e}", backup.display())))?;
        let dir = self.backup_dir(cfg);
        let dir_canon = dir.canonicalize().map_err(|_| {
            SyncError::DatabaseError("no backups have been written yet".to_string())
        })?;
        if !backup_canon.starts_with(&dir_canon) {
            return Err(SyncError::DatabaseError(format!(
                "refusing to restore {}: not inside the backup directory {}",
                backup.display(),
                dir.display()
            )));
        }
        if backup_canon.extension().and_then(|e| e.to_str()) != Some("db") {
            return Err(SyncError::DatabaseError(format!(
                "refusing to restore {}: not a .db backup file",
                backup.display()
            )));
        }
        // Safety net first: snapshot the live database before touching it.
        let safety = self
            .backup_now(cfg, BackupReason::PreRestore)
            .await
            .map_err(|e| {
                SyncError::DatabaseError(format!("pre-restore safety backup failed: {e}"))
            })?;
        info!(safety = %safety.display(), "Wrote pre-restore safety backup");

        let live = PathBuf::from(&self.path);
        tokio::task::spawn_blocking(move || swap_in_backup(&live, &backup_canon))
            .await
            .map_err(|e| SyncError::DatabaseError(format!("restore task failed: {e}")))?
    }
}

/// Config keys declared with `secret: true` in a plugin's `config_fields` JSON.
///
/// `config_fields` is the serialised `Vec<ConfigField>` from the plugin's
/// metadata; only the `key` and `secret` members matter here. Returns an empty
/// set when the plugin declares no fields.
pub fn secret_config_keys(config_fields_json: Option<&str>) -> HashSet<String> {
    let mut keys = HashSet::new();
    if let Some(json) = config_fields_json {
        if let Ok(fields) = serde_json::from_str::<Vec<serde_json::Value>>(json) {
            for field in fields {
                let is_secret = field
                    .get("secret")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if is_secret {
                    if let Some(key) = field.get("key").and_then(serde_json::Value::as_str) {
                        keys.insert(key.to_string());
                    }
                }
            }
        }
    }
    keys
}

/// Remove `secret: true` values from a plugin `config_json` string.
///
/// Used wherever config crosses a boundary that must never carry secrets:
/// the P2P sync payload (both directions) and the on-disk scrub performed by
/// [`Db::migrate_plugin_secrets_to_keyring`]. Unparseable input is returned
/// unchanged — never silently dropped.
pub fn strip_secret_values(
    config_fields_json: Option<&str>,
    config_json: Option<&str>,
) -> Option<String> {
    let secret_keys = secret_config_keys(config_fields_json);
    if secret_keys.is_empty() {
        return config_json.map(str::to_string);
    }
    let json: String = config_json?.to_string();
    let mut map: HashMap<String, String> = match serde_json::from_str(&json) {
        Ok(map) => map,
        Err(_) => return Some(json),
    };
    let before = map.len();
    map.retain(|k, _| !secret_keys.contains(k));
    if map.len() == before {
        return Some(json);
    }
    serde_json::to_string(&map).ok().or(Some(json))
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
        allowed_hosts_json: row.get::<Option<String>>(16)?,
        code_hash: row.get::<Option<String>>(17)?,
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

fn parse_stock_event(row: &turso::Row) -> Result<StockEvent, SyncError> {
    let kind_str: String = row.get(4)?;
    let kind = StockEventKind::from_str_loose(&kind_str).ok_or_else(|| {
        SyncError::DatabaseError(format!("unknown stock event kind `{kind_str}`"))
    })?;
    Ok(StockEvent {
        id: row.get::<i64>(0)?,
        event_key: row.get::<String>(1)?,
        product_id: row.get::<String>(2)?,
        variant_id: row.get::<String>(3)?,
        kind,
        delta: row.get::<i64>(5)?,
        platform: row.get::<Option<String>>(6)?,
        source_order_id: row.get::<Option<String>>(7)?,
        note: row.get::<Option<String>>(8)?,
        created_at: row.get::<String>(9)?,
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
    use std::str::FromStr;

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

    /// Every table the app reads or writes after a migration from empty. A
    /// migration step that silently fails leaves the DB one table short, and
    /// the first symptom today is a runtime error deep in a sync cycle.
    const EXPECTED_TABLES: &[&str] = &[
        "app_settings",
        "auth_tokens",
        "cached_listings",
        "cached_platform_listings",
        "company",
        "company_config",
        "company_peers",
        "company_registry",
        "inventory_history",
        "listing_descriptions",
        "listing_photos",
        "platform_mappings",
        "platform_snapshots",
        "plugin_kv_store",
        "pricing_snapshots",
        "products",
        "product_variants",
        "product_vendor_data",
        "stock_ledger",
        "sync_events",
        "vendor_listings_cache",
        "vendor_plugin_installs",
        "vendor_plugin_registry",
        "xmr_processed_orders",
    ];

    async fn table_names(db: &Db) -> Vec<String> {
        let conn = db.connect().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
                (),
            )
            .await
            .unwrap();
        let mut names = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            names.push(row.get::<String>(0).unwrap());
        }
        names
    }

    async fn column_names(db: &Db, table: &str) -> Vec<String> {
        let conn = db.connect().await.unwrap();
        let mut rows = conn
            .query(&format!("PRAGMA table_info({table})"), ())
            .await
            .unwrap();
        let mut names = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            names.push(row.get::<String>(1).unwrap());
        }
        names
    }

    #[tokio::test]
    async fn migrating_an_empty_db_builds_the_whole_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();

        let tables = table_names(&db).await;
        for expected in EXPECTED_TABLES {
            assert!(
                tables.iter().any(|t| t == expected),
                "table `{expected}` missing after migrating from empty; got {tables:?}"
            );
        }
    }

    #[tokio::test]
    async fn migration_008_restores_the_columns_migration_007_dropped() {
        // Migration 007 rebuilds `platform_mappings`, which loses the columns
        // migration 002 added. 008 is the inline repair — this is the assertion
        // that keeps the pair honest, since neither step has a test of its own.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repair.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();

        let columns = column_names(&db, "platform_mappings").await;
        for expected in ["deleted_at", "updated_at", "variant_id", "is_active"] {
            assert!(
                columns.iter().any(|c| c == expected),
                "platform_mappings is missing `{expected}`; got {columns:?}"
            );
        }
    }

    fn registry_row(id: &str, allowed_hosts_json: Option<&str>) -> VendorPluginRow {
        VendorPluginRow {
            id: id.to_string(),
            plugin_file: "plugin.zip".to_string(),
            display_name: format!("Plugin {id}"),
            description: String::new(),
            files_json: r#"{"index.ts":""}"#.to_string(),
            version: "1.0.0".to_string(),
            config_fields: None,
            config_json: None,
            status: "approved".to_string(),
            submitted_by: None,
            approved_by: None,
            category: "vendor".to_string(),
            icon: None,
            include_vendor_stock: false,
            created_at: "2026-09-25 00:00:00".to_string(),
            updated_at: "2026-09-25 00:00:00".to_string(),
            allowed_hosts_json: allowed_hosts_json.map(str::to_string),
            code_hash: None,
        }
    }

    #[tokio::test]
    async fn review_binds_the_approval_to_the_current_code_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();

        db.upsert_registry_plugin(&registry_row("plug", Some(r#"["api.example.com"]"#)))
            .await
            .unwrap();

        db.review_registry_plugin("plug", "approved", Some("onion-x"))
            .await
            .unwrap();
        let row = db.get_registry_plugin("plug").await.unwrap().unwrap();
        assert_eq!(row.status, "approved");
        assert_eq!(row.approved_by.as_deref(), Some("onion-x"));
        assert_eq!(
            row.code_hash,
            crate::plugins::plugin_code_fingerprint(&row.files_json, &row.allowed_hosts_json)
        );
        assert!(row.code_hash.is_some());

        // A rejection is bound the same way, so a changed re-submission
        // comes back as pending rather than inheriting the rejection.
        db.review_registry_plugin("plug", "rejected", Some("onion-x"))
            .await
            .unwrap();
        let row = db.get_registry_plugin("plug").await.unwrap().unwrap();
        assert_eq!(row.status, "rejected");
        assert!(row.code_hash.is_some());
    }

    #[tokio::test]
    async fn registry_plugins_keep_their_cached_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();
        assert!(column_names(&db, "vendor_plugin_registry")
            .await
            .iter()
            .any(|c| c == "allowed_hosts"));

        // The three states: a declared list, declared-none (JSON null), not computed.
        db.upsert_registry_plugin(&registry_row("listed", Some(r#"["www.rothco.com"]"#)))
            .await
            .unwrap();
        db.upsert_registry_plugin(&registry_row("open", Some("null")))
            .await
            .unwrap();
        db.upsert_registry_plugin(&registry_row("unknown", None))
            .await
            .unwrap();

        async fn cached(db: &Db, id: &str) -> Option<String> {
            db.get_registry_plugin(id)
                .await
                .unwrap()
                .unwrap()
                .allowed_hosts_json
        }
        assert_eq!(
            cached(&db, "listed").await.as_deref(),
            Some(r#"["www.rothco.com"]"#)
        );
        assert_eq!(cached(&db, "open").await.as_deref(), Some("null"));
        assert_eq!(cached(&db, "unknown").await, None);

        // An upsert carrying no allowlist (a row from a peer) clears the cached one, and
        // the install path's recomputation sets it again.
        db.upsert_registry_plugin(&registry_row("listed", None))
            .await
            .unwrap();
        assert_eq!(cached(&db, "listed").await, None);
        db.set_registry_plugin_allowed_hosts("listed", Some(r#"["a.example.com"]"#))
            .await
            .unwrap();
        assert_eq!(
            cached(&db, "listed").await.as_deref(),
            Some(r#"["a.example.com"]"#)
        );
        let listed = db
            .list_registry_plugins()
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.id == "listed")
            .unwrap();
        assert_eq!(
            listed.allowed_hosts_json.as_deref(),
            Some(r#"["a.example.com"]"#)
        );
    }

    #[tokio::test]
    async fn plugin_installs_keep_a_per_machine_time_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installs.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();

        async fn timeout(db: &Db) -> Option<i64> {
            db.list_installed_plugins().await.unwrap()[0].timeout_minutes
        }

        db.install_plugin("p").await.unwrap();
        assert_eq!(timeout(&db).await, None);

        db.set_plugin_timeout("p", Some(120)).await.unwrap();
        assert_eq!(timeout(&db).await, Some(120));

        // Reinstalling (e.g. after an update) keeps the user's choice.
        db.install_plugin("p").await.unwrap();
        assert_eq!(timeout(&db).await, Some(120));

        db.set_plugin_timeout("p", None).await.unwrap();
        assert_eq!(timeout(&db).await, None);
    }

    #[tokio::test]
    async fn migrating_twice_is_a_no_op() {
        // `migrate()` runs on every launch, so a step that is not gated on the
        // version — or not idempotent — corrupts the schema on the second run.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("twice.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();

        db.migrate().await.unwrap();
        let first = table_names(&db).await;

        db.migrate().await.unwrap();
        let second = table_names(&db).await;

        assert_eq!(first, second, "a second migrate() changed the schema");

        let conn = db.connect().await.unwrap();
        let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
        let version: i64 = rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    /// Open a database and apply migration steps `1..=target` one at a time,
    /// exactly as `migrate()` would have on an older release.
    async fn db_at_schema_version(dir: &tempfile::TempDir, name: &str, target: i64) -> Db {
        let path = dir.path().join(name);
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        for step in 1..=target {
            db.migrate_step(step).await.unwrap();
        }
        assert_eq!(db.schema_version().await.unwrap(), target);
        db
    }

    async fn user_version_of(db: &Db) -> i64 {
        let conn = db.connect().await.unwrap();
        let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    }

    async fn table_columns(conn: &turso::Connection, table: &str) -> Vec<String> {
        let mut rows = conn
            .query(&format!("PRAGMA table_info({table})"), ())
            .await
            .unwrap();
        let mut cols = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            cols.push(row.get::<String>(1).unwrap());
        }
        cols
    }

    async fn table_ddl(conn: &turso::Connection, table: &str) -> String {
        let mut rows = conn
            .query(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                turso::params![table],
            )
            .await
            .unwrap();
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(0)
            .unwrap()
    }

    async fn index_names(conn: &turso::Connection, table: &str) -> Vec<String> {
        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = ?1",
                turso::params![table],
            )
            .await
            .unwrap();
        let mut names = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            names.push(row.get::<String>(0).unwrap());
        }
        names
    }

    async fn index_sql(conn: &turso::Connection, index: &str) -> String {
        let mut rows = conn
            .query(
                "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1",
                turso::params![index],
            )
            .await
            .unwrap();
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>(0)
            .unwrap()
    }

    #[tokio::test]
    async fn user_version_pragma_rolls_back_with_its_transaction() {
        // migrate_step() bumps user_version inside the step's transaction. This
        // locks in the SQLite behavior that makes that atomic: the pragma is
        // rolled back with the transaction, so a crash mid-step can never
        // leave the version bumped while the schema change rolled back.
        let dir = tempfile::tempdir().unwrap();
        let db = db_at_schema_version(&dir, "pragma-tx.db", 1).await;

        let conn = db.connect().await.unwrap();
        conn.execute("BEGIN IMMEDIATE", ()).await.unwrap();
        conn.execute("PRAGMA user_version = 42", ()).await.unwrap();
        conn.execute("ROLLBACK", ()).await.unwrap();
        assert_eq!(user_version_of(&db).await, 1);

        conn.execute("BEGIN IMMEDIATE", ()).await.unwrap();
        conn.execute("PRAGMA user_version = 42", ()).await.unwrap();
        conn.execute("COMMIT", ()).await.unwrap();
        assert_eq!(user_version_of(&db).await, 42);
    }

    #[tokio::test]
    async fn step_7_rebuild_preserves_populated_mappings() {
        // Upgrade from a populated v6 database through the DROP/RENAME table
        // rebuild in step 7. The step must bump the version by itself, keep
        // every row, and swap the unique indexes.
        let dir = tempfile::tempdir().unwrap();
        let db = db_at_schema_version(&dir, "v6.db", 6).await;

        let conn = db.connect().await.unwrap();
        conn.execute(
            "INSERT INTO products (id, canonical_sku, name, quantity)
             VALUES ('p1', 'SKU-1', 'Widget', 10)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO platform_mappings
                 (id, product_id, platform, platform_item_id, platform_sku, is_active, deleted_at, updated_at)
             VALUES (7, 'p1', 'ebay', 'EBAY-1', 'SKU-1', 1, NULL, '2026-10-01')",
            (),
        )
        .await
        .unwrap();
        drop(conn);

        db.migrate_step(7).await.unwrap();
        assert_eq!(user_version_of(&db).await, 7);

        let conn = db.connect().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT product_id, platform, platform_item_id, platform_sku, deleted_at, updated_at, variant_id
                 FROM platform_mappings WHERE id = 7",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "p1");
        assert_eq!(row.get::<String>(1).unwrap(), "ebay");
        assert_eq!(row.get::<String>(2).unwrap(), "EBAY-1");
        assert_eq!(row.get::<String>(3).unwrap(), "SKU-1");
        assert!(row.get::<Option<String>>(4).unwrap().is_none());
        assert_eq!(row.get::<String>(5).unwrap(), "2026-10-01");
        assert!(row.get::<Option<String>>(6).unwrap().is_none());

        // The rebuild added variant_id and swapped the unique indexes.
        assert!(table_columns(&conn, "platform_mappings")
            .await
            .contains(&"variant_id".to_string()));
        let indexes = index_names(&conn, "platform_mappings").await;
        assert!(indexes.contains(&"idx_mappings_product_platform_variant".to_string()));
        assert!(indexes.contains(&"idx_mappings_platform_item".to_string()));
        assert!(!indexes.contains(&"idx_mappings_product_platform".to_string()));
        drop(conn);

        // The remaining steps complete the upgrade from the stepped version.
        db.migrate().await.unwrap();
        assert_eq!(user_version_of(&db).await, CURRENT_SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn step_15_rebuild_drops_autoincrement_and_keeps_data() {
        // Upgrade from a populated v14 database through the step-15 rebuild of
        // platform_mappings (AUTOINCREMENT is unsupported in MVCC mode).
        let dir = tempfile::tempdir().unwrap();
        let db = db_at_schema_version(&dir, "v14.db", 14).await;

        let conn = db.connect().await.unwrap();
        assert!(
            table_ddl(&conn, "platform_mappings")
                .await
                .contains("AUTOINCREMENT"),
            "v14 platform_mappings should still use AUTOINCREMENT"
        );
        conn.execute(
            "INSERT INTO products (id, canonical_sku, name, quantity)
             VALUES ('p2', 'SKU-2', 'Gadget', 3)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO platform_mappings (product_id, platform, platform_item_id, is_active, variant_id)
             VALUES ('p2', 'etsy', 'ETSY-9', 1, 'var-1')",
            (),
        )
        .await
        .unwrap();
        drop(conn);

        db.migrate_step(15).await.unwrap();
        assert_eq!(user_version_of(&db).await, 15);

        let conn = db.connect().await.unwrap();
        assert!(
            !table_ddl(&conn, "platform_mappings")
                .await
                .contains("AUTOINCREMENT"),
            "step 15 must remove AUTOINCREMENT"
        );
        let mut rows = conn
            .query(
                "SELECT product_id, platform, platform_item_id, variant_id FROM platform_mappings",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "p2");
        assert_eq!(row.get::<String>(3).unwrap(), "var-1");
        assert!(rows.next().await.unwrap().is_none());
        drop(conn);

        db.migrate().await.unwrap();
        assert_eq!(user_version_of(&db).await, CURRENT_SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn step_16_rebuild_preserves_every_table() {
        // Upgrade from a populated v15 database through the step-16 rebuild of
        // all remaining AUTOINCREMENT tables. Every row in every rebuilt table
        // must survive.
        let dir = tempfile::tempdir().unwrap();
        let db = db_at_schema_version(&dir, "v15.db", 15).await;

        let conn = db.connect().await.unwrap();
        for table in [
            "platform_snapshots",
            "sync_events",
            "inventory_history",
            "listing_descriptions",
            "listing_photos",
            "pricing_snapshots",
            "vendor_listings_cache",
        ] {
            assert!(
                table_ddl(&conn, table).await.contains("AUTOINCREMENT"),
                "v15 {table} should still use AUTOINCREMENT"
            );
        }
        conn.execute(
            "INSERT INTO products (id, canonical_sku, name, quantity)
             VALUES ('p3', 'SKU-3', 'Thing', 1)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO platform_snapshots (product_id, platform, last_known_quantity, last_polled_at)
             VALUES ('p3', 'ebay', 5, '2026-10-01')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO sync_events (product_id, source_platform, target_platform, old_quantity, new_quantity, event_type, sync_cycle_id)
             VALUES ('p3', 'ebay', 'etsy', 5, 4, 'quantity', 'cyc-1')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO inventory_history (product_id, quantity, sync_cycle_id)
             VALUES ('p3', 4, 'cyc-1')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO listing_descriptions (product_id, platform, platform_item_id, description_text)
             VALUES ('p3', 'ebay', 'EBAY-1', 'hello')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO listing_photos (product_id, platform, platform_item_id, url)
             VALUES ('p3', 'ebay', 'EBAY-1', 'https://x/y.png')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO pricing_snapshots (product_id, platform, amount)
             VALUES ('p3', 'ebay', 9.99)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO vendor_listings_cache (plugin_id, vendor_item_id, title)
             VALUES ('plug', 'V-1', 'Vendor thing')",
            (),
        )
        .await
        .unwrap();
        drop(conn);

        db.migrate_step(16).await.unwrap();
        assert_eq!(user_version_of(&db).await, 16);

        let conn = db.connect().await.unwrap();
        for table in [
            "platform_snapshots",
            "sync_events",
            "inventory_history",
            "listing_descriptions",
            "listing_photos",
            "pricing_snapshots",
            "vendor_listings_cache",
        ] {
            assert!(
                !table_ddl(&conn, table).await.contains("AUTOINCREMENT"),
                "{table} still uses AUTOINCREMENT after step 16"
            );
            let mut rows = conn
                .query(&format!("SELECT COUNT(*) FROM {table}"), ())
                .await
                .unwrap();
            let n: i64 = rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap();
            assert_eq!(n, 1, "{table} lost its row in the step-16 rebuild");
        }
        drop(conn);

        db.migrate().await.unwrap();
        assert_eq!(user_version_of(&db).await, CURRENT_SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn step_23_makes_the_mapping_unique_indexes_partial() {
        // Upgrade from a populated v22 database: one live mapping and one
        // soft-deleted tombstone. Step 23 must keep both rows, swap both
        // unique indexes for partial ones, and afterwards let the
        // tombstone's keys be re-mapped.
        let dir = tempfile::tempdir().unwrap();
        let db = db_at_schema_version(&dir, "v22.db", 22).await;

        let conn = db.connect().await.unwrap();
        for index in [
            "idx_mappings_product_platform_variant",
            "idx_mappings_platform_item",
        ] {
            assert!(
                !index_sql(&conn, index).await.contains("WHERE"),
                "v22 {index} should not be partial yet"
            );
        }
        conn.execute(
            "INSERT INTO products (id, canonical_sku, name, quantity)
             VALUES ('p1', 'SKU-1', 'Widget', 10)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO platform_mappings
                 (id, product_id, platform, platform_item_id, platform_sku, is_active, deleted_at, updated_at, variant_id)
             VALUES (1, 'p1', 'ebay', 'EBAY-LIVE', 'SKU-1', 1, NULL, '2026-10-01', 'v1'),
                    (2, 'p1', 'etsy', 'ETSY-GONE', 'SKU-1', 1, '2026-10-02', '2026-10-02', 'v1')",
            (),
        )
        .await
        .unwrap();
        drop(conn);

        db.migrate_step(23).await.unwrap();
        assert_eq!(user_version_of(&db).await, 23);

        let conn = db.connect().await.unwrap();
        for index in [
            "idx_mappings_product_platform_variant",
            "idx_mappings_platform_item",
        ] {
            let sql = index_sql(&conn, index).await;
            assert!(
                sql.contains("WHERE deleted_at IS NULL"),
                "step 23 must make {index} partial; got: {sql}"
            );
        }
        let mut rows = conn
            .query("SELECT COUNT(*) FROM platform_mappings", ())
            .await
            .unwrap();
        let n: i64 = rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap();
        assert_eq!(n, 2, "step 23 must preserve live rows and tombstones");
        drop(conn);

        // The tombstone no longer blocks re-mapping its keys…
        db.insert_mapping("p1", "etsy", "ETSY-GONE", Some("SKU-1"), "v1")
            .await
            .unwrap();
        // …but a second live mapping for the same keys still conflicts.
        assert!(
            db.insert_mapping("p1", "etsy", "ETSY-GONE", Some("SKU-1"), "v1")
                .await
                .is_err(),
            "the partial index must still reject duplicate live mappings"
        );

        db.migrate().await.unwrap();
        assert_eq!(user_version_of(&db).await, CURRENT_SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn failed_step_rolls_back_and_stays_rerunnable() {
        // Simulates a crash between DROP and RENAME in a table rebuild: the
        // step body runs inside a transaction, an error aborts it before
        // COMMIT, and the database must stay at the previous version with its
        // data intact — so the next launch can simply re-run the step.
        let dir = tempfile::tempdir().unwrap();
        let db = db_at_schema_version(&dir, "crash.db", 6).await;

        let conn = db.connect().await.unwrap();
        conn.execute(
            "INSERT INTO products (id, canonical_sku, name, quantity)
             VALUES ('p4', 'SKU-4', 'Doohickey', 2)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO platform_mappings (product_id, platform, platform_item_id)
             VALUES ('p4', 'ebay', 'EBAY-4')",
            (),
        )
        .await
        .unwrap();

        conn.execute("BEGIN IMMEDIATE", ()).await.unwrap();
        db.run_migration_step(&conn, 7).await.unwrap();
        // The crash: a statement error aborts the transaction before COMMIT.
        conn.execute("THIS IS NOT VALID SQL AND SIMULATES A CRASH", ())
            .await
            .unwrap_err();
        conn.execute("ROLLBACK", ()).await.unwrap();

        assert_eq!(
            user_version_of(&db).await,
            6,
            "a failed step must not bump user_version"
        );
        let mut rows = conn
            .query("SELECT COUNT(*) FROM platform_mappings", ())
            .await
            .unwrap();
        let n: i64 = rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap();
        assert_eq!(n, 1, "rolled-back rebuild must not lose rows");
        assert!(
            !table_columns(&conn, "platform_mappings")
                .await
                .contains(&"variant_id".to_string()),
            "rolled-back rebuild must not leave the new column behind"
        );
        drop(conn);

        // The next launch re-runs the step and completes the upgrade.
        db.migrate().await.unwrap();
        assert_eq!(user_version_of(&db).await, CURRENT_SCHEMA_VERSION);
        let conn = db.connect().await.unwrap();
        let mut rows = conn
            .query("SELECT COUNT(*) FROM platform_mappings", ())
            .await
            .unwrap();
        let n: i64 = rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap();
        assert_eq!(n, 1);
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

    async fn count_rows(db: &Db, table: &str, column: &str, value: &str) -> i64 {
        let conn = db.connect().await.unwrap();
        let mut rows = conn
            .query(
                &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
                params![value],
            )
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    }

    fn deletable_product(id: &str) -> Product {
        Product {
            id: id.to_string(),
            canonical_sku: format!("SKU-{id}"),
            name: format!("Product {id}"),
            quantity: 3,
            low_stock_threshold: None,
            is_tracked: true,
            has_variants: true,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn deletable_variant(id: &str, product_id: &str) -> ProductVariant {
        ProductVariant {
            id: id.to_string(),
            product_id: product_id.to_string(),
            sku: format!("SKU-{id}"),
            name: format!("Variant {id}"),
            attributes: std::collections::HashMap::new(),
            quantity: 3,
            on_hand_quantity: 3,
            image_url: None,
            sort_order: 0,
            source_plugin_id: Some("plug".to_string()),
            source_vendor_item_id: Some("VI-1".to_string()),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[tokio::test]
    async fn deleting_a_product_cleans_up_its_dependent_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("test.db").to_str().unwrap())
            .await
            .unwrap();
        db.migrate().await.unwrap();

        db.insert_product(&deletable_product("p-del"))
            .await
            .unwrap();
        db.insert_variant(&deletable_variant("v-del", "p-del"))
            .await
            .unwrap();
        db.insert_mapping("p-del", "ebay", "ITEM-1", Some("SKU-v-del"), "v-del")
            .await
            .unwrap();
        db.upsert_snapshot("p-del", "ebay", None, 3, "2026-10-20 00:00:00")
            .await
            .unwrap();
        db.record_inventory_history("p-del", 3, "cycle-1")
            .await
            .unwrap();
        db.upsert_listing_description("p-del", "ebay", "ITEM-1", Some("<p>x</p>"), Some("x"))
            .await
            .unwrap();
        db.save_listing_photo("p-del", "ebay", "ITEM-1", "https://img/1.jpg", 0)
            .await
            .unwrap();
        db.record_pricing_snapshot("p-del", "ebay", Decimal::new(999, 2), "USD")
            .await
            .unwrap();
        db.save_cached_listing("p-del", "ebay", "ITEM-1", "{}")
            .await
            .unwrap();
        // A vendor-data row for the product's variant (variant_id set).
        db.connect()
            .await
            .unwrap()
            .execute(
                "INSERT INTO product_vendor_data (product_id, variant_id, plugin_id, vendor_item_id, title)
                 VALUES ('p-del', 'v-del', 'plug', 'VI-1', 'Vendor')",
                (),
            )
            .await
            .unwrap();

        // A second product whose rows must survive the delete of the first.
        db.insert_product(&deletable_product("p-keep"))
            .await
            .unwrap();
        db.upsert_listing_description("p-keep", "ebay", "ITEM-9", Some("<p>k</p>"), Some("k"))
            .await
            .unwrap();
        db.save_cached_listing("p-keep", "ebay", "ITEM-9", "{}")
            .await
            .unwrap();

        for (table, column) in [
            ("platform_mappings", "product_id"),
            ("platform_snapshots", "product_id"),
            ("inventory_history", "product_id"),
            ("listing_descriptions", "product_id"),
            ("listing_photos", "product_id"),
            ("pricing_snapshots", "product_id"),
            ("cached_listings", "product_id"),
            ("product_vendor_data", "product_id"),
            ("product_variants", "product_id"),
        ] {
            assert_eq!(
                count_rows(&db, table, column, "p-del").await,
                1,
                "{table} should hold the product's row before the delete"
            );
        }

        db.delete_product("p-del").await.unwrap();

        for (table, column) in [
            ("platform_mappings", "product_id"),
            ("platform_snapshots", "product_id"),
            ("inventory_history", "product_id"),
            ("listing_descriptions", "product_id"),
            ("listing_photos", "product_id"),
            ("pricing_snapshots", "product_id"),
            ("cached_listings", "product_id"),
            ("product_vendor_data", "product_id"),
            ("product_variants", "product_id"),
            ("products", "id"),
        ] {
            assert_eq!(
                count_rows(&db, table, column, "p-del").await,
                0,
                "{table} still holds an orphan after the delete"
            );
        }

        // Scoped to the deleted product only.
        assert!(db.get_product("p-keep").await.unwrap().is_some());
        assert_eq!(
            count_rows(&db, "listing_descriptions", "product_id", "p-keep").await,
            1
        );
        assert_eq!(
            count_rows(&db, "cached_listings", "product_id", "p-keep").await,
            1
        );
    }

    #[tokio::test]
    async fn deleting_a_variant_cleans_up_its_mappings_and_vendor_data() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("test.db").to_str().unwrap())
            .await
            .unwrap();
        db.migrate().await.unwrap();

        db.insert_product(&deletable_product("p-v")).await.unwrap();
        db.insert_variant(&deletable_variant("v-keep", "p-v"))
            .await
            .unwrap();
        db.insert_variant(&deletable_variant("v-gone", "p-v"))
            .await
            .unwrap();
        db.insert_mapping("p-v", "ebay", "ITEM-1", Some("SKU-v-keep"), "v-keep")
            .await
            .unwrap();
        db.insert_mapping("p-v", "ebay", "ITEM-2", Some("SKU-v-gone"), "v-gone")
            .await
            .unwrap();
        db.connect()
            .await
            .unwrap()
            .execute(
                "INSERT INTO product_vendor_data (product_id, variant_id, plugin_id, vendor_item_id, title)
                 VALUES ('p-v', 'v-gone', 'plug', 'VI-1', 'Vendor')",
                (),
            )
            .await
            .unwrap();

        db.delete_variant("v-gone").await.unwrap();

        assert!(db.get_variant("v-gone").await.unwrap().is_none());
        assert_eq!(
            count_rows(&db, "platform_mappings", "variant_id", "v-gone").await,
            0,
            "the variant's mapping was orphaned"
        );
        assert_eq!(
            count_rows(&db, "product_vendor_data", "variant_id", "v-gone").await,
            0,
            "the variant's vendor data was orphaned"
        );

        // The sibling variant and its rows are untouched.
        assert!(db.get_variant("v-keep").await.unwrap().is_some());
        assert_eq!(
            count_rows(&db, "platform_mappings", "variant_id", "v-keep").await,
            1
        );
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
    async fn deleted_mapping_can_be_recreated() {
        // N-4.5: `delete_mapping` soft-deletes, and the unique indexes used
        // to span the tombstone, so re-mapping the same listing always hit
        // UNIQUE. With the partial indexes (migration 023) a deleted
        // mapping's keys are free again.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remap.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();

        for (id, sku) in [("p1", "SKU-P1"), ("p2", "SKU-P2")] {
            db.insert_product(&Product {
                id: id.to_string(),
                canonical_sku: sku.to_string(),
                name: format!("Product {id}"),
                quantity: 5,
                low_stock_threshold: None,
                is_tracked: true,
                has_variants: false,
                created_at: String::new(),
                updated_at: String::new(),
            })
            .await
            .unwrap();
        }

        db.insert_mapping("p1", "ebay", "EBAY-123", Some("SKU-P1"), "v1")
            .await
            .unwrap();
        let original_id = db.list_mappings_for_product("p1").await.unwrap()[0].id;
        db.delete_mapping(original_id).await.unwrap();
        assert!(db
            .get_mapping_by_platform_item("ebay", "EBAY-123")
            .await
            .unwrap()
            .is_none());

        // Re-create the exact same mapping: same product, platform, item
        // and variant — both partial unique indexes are exercised.
        db.insert_mapping("p1", "ebay", "EBAY-123", Some("SKU-P1"), "v1")
            .await
            .unwrap();
        let mappings = db.list_mappings_for_product("p1").await.unwrap();
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].platform_item_id, "EBAY-123");

        // The tombstone is still stored (P2P propagates deletions from it),
        // alongside the live replacement.
        assert_eq!(
            count_rows(&db, "platform_mappings", "platform_item_id", "EBAY-123").await,
            2
        );

        // Unmap again and map the same platform item to a different
        // product: `idx_mappings_platform_item` alone must not block it.
        db.delete_mapping(mappings[0].id).await.unwrap();
        db.insert_mapping("p2", "ebay", "EBAY-123", Some("SKU-P2"), "v9")
            .await
            .unwrap();
        let found = db
            .get_mapping_by_platform_item("ebay", "EBAY-123")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.product_id, "p2");
    }

    #[tokio::test]
    async fn record_created_listing_records_mapping_and_listing_atomically() {
        // N-4.5: the mapping for a newly created listing and the local
        // record of that listing land in one transaction — both or neither.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("created-listing.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();

        db.insert_product(&Product {
            id: "p1".to_string(),
            canonical_sku: "SKU-P1".to_string(),
            name: "Product 1".to_string(),
            quantity: 5,
            low_stock_threshold: None,
            is_tracked: true,
            has_variants: false,
            created_at: String::new(),
            updated_at: String::new(),
        })
        .await
        .unwrap();

        // Happy path: mapping and cached listing record appear together.
        db.record_created_listing(
            "p1",
            "ebay",
            "EBAY-NEW",
            Some("SKU-P1"),
            "v1",
            Some(r#"{"title":"Widget"}"#),
        )
        .await
        .unwrap();
        assert_eq!(db.list_mappings_for_product("p1").await.unwrap().len(), 1);
        assert_eq!(
            db.get_cached_listings("p1").await.unwrap(),
            vec![r#"{"title":"Widget"}"#.to_string()]
        );

        // Failure path: pre-seed a cached record whose primary key the next
        // call would reuse, so its second statement fails. The mapping
        // insert from the same call must roll back with it.
        db.save_cached_listing("p1", "etsy", "ETSY-NEW", "{}")
            .await
            .unwrap();
        assert!(db
            .record_created_listing(
                "p1",
                "etsy",
                "ETSY-NEW",
                Some("SKU-P1"),
                "v1",
                Some(r#"{"title":"Gadget"}"#),
            )
            .await
            .is_err());
        assert!(
            db.get_mapping_by_platform_item("etsy", "ETSY-NEW")
                .await
                .unwrap()
                .is_none(),
            "a failed listing record must not leave its mapping behind"
        );
        assert_eq!(
            db.get_cached_listings("p1").await.unwrap().len(),
            2,
            "the pre-existing cached records must be untouched"
        );
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

        db.upsert_snapshot("p1", "ebay", None, 10, "2025-01-01 00:00:00")
            .await
            .unwrap();

        let snap = db.get_snapshot("p1", "ebay", None).await.unwrap().unwrap();
        assert_eq!(snap.last_known_quantity, 10);

        // Update via upsert
        db.upsert_snapshot("p1", "ebay", None, 8, "2025-01-01 00:05:00")
            .await
            .unwrap();
        let snap = db.get_snapshot("p1", "ebay", None).await.unwrap().unwrap();
        assert_eq!(snap.last_known_quantity, 8);
    }

    #[tokio::test]
    async fn test_snapshots_are_keyed_per_variant() {
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
            has_variants: true,
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.insert_product(&product).await.unwrap();

        // Two variants on the same platform must hold independent snapshots.
        db.upsert_snapshot("p1", "ebay", Some("va"), 3, "2025-01-01 00:00:00")
            .await
            .unwrap();
        db.upsert_snapshot("p1", "ebay", Some("vb"), 7, "2025-01-01 00:00:00")
            .await
            .unwrap();
        db.upsert_snapshot("p1", "ebay", None, 10, "2025-01-01 00:00:00")
            .await
            .unwrap();

        let sa = db
            .get_snapshot("p1", "ebay", Some("va"))
            .await
            .unwrap()
            .unwrap();
        let sb = db
            .get_snapshot("p1", "ebay", Some("vb"))
            .await
            .unwrap()
            .unwrap();
        let sn = db.get_snapshot("p1", "ebay", None).await.unwrap().unwrap();
        assert_eq!(sa.last_known_quantity, 3);
        assert_eq!(sa.variant_id.as_deref(), Some("va"));
        assert_eq!(sb.last_known_quantity, 7);
        assert_eq!(sb.variant_id.as_deref(), Some("vb"));
        assert_eq!(sn.last_known_quantity, 10);
        assert_eq!(sn.variant_id, None);

        // Updating one variant's snapshot must not touch the others.
        db.upsert_snapshot("p1", "ebay", Some("va"), 2, "2025-01-01 00:05:00")
            .await
            .unwrap();
        assert_eq!(
            db.get_snapshot("p1", "ebay", Some("va"))
                .await
                .unwrap()
                .unwrap()
                .last_known_quantity,
            2
        );
        assert_eq!(
            db.get_snapshot("p1", "ebay", Some("vb"))
                .await
                .unwrap()
                .unwrap()
                .last_known_quantity,
            7
        );
        assert_eq!(
            db.get_snapshot("p1", "ebay", None)
                .await
                .unwrap()
                .unwrap()
                .last_known_quantity,
            10
        );

        // The p2p export path carries the variant id through.
        let all = db.list_all_snapshots().await.unwrap();
        assert_eq!(all.len(), 3);
        assert!(all.iter().any(|s| s.variant_id.as_deref() == Some("va")));
        assert!(all.iter().any(|s| s.variant_id.as_deref() == Some("vb")));
        assert!(all.iter().any(|s| s.variant_id.is_none()));
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

    // ── Leaving a company ─────────────────────────────────────

    #[tokio::test]
    async fn leaving_a_company_registered_with_the_app_db_keeps_the_app_db() {
        // A legacy-migrated company is registered with the app DB's own path.
        let dir = tempfile::tempdir().unwrap();
        let app_path = dir.path().join("nisaba.db");
        let app_db = Db::open(app_path.to_str().unwrap()).await.unwrap();
        app_db.migrate().await.unwrap();
        app_db
            .register_company(
                "legacy",
                "Legacy",
                "admin",
                "hash",
                app_path.to_str().unwrap(),
            )
            .await
            .unwrap();
        app_db
            .register_company("other", "Other", "member", "hash", "elsewhere.db")
            .await
            .unwrap();

        // What `leave_company` does: look up the path, unregister, delete.
        let registry = app_db.list_registered_companies().await.unwrap();
        let legacy = registry.iter().find(|e| e.id == "legacy").unwrap();
        app_db.unregister_company("legacy").await.unwrap();
        let deleted =
            remove_company_db_file(Path::new(&legacy.db_path), Path::new(app_db.path())).unwrap();

        assert!(!deleted);
        assert!(app_path.exists());
        let remaining = app_db.list_registered_companies().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "other");
    }

    #[test]
    fn the_app_db_is_recognised_under_another_spelling_of_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let app_path = dir.path().join("nisaba.db");
        std::fs::write(&app_path, b"app").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let alias = dir.path().join("sub").join("..").join("nisaba.db");

        assert!(!remove_company_db_file(&alias, &app_path).unwrap());
        assert!(app_path.exists());

        #[cfg(unix)]
        {
            let link = dir.path().join("link.db");
            std::os::unix::fs::symlink(&app_path, &link).unwrap();
            assert!(!remove_company_db_file(&link, &app_path).unwrap());
            assert!(app_path.exists());
        }
    }

    #[test]
    fn leaving_a_company_with_its_own_db_deletes_only_that_file() {
        let dir = tempfile::tempdir().unwrap();
        let app_path = dir.path().join("nisaba.db");
        let company_path = dir.path().join("company.db");
        std::fs::write(&app_path, b"app").unwrap();
        std::fs::write(&company_path, b"company").unwrap();

        assert!(remove_company_db_file(&company_path, &app_path).unwrap());
        assert!(!company_path.exists());
        assert!(app_path.exists());
        // Already gone counts as deleted.
        assert!(remove_company_db_file(&company_path, &app_path).unwrap());
    }

    // ── Recovering from a panic on open ───────────────────────

    type BuildFuture =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Database, TursoError>> + Send>>;

    /// A database builder that panics on its first `panics` calls, the way
    /// turso does on a file whose journals don't match it, and counts calls.
    fn build_panicking_first(
        panics: usize,
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) -> impl Fn(String) -> BuildFuture {
        move |path| {
            let attempt = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move {
                if attempt < panics {
                    panic!("simulated turso panic on open");
                }
                Builder::new_local(&path).build().await
            })
        }
    }

    fn journal_backups(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|p| p.to_string_lossy().contains(".journal-backup-"))
            .collect()
    }

    #[test]
    fn journal_files_are_moved_aside_with_their_contents() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nisaba.db");
        std::fs::write(&db_path, b"main").unwrap();
        std::fs::write(dir.path().join("nisaba.db-wal"), b"uncheckpointed").unwrap();
        std::fs::write(dir.path().join("nisaba.db-shm"), b"index").unwrap();

        let backup = move_journal_files_aside(&db_path).unwrap().unwrap();

        assert_eq!(backup.parent(), Some(dir.path()));
        assert_eq!(journal_backups(dir.path()), vec![backup.clone()]);
        assert_eq!(
            std::fs::read(backup.join("nisaba.db-wal")).unwrap(),
            b"uncheckpointed"
        );
        assert_eq!(
            std::fs::read(backup.join("nisaba.db-shm")).unwrap(),
            b"index"
        );
        assert!(!dir.path().join("nisaba.db-wal").exists());
        assert!(!dir.path().join("nisaba.db-shm").exists());
        assert_eq!(std::fs::read(&db_path).unwrap(), b"main");
    }

    #[test]
    fn a_db_without_journal_files_gets_no_backup_dir() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nisaba.db");
        std::fs::write(&db_path, b"main").unwrap();

        assert_eq!(move_journal_files_aside(&db_path).unwrap(), None);
        assert!(journal_backups(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn a_panicking_open_is_retried_once_with_the_journals_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nisaba.db");
        std::fs::write(dir.path().join("nisaba.db-wal"), b"uncheckpointed").unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let opened = open_database(
            db_path.to_str().unwrap(),
            build_panicking_first(1, calls.clone()),
        )
        .await;

        assert!(
            opened.is_ok(),
            "retry should open the DB: {:?}",
            opened.err()
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        let backups = journal_backups(dir.path());
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read(backups[0].join("nisaba.db-wal")).unwrap(),
            b"uncheckpointed"
        );
    }

    #[tokio::test]
    async fn an_open_that_panics_again_on_retry_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nisaba.db");
        std::fs::write(&db_path, b"main").unwrap();
        std::fs::write(dir.path().join("nisaba.db-wal"), b"uncheckpointed").unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let opened = open_database(
            db_path.to_str().unwrap(),
            build_panicking_first(usize::MAX, calls.clone()),
        )
        .await;

        assert!(opened.is_err());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(std::fs::read(&db_path).unwrap(), b"main");
        let backups = journal_backups(dir.path());
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read(backups[0].join("nisaba.db-wal")).unwrap(),
            b"uncheckpointed"
        );
    }

    // ── Plugin `secret: true` config values ────────────────────

    const SECRET_FIELDS_JSON: &str = r#"[
        {"key":"api_key","label":"API key","required":true,"secret":true},
        {"key":"region","label":"Region","required":false,"secret":false}
    ]"#;

    fn secret_plugin_row(id: &str) -> VendorPluginRow {
        let mut row = registry_row(id, None);
        row.config_fields = Some(SECRET_FIELDS_JSON.to_string());
        row
    }

    async fn secret_test_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();
        (dir, db)
    }

    // ── Per-plugin KV store (N-7.3) ─────────────────────────────────────

    #[tokio::test]
    async fn plugin_kv_store_crud_scoping_and_accounting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kv.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();

        // Missing keys read as None.
        assert_eq!(db.plugin_store_get("p1", "cursor").await.unwrap(), None);
        assert_eq!(db.plugin_store_total_bytes("p1").await.unwrap(), 0);

        // Set, read, overwrite.
        db.plugin_store_set("p1", "cursor", "page-3").await.unwrap();
        assert_eq!(
            db.plugin_store_get("p1", "cursor")
                .await
                .unwrap()
                .as_deref(),
            Some("page-3")
        );
        db.plugin_store_set("p1", "cursor", "page-4").await.unwrap();
        assert_eq!(
            db.plugin_store_get("p1", "cursor")
                .await
                .unwrap()
                .as_deref(),
            Some("page-4")
        );

        // Scoped per plugin: another plugin reads nothing.
        assert_eq!(db.plugin_store_get("p2", "cursor").await.unwrap(), None);
        db.plugin_store_set("p2", "cursor", "p2-value")
            .await
            .unwrap();
        assert_eq!(
            db.plugin_store_get("p1", "cursor")
                .await
                .unwrap()
                .as_deref(),
            Some("page-4")
        );

        // Byte accounting charges key + value.
        db.plugin_store_set("p3", "k1", "vv").await.unwrap();
        assert_eq!(db.plugin_store_total_bytes("p3").await.unwrap(), 4);
        db.plugin_store_set("p3", "k1", "vvvv").await.unwrap();
        assert_eq!(db.plugin_store_total_bytes("p3").await.unwrap(), 6);

        // Delete reports whether the key existed.
        assert!(db.plugin_store_delete("p3", "k1").await.unwrap());
        assert!(!db.plugin_store_delete("p3", "k1").await.unwrap());
        assert_eq!(db.plugin_store_total_bytes("p3").await.unwrap(), 0);

        // Clearing drops the whole plugin's store (uninstall path).
        db.clear_plugin_store("p1").await.unwrap();
        assert_eq!(db.plugin_store_get("p1", "cursor").await.unwrap(), None);
        assert_eq!(db.plugin_store_total_bytes("p1").await.unwrap(), 0);
        // Other plugins are untouched.
        assert_eq!(
            db.plugin_store_get("p2", "cursor")
                .await
                .unwrap()
                .as_deref(),
            Some("p2-value")
        );
    }

    // ── Stock ledger (N-4.1) ──────────────────────────────────────

    fn ledger_product(id: &str, quantity: i64) -> Product {
        Product {
            id: id.to_string(),
            canonical_sku: format!("SKU-{id}"),
            name: format!("Product {id}"),
            quantity,
            low_stock_threshold: None,
            is_tracked: true,
            has_variants: true,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn ledger_variant(id: &str, product_id: &str, quantity: i64) -> ProductVariant {
        ProductVariant {
            id: id.to_string(),
            product_id: product_id.to_string(),
            sku: format!("SKU-{id}"),
            name: format!("Variant {id}"),
            attributes: std::collections::HashMap::new(),
            quantity,
            on_hand_quantity: quantity,
            image_url: None,
            sort_order: 0,
            source_plugin_id: None,
            source_vendor_item_id: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    async fn open_migrated_ledger_db(name: &str) -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();
        (dir, db)
    }

    #[test]
    fn secret_config_keys_parses_secret_true_fields() {
        let keys = secret_config_keys(Some(SECRET_FIELDS_JSON));
        assert_eq!(keys, HashSet::from(["api_key".to_string()]));
        assert!(secret_config_keys(None).is_empty());
        assert!(secret_config_keys(Some("not json")).is_empty());
    }

    #[test]
    fn strip_secret_values_removes_secret_true_entries() {
        let stripped = strip_secret_values(
            Some(SECRET_FIELDS_JSON),
            Some(r#"{"api_key":"s3cr3t","region":"us"}"#),
        );
        assert_eq!(stripped.as_deref(), Some(r#"{"region":"us"}"#));

        // Nothing secret declared: unchanged.
        let untouched = strip_secret_values(None, Some(r#"{"api_key":"s3cr3t"}"#));
        assert_eq!(untouched.as_deref(), Some(r#"{"api_key":"s3cr3t"}"#));

        // No secrets present: the original string is returned as-is.
        let same = strip_secret_values(Some(SECRET_FIELDS_JSON), Some(r#"{"region":"us"}"#));
        assert_eq!(same.as_deref(), Some(r#"{"region":"us"}"#));

        // Unparseable input is never silently dropped.
        let corrupt = strip_secret_values(Some(SECRET_FIELDS_JSON), Some("{oops"));
        assert_eq!(corrupt.as_deref(), Some("{oops"));
    }

    #[tokio::test]
    async fn set_plugin_config_split_routes_secrets_to_the_secret_store() {
        let (_dir, db) = secret_test_db().await;
        db.upsert_registry_plugin(&secret_plugin_row("p1"))
            .await
            .unwrap();

        let store = crate::crypto::InMemoryPluginSecrets::new();
        let mut config = HashMap::new();
        config.insert("api_key".to_string(), "s3cr3t-token".to_string());
        config.insert("region".to_string(), "us".to_string());
        db.set_plugin_config_split("p1", &config, &store)
            .await
            .unwrap();

        // The secret is in the store, not in the database…
        assert_eq!(
            store.load("p1", "api_key").unwrap().as_deref(),
            Some("s3cr3t-token")
        );
        let raw = db
            .get_registry_plugin("p1")
            .await
            .unwrap()
            .unwrap()
            .config_json
            .unwrap();
        assert!(
            !raw.contains("s3cr3t-token"),
            "config_json must not contain the secret: {raw}"
        );
        // …while the non-secret value is still there.
        let raw_map: HashMap<String, String> = serde_json::from_str(&raw).unwrap();
        assert_eq!(raw_map.get("region").map(String::as_str), Some("us"));
        assert!(!raw_map.contains_key("api_key"));
    }

    #[tokio::test]
    async fn get_plugin_config_merged_injects_secrets_for_runs() {
        let (_dir, db) = secret_test_db().await;
        db.upsert_registry_plugin(&secret_plugin_row("p1"))
            .await
            .unwrap();

        let store = crate::crypto::InMemoryPluginSecrets::new();
        let mut config = HashMap::new();
        config.insert("api_key".to_string(), "s3cr3t-token".to_string());
        config.insert("region".to_string(), "us".to_string());
        db.set_plugin_config_split("p1", &config, &store)
            .await
            .unwrap();

        let merged = db.get_plugin_config_merged("p1", &store).await.unwrap();
        assert_eq!(
            merged.get("api_key").map(String::as_str),
            Some("s3cr3t-token")
        );
        assert_eq!(merged.get("region").map(String::as_str), Some("us"));
    }

    #[tokio::test]
    async fn legacy_plaintext_secrets_migrate_to_the_secret_store_on_first_load() {
        let (_dir, db) = secret_test_db().await;
        // A row written by an older version: the secret sits in config_json.
        let mut row = secret_plugin_row("p1");
        row.config_json = Some(r#"{"api_key":"legacy-secret","region":"us"}"#.to_string());
        db.upsert_registry_plugin(&row).await.unwrap();

        let store = crate::crypto::InMemoryPluginSecrets::new();
        let merged = db.get_plugin_config_merged("p1", &store).await.unwrap();

        // The plugin still gets its full config…
        assert_eq!(
            merged.get("api_key").map(String::as_str),
            Some("legacy-secret")
        );
        assert_eq!(merged.get("region").map(String::as_str), Some("us"));
        // …the secret moved to the store…
        assert_eq!(
            store.load("p1", "api_key").unwrap().as_deref(),
            Some("legacy-secret")
        );
        // …and config_json was scrubbed.
        let raw = db
            .get_registry_plugin("p1")
            .await
            .unwrap()
            .unwrap()
            .config_json
            .unwrap();
        assert!(
            !raw.contains("legacy-secret"),
            "config_json must be scrubbed after migration: {raw}"
        );
        let raw_map: HashMap<String, String> = serde_json::from_str(&raw).unwrap();
        assert_eq!(raw_map.get("region").map(String::as_str), Some("us"));
    }

    #[tokio::test]
    async fn clearing_a_secret_field_deletes_the_stored_entry() {
        let (_dir, db) = secret_test_db().await;
        db.upsert_registry_plugin(&secret_plugin_row("p1"))
            .await
            .unwrap();

        let store = crate::crypto::InMemoryPluginSecrets::new();
        let mut config = HashMap::new();
        config.insert("api_key".to_string(), "s3cr3t-token".to_string());
        db.set_plugin_config_split("p1", &config, &store)
            .await
            .unwrap();
        assert!(store.load("p1", "api_key").unwrap().is_some());

        // Saving an empty secret clears it.
        config.insert("api_key".to_string(), String::new());
        db.set_plugin_config_split("p1", &config, &store)
            .await
            .unwrap();
        assert_eq!(store.load("p1", "api_key").unwrap(), None);
        let merged = db.get_plugin_config_merged("p1", &store).await.unwrap();
        assert!(!merged.contains_key("api_key"));
    }

    #[tokio::test]
    async fn deleting_a_plugin_removes_its_secrets_from_the_store() {
        let (_dir, db) = secret_test_db().await;
        db.upsert_registry_plugin(&secret_plugin_row("p1"))
            .await
            .unwrap();

        let store = crate::crypto::InMemoryPluginSecrets::new();
        let mut config = HashMap::new();
        config.insert("api_key".to_string(), "s3cr3t-token".to_string());
        db.set_plugin_config_split("p1", &config, &store)
            .await
            .unwrap();

        db.delete_registry_plugin_with_secrets("p1", &store)
            .await
            .unwrap();
        assert_eq!(store.load("p1", "api_key").unwrap(), None);
        assert!(db.get_registry_plugin("p1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn duplicate_sale_key_is_a_noop() {
        let (_dir, db) = open_migrated_ledger_db("ledger-dup.db").await;
        db.insert_product(&ledger_product("p1", 10)).await.unwrap();
        db.insert_variant(&ledger_variant("v1", "p1", 10))
            .await
            .unwrap();
        // Variants created after migration have no opening event yet; record
        // one explicitly so the ledger starts at the current quantity.
        assert!(db
            .record_stock_event(&StockEvent::opening("p1", "v1", 10))
            .await
            .unwrap());

        let sale = StockEvent::sale("ebay", "ORDER-1", "p1", "v1", 3);
        assert!(db.record_stock_event(&sale).await.unwrap());
        // Recording the very same sale again (same platform + order id) must
        // not change the ledger.
        assert!(!db.record_stock_event(&sale).await.unwrap());

        let events = db.stock_events_for_variant("v1").await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(db.canonical_variant_quantity("v1").await.unwrap(), 7);

        let refreshed = db.refresh_variant_quantity_from_ledger("v1").await.unwrap();
        assert_eq!(refreshed, 7);
        assert_eq!(db.get_product("p1").await.unwrap().unwrap().quantity, 10);
    }

    #[tokio::test]
    async fn mixed_event_kinds_sum_correctly() {
        let (_dir, db) = open_migrated_ledger_db("ledger-mixed.db").await;
        db.insert_product(&ledger_product("p1", 10)).await.unwrap();
        db.insert_variant(&ledger_variant("v1", "p1", 10))
            .await
            .unwrap();

        assert!(db
            .record_stock_event(&StockEvent::opening("p1", "v1", 10))
            .await
            .unwrap());
        assert!(db
            .record_stock_event(&StockEvent::sale("squarespace", "ORD-9", "p1", "v1", 3))
            .await
            .unwrap());
        let mut restock = StockEvent::keyed(StockEventKind::Restock, "p1", "v1", 5);
        restock.platform = Some("manual".to_string());
        assert!(db.record_stock_event(&restock).await.unwrap());
        let mut adjust = StockEvent::keyed(StockEventKind::Adjustment, "p1", "v1", -1);
        adjust.note = Some("damaged unit".to_string());
        assert!(db.record_stock_event(&adjust).await.unwrap());
        let vendor = StockEvent::keyed(StockEventKind::VendorUpdate, "p1", "v1", 2);
        assert!(db.record_stock_event(&vendor).await.unwrap());

        // 10 - 3 + 5 - 1 + 2 = 13
        assert_eq!(db.canonical_variant_quantity("v1").await.unwrap(), 13);

        let events = db.stock_events_for_variant("v1").await.unwrap();
        let kinds: Vec<StockEventKind> = events.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                StockEventKind::Opening,
                StockEventKind::Sale,
                StockEventKind::Restock,
                StockEventKind::Adjustment,
                StockEventKind::VendorUpdate,
            ]
        );
        assert_eq!(events[1].source_order_id.as_deref(), Some("ORD-9"));

        let refreshed = db.refresh_variant_quantity_from_ledger("v1").await.unwrap();
        assert_eq!(refreshed, 13);
        let variant = db.get_variant("v1").await.unwrap().unwrap();
        assert_eq!(variant.quantity, 13);
    }

    #[tokio::test]
    async fn backfill_preserves_current_quantities() {
        // Variants that already exist when migration 020 runs get one opening
        // event each, so the ledger sum equals the stored quantity.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger-backfill.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();
        let conn = db.connect().await.unwrap();
        conn.execute(
            "INSERT INTO products (id, canonical_sku, name, quantity, is_tracked, has_variants)
             VALUES ('p1', 'SKU-p1', 'P1', 7, 1, 1), ('p2', 'SKU-p2', 'P2', 0, 1, 1)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO product_variants (id, product_id, sku, name, attributes_json, quantity, on_hand_quantity, sort_order)
             VALUES ('v1', 'p1', 'SKU-v1', 'V1', '{}', 7, 7, 0),
                    ('v2', 'p2', 'SKU-v2', 'V2', '{}', 4, 4, 0)",
            (),
        )
        .await
        .unwrap();
        // Simulate a pre-020 database: reset the version, then migrate again.
        conn.execute("PRAGMA user_version = 19", ()).await.unwrap();
        db.migrate().await.unwrap();

        assert_eq!(db.canonical_variant_quantity("v1").await.unwrap(), 7);
        assert_eq!(db.canonical_variant_quantity("v2").await.unwrap(), 4);

        // Re-running the backfill must not double-count the opening events.
        db.backfill_ledger_opening_events().await.unwrap();
        assert_eq!(db.canonical_variant_quantity("v1").await.unwrap(), 7);
        assert_eq!(db.canonical_variant_quantity("v2").await.unwrap(), 4);

        let events = db.stock_events_for_variant("v1").await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, StockEventKind::Opening);
        assert_eq!(events[0].event_key, "opening:v1");
        assert_eq!(events[0].delta, 7);
    }

    // ── N-4.7: execute_write semantics, history-on-change, pruning ──

    async fn migrated_test_db(name: &str) -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn execute_write_returns_the_statements_row_count() {
        let (_dir, db) = migrated_test_db("exec-write-count.db").await;

        let inserted = db
            .execute_write(
                "INSERT INTO products (id, canonical_sku, name, quantity)
                 VALUES ('p1', 'SKU-p1', 'P1', 1), ('p2', 'SKU-p2', 'P2', 2)",
                (),
            )
            .await
            .unwrap();
        assert_eq!(inserted, 2, "INSERT reports its own row count");

        let updated = db
            .execute_write("UPDATE products SET quantity = quantity + 10", ())
            .await
            .unwrap();
        assert_eq!(updated, 2, "UPDATE reports the rows it touched");

        let deleted = db
            .execute_write("DELETE FROM products WHERE id = 'p1'", ())
            .await
            .unwrap();
        assert_eq!(deleted, 1);

        let no_op = db
            .execute_write("DELETE FROM products WHERE id = 'missing'", ())
            .await
            .unwrap();
        assert_eq!(
            no_op, 0,
            "a statement matching nothing reports 0, not COMMIT's count"
        );
    }

    #[tokio::test]
    async fn execute_write_does_not_commit_a_failed_statement() {
        let (_dir, db) = migrated_test_db("exec-write-failure.db").await;

        db.execute_write(
            "INSERT INTO products (id, canonical_sku, name, quantity)
             VALUES ('p1', 'SKU-p1', 'P1', 1)",
            (),
        )
        .await
        .unwrap();

        // Duplicate primary key: the statement fails, so nothing may be
        // committed and the error must surface.
        let result = db
            .execute_write(
                "INSERT INTO products (id, canonical_sku, name, quantity)
                 VALUES ('p1', 'SKU-dupe', 'Dupe', 99)",
                (),
            )
            .await;
        assert!(result.is_err(), "failed statement returns an error");

        let product = db.get_product("p1").await.unwrap().unwrap();
        assert_eq!(product.name, "P1", "the failed insert left no trace");
        assert_eq!(product.quantity, 1);

        // The database is still writable after the rolled-back failure.
        let n = db
            .execute_write("UPDATE products SET quantity = 5 WHERE id = 'p1'", ())
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn record_inventory_history_writes_only_on_change() {
        let (_dir, db) = migrated_test_db("history-on-change.db").await;
        db.execute_write(
            "INSERT INTO products (id, canonical_sku, name, quantity)
             VALUES ('p1', 'SKU-p1', 'P1', 5)",
            (),
        )
        .await
        .unwrap();

        // First observation records a row; repeats of the same quantity —
        // the every-cycle spam this guards against — do not.
        db.record_inventory_history("p1", 5, "cyc-1").await.unwrap();
        db.record_inventory_history("p1", 5, "cyc-2").await.unwrap();
        db.record_inventory_history("p1", 5, "cyc-3").await.unwrap();
        let history = db.get_inventory_history("p1", None).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].quantity, 5);

        // A real change records a row; changing back records another.
        db.record_inventory_history("p1", 3, "cyc-4").await.unwrap();
        db.record_inventory_history("p1", 3, "cyc-5").await.unwrap();
        db.record_inventory_history("p1", 5, "cyc-6").await.unwrap();
        let history = db.get_inventory_history("p1", None).await.unwrap();
        let quantities: Vec<i64> = history.iter().map(|h| h.quantity).collect();
        assert_eq!(quantities, vec![5, 3, 5]);
    }

    #[tokio::test]
    async fn prune_and_downsample_delete_only_old_rows() {
        let (_dir, db) = migrated_test_db("prune.db").await;
        let conn = db.connect().await.unwrap();
        conn.execute(
            "INSERT INTO sync_events (product_id, source_platform, target_platform,
                old_quantity, new_quantity, event_type, sync_cycle_id, created_at)
             VALUES ('p1', 'a', 'b', 1, 1, 'quantity_change', 'old', datetime('now', '-100 days')),
                    ('p1', 'a', 'b', 1, 1, 'quantity_change', 'new', datetime('now'))",
            (),
        )
        .await
        .unwrap();
        for (quantity, recorded_at) in [
            (9, "datetime('now', '-40 days')"),
            (8, "datetime('now', '-40 days', '+1 hour')"),
            (7, "datetime('now', '-40 days', '+2 hours')"),
            (4, "datetime('now')"),
        ] {
            conn.execute(
                &format!(
                    "INSERT INTO inventory_history (product_id, quantity, sync_cycle_id, recorded_at)
                     VALUES ('p1', {quantity}, 'cyc', {recorded_at})"
                ),
                (),
            )
            .await
            .unwrap();
        }
        drop(conn);

        // The returned counts are the DELETEs' own row counts (this also
        // pins execute_write reporting the statement count, not COMMIT's).
        let pruned = db.prune_old_sync_events(90).await.unwrap();
        assert_eq!(pruned, 1);
        let events = db.list_recent_sync_events(10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sync_cycle_id, "new");

        let removed = db.downsample_old_history(30).await.unwrap();
        assert_eq!(removed, 2, "three rows on one old day thin to one");
        let history = db.get_inventory_history("p1", None).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].quantity, 9);
        assert_eq!(history[1].quantity, 4);
    }

    /// N-6.1: a computed price must round-trip through the DB exactly.
    /// 0.1 + 0.2 in f64 is 0.30000000000000004; in Decimal it is 0.3, and the
    /// TEXT column must hold "0.3".
    #[tokio::test]
    async fn pricing_snapshot_round_trips_an_exact_decimal() {
        let (_dir, db) = migrated_test_db("money-snapshot.db").await;
        let amount = Decimal::from_str("0.1").unwrap() + Decimal::from_str("0.2").unwrap();
        db.record_pricing_snapshot("p1", "ebay", amount, "USD")
            .await
            .unwrap();

        let prices = db.get_latest_prices("p1").await.unwrap();
        assert_eq!(prices.len(), 1);
        assert_eq!(prices[0].amount.to_string(), "0.3");
        assert!(
            !prices[0].amount.to_string().contains("0.30000000000000004"),
            "f64 artifact survived the DB round-trip"
        );

        // The stored value is TEXT holding the exact decimal, not a REAL.
        let conn = db.connect().await.unwrap();
        let mut rows = conn
            .query("SELECT amount, typeof(amount) FROM pricing_snapshots", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "0.3");
        assert_eq!(row.get::<String>(1).unwrap(), "text");
    }

    /// N-6.1: vendor cache prices round-trip exactly too.
    /// N-7.2: images and the typed upc/brand/mpn/weight/dimensions round-trip.
    #[tokio::test]
    async fn vendor_cache_price_round_trips_an_exact_decimal() {
        let (_dir, db) = migrated_test_db("money-vendor.db").await;
        let listing = VendorListingCache {
            plugin_id: "plug".to_string(),
            vendor_item_id: "VI-1".to_string(),
            title: "Widget".to_string(),
            price: Some(Decimal::from_str("19.99").unwrap()),
            currency: Some("USD".to_string()),
            quantity: Some(3),
            sku: None,
            images: vec![
                "https://cdn/1.jpg".to_string(),
                "https://cdn/2.jpg".to_string(),
            ],
            image_url: None,
            url: None,
            upc: Some("001234567890".to_string()),
            brand: Some("Acme".to_string()),
            mpn: Some("ACM-1".to_string()),
            weight: Some(Weight {
                value: 1.5,
                unit: "lb".to_string(),
            }),
            dimensions: Some(Dimensions {
                length: 12.0,
                width: 9.0,
                height: 4.0,
                unit: "in".to_string(),
            }),
            extras: HashMap::new(),
            group_key: None,
            variant_attributes: HashMap::new(),
            fetched_at: "2026-10-20 00:00:00".to_string(),
        };
        db.save_vendor_listings_cache("plug", &[listing])
            .await
            .unwrap();

        let rows = db.get_vendor_listings_cache("plug").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].price.map(|d| d.to_string()),
            Some("19.99".to_string())
        );
        assert_eq!(rows[0].images, ["https://cdn/1.jpg", "https://cdn/2.jpg"]);
        assert_eq!(rows[0].upc.as_deref(), Some("001234567890"));
        assert_eq!(rows[0].brand.as_deref(), Some("Acme"));
        assert_eq!(rows[0].mpn.as_deref(), Some("ACM-1"));
        assert_eq!(
            rows[0].weight,
            Some(Weight {
                value: 1.5,
                unit: "lb".to_string()
            })
        );
        assert_eq!(
            rows[0].dimensions,
            Some(Dimensions {
                length: 12.0,
                width: 9.0,
                height: 4.0,
                unit: "in".to_string()
            })
        );

        db.save_product_vendor_data("p1", "v1", "plug", &rows[0])
            .await
            .unwrap();
        let pvd = db.get_product_vendor_data("p1").await.unwrap();
        assert_eq!(pvd.len(), 1);
        assert_eq!(
            pvd[0].2.price.map(|d| d.to_string()),
            Some("19.99".to_string())
        );
        assert_eq!(pvd[0].2.images, ["https://cdn/1.jpg", "https://cdn/2.jpg"]);
        assert_eq!(pvd[0].2.upc.as_deref(), Some("001234567890"));
        assert_eq!(
            pvd[0].2.dimensions,
            Some(Dimensions {
                length: 12.0,
                width: 9.0,
                height: 4.0,
                unit: "in".to_string()
            })
        );
    }

    /// N-7.4: `merge_vendor_listings_cache` upserts changed rows, removes
    /// deleted ids, and leaves untouched rows — including their stored
    /// `fetched_at` — alone.
    #[tokio::test]
    async fn merge_vendor_listings_cache_applies_changes_and_deletes() {
        let (_dir, db) = migrated_test_db("merge-cache.db").await;

        let row = |id: &str, title: &str, fetched_at: &str| VendorListingCache {
            plugin_id: "plug".to_string(),
            vendor_item_id: id.to_string(),
            title: title.to_string(),
            price: None,
            currency: None,
            quantity: None,
            sku: None,
            images: vec![],
            image_url: None,
            url: None,
            upc: None,
            brand: None,
            mpn: None,
            weight: None,
            dimensions: None,
            extras: HashMap::new(),
            group_key: None,
            variant_attributes: HashMap::new(),
            fetched_at: fetched_at.to_string(),
        };

        db.save_vendor_listings_cache(
            "plug",
            &[
                row("a", "Apple", "2026-10-20 00:00:00"),
                row("b", "Banana", "2026-10-20 00:00:00"),
                row("c", "Cherry", "2026-10-20 00:00:00"),
            ],
        )
        .await
        .unwrap();

        // Second run: "a" changed, "d" is new, "b" is gone at the supplier.
        db.merge_vendor_listings_cache(
            "plug",
            &[
                row("a", "Apple (new harvest)", "2026-10-21 08:00:00"),
                row("d", "Date", "2026-10-21 08:00:00"),
            ],
            &["b".to_string()],
        )
        .await
        .unwrap();

        let mut rows = db.get_vendor_listings_cache("plug").await.unwrap();
        rows.sort_by(|x, y| x.vendor_item_id.cmp(&y.vendor_item_id));
        assert_eq!(rows.len(), 3);

        let a = &rows[0];
        assert_eq!(a.vendor_item_id, "a");
        assert_eq!(a.title, "Apple (new harvest)");
        assert_eq!(a.fetched_at, "2026-10-21 08:00:00");

        let c = &rows[1];
        assert_eq!(c.vendor_item_id, "c");
        assert_eq!(c.title, "Cherry");
        // Untouched rows keep their stored fetched_at.
        assert_eq!(c.fetched_at, "2026-10-20 00:00:00");

        let d = &rows[2];
        assert_eq!(d.vendor_item_id, "d");
        assert_eq!(d.title, "Date");
        assert_eq!(d.fetched_at, "2026-10-21 08:00:00");
    }

    /// N-7.4: the host's incremental-sync state (`since` + `cursor`) round-trips
    /// and clears.
    #[tokio::test]
    async fn vendor_sync_state_round_trips_and_clears() {
        let (_dir, db) = migrated_test_db("sync-state.db").await;

        assert_eq!(db.get_vendor_sync_state("plug").await.unwrap(), None);

        db.set_vendor_sync_state("plug", "2026-10-21 08:00:00", Some("c1"))
            .await
            .unwrap();
        assert_eq!(
            db.get_vendor_sync_state("plug").await.unwrap(),
            Some(("2026-10-21 08:00:00".to_string(), Some("c1".to_string())))
        );

        // A full pull records no cursor.
        db.set_vendor_sync_state("plug", "2026-10-21 09:00:00", None)
            .await
            .unwrap();
        assert_eq!(
            db.get_vendor_sync_state("plug").await.unwrap(),
            Some(("2026-10-21 09:00:00".to_string(), None))
        );

        db.clear_vendor_sync_state("plug").await.unwrap();
        assert_eq!(db.get_vendor_sync_state("plug").await.unwrap(), None);
    }

    /// N-7.2: migration 025 adds the frozen-schema columns (`images_json`,
    /// `upc`, `brand`, `mpn`, `weight_json`, `dimensions_json`) to both
    /// vendor listing tables, preserving every row — including a legacy row
    /// whose only image is the old `image_url`, which reads back folded
    /// into `images`.
    #[tokio::test]
    async fn migration_025_adds_vendor_listing_typed_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy-vendor.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        let conn = db.connect().await.unwrap();
        // Pre-25 shape of the two vendor tables.
        conn.execute_batch(
            "CREATE TABLE vendor_listings_cache (
                id INTEGER PRIMARY KEY, plugin_id TEXT NOT NULL, vendor_item_id TEXT NOT NULL,
                title TEXT NOT NULL, price TEXT, currency TEXT, quantity INTEGER, sku TEXT,
                image_url TEXT, url TEXT, extras_json TEXT NOT NULL DEFAULT '{}',
                group_key TEXT, variant_attrs_json TEXT NOT NULL DEFAULT '{}',
                fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(plugin_id, vendor_item_id));
             INSERT INTO vendor_listings_cache (plugin_id, vendor_item_id, title, quantity, image_url)
                VALUES ('plug', 'VI-1', 'Widget', 0, 'https://cdn/legacy.jpg');
             CREATE TABLE product_vendor_data (
                product_id TEXT NOT NULL, variant_id TEXT, plugin_id TEXT NOT NULL,
                vendor_item_id TEXT NOT NULL, title TEXT NOT NULL, price TEXT, currency TEXT,
                quantity INTEGER, sku TEXT, image_url TEXT, url TEXT,
                extras_json TEXT NOT NULL DEFAULT '{}', group_key TEXT,
                variant_attrs_json TEXT NOT NULL DEFAULT '{}',
                fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (product_id, plugin_id, vendor_item_id));
             INSERT INTO product_vendor_data (product_id, plugin_id, vendor_item_id, title, quantity, image_url)
                VALUES ('p1', 'plug', 'VI-1', 'Widget', NULL, 'https://cdn/legacy.jpg');
             -- get_product_vendor_data LEFT JOINs the registry for display names.
             CREATE TABLE vendor_plugin_registry (
                id TEXT PRIMARY KEY, display_name TEXT);
             INSERT INTO vendor_plugin_registry (id, display_name)
                VALUES ('plug', 'Test Plugin');",
        )
        .await
        .unwrap();
        drop(conn);

        db.migrate_step(25).await.unwrap();

        let conn = db.connect().await.unwrap();
        for table in ["vendor_listings_cache", "product_vendor_data"] {
            for col in [
                "images_json",
                "upc",
                "brand",
                "mpn",
                "weight_json",
                "dimensions_json",
            ] {
                let mut rows = conn
                    .query(
                        &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = '{col}'"),
                        (),
                    )
                    .await
                    .unwrap();
                let row = rows.next().await.unwrap().unwrap();
                assert_eq!(
                    row.get::<i64>(0).unwrap(),
                    1,
                    "{table}.{col} should exist after migration 025"
                );
            }
        }
        drop(conn);

        // Legacy rows survive, and the old single image reads back as `images`.
        let rows = db.get_vendor_listings_cache("plug").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].images, ["https://cdn/legacy.jpg"]);
        assert_eq!(rows[0].quantity, Some(0), "0 stays 0 (out of stock)");

        let pvd = db.get_product_vendor_data("p1").await.unwrap();
        assert_eq!(pvd.len(), 1);
        assert_eq!(pvd[0].2.images, ["https://cdn/legacy.jpg"]);
        assert_eq!(
            pvd[0].2.quantity, None,
            "NULL stays NULL (unknown), not coerced to 0"
        );
    }

    /// N-6.1: migration 024 converts legacy REAL price columns to TEXT
    /// holding the exact decimal string, preserving every row.
    #[tokio::test]
    async fn migration_024_converts_legacy_real_price_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy-money.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        let conn = db.connect().await.unwrap();
        // Pre-24 shape of the three money tables (REAL price/amount).
        conn.execute_batch(
            "CREATE TABLE pricing_snapshots (
                id INTEGER PRIMARY KEY, product_id TEXT NOT NULL, platform TEXT NOT NULL,
                amount REAL NOT NULL, currency TEXT NOT NULL DEFAULT 'USD',
                recorded_at TEXT NOT NULL DEFAULT (datetime('now')));
             INSERT INTO pricing_snapshots (product_id, platform, amount, currency)
                VALUES ('p1', 'ebay', 19.99, 'USD');
             CREATE TABLE vendor_listings_cache (
                id INTEGER PRIMARY KEY, plugin_id TEXT NOT NULL, vendor_item_id TEXT NOT NULL,
                title TEXT NOT NULL, price REAL, currency TEXT, quantity INTEGER, sku TEXT,
                image_url TEXT, url TEXT, extras_json TEXT NOT NULL DEFAULT '{}',
                group_key TEXT, variant_attrs_json TEXT NOT NULL DEFAULT '{}',
                fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(plugin_id, vendor_item_id));
             INSERT INTO vendor_listings_cache (plugin_id, vendor_item_id, title, price, currency)
                VALUES ('plug', 'VI-1', 'Widget', 0.3, 'USD');
             CREATE TABLE product_vendor_data (
                product_id TEXT NOT NULL, variant_id TEXT, plugin_id TEXT NOT NULL,
                vendor_item_id TEXT NOT NULL, title TEXT NOT NULL, price REAL, currency TEXT,
                quantity INTEGER, sku TEXT, image_url TEXT, url TEXT,
                extras_json TEXT NOT NULL DEFAULT '{}', group_key TEXT,
                variant_attrs_json TEXT NOT NULL DEFAULT '{}',
                fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (product_id, plugin_id, vendor_item_id));
             INSERT INTO product_vendor_data (product_id, plugin_id, vendor_item_id, title, price, currency)
                VALUES ('p1', 'plug', 'VI-1', 'Widget', 48.5, 'EUR');",
        )
        .await
        .unwrap();
        drop(conn);

        db.migrate_step(24).await.unwrap();

        let conn = db.connect().await.unwrap();
        // Declared types are TEXT now.
        for (table, col) in [
            ("pricing_snapshots", "amount"),
            ("vendor_listings_cache", "price"),
            ("product_vendor_data", "price"),
        ] {
            let mut rows = conn
                .query(
                    &format!("SELECT type FROM pragma_table_info('{table}') WHERE name = '{col}'"),
                    (),
                )
                .await
                .unwrap();
            let row = rows.next().await.unwrap().unwrap();
            assert_eq!(
                row.get::<String>(0).unwrap(),
                "TEXT",
                "{table}.{col} should be TEXT after migration 024"
            );
        }
        // Values survived as exact decimal text.
        let mut rows = conn
            .query("SELECT amount, typeof(amount) FROM pricing_snapshots", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "19.99");
        assert_eq!(row.get::<String>(1).unwrap(), "text");

        let mut rows = conn
            .query("SELECT price, typeof(price) FROM vendor_listings_cache", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "0.3");
        assert_eq!(row.get::<String>(1).unwrap(), "text");

        let mut rows = conn
            .query(
                "SELECT price, typeof(price), currency FROM product_vendor_data",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "48.5");
        assert_eq!(row.get::<String>(1).unwrap(), "text");
        assert_eq!(row.get::<String>(2).unwrap(), "EUR");
    }

    // ── N-8.1: automatic database backups ────────────────────────────────

    /// Migrating a database with pending steps writes a timestamped backup
    /// first, and the backup holds the pre-migration state.
    #[tokio::test]
    async fn backup_on_migrate_writes_timestamped_backup_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();

        // Write something BEFORE any migration runs, so the backup must
        // contain the pre-migration state.
        let conn = db.connect().await.unwrap();
        conn.execute("CREATE TABLE scratch (v TEXT)", ())
            .await
            .unwrap();
        conn.execute("INSERT INTO scratch (v) VALUES ('pre-migration-data')", ())
            .await
            .unwrap();
        drop(conn);

        db.migrate_with_backup(&BackupConfig::default())
            .await
            .unwrap();

        // Exactly one timestamped backup, next to the database.
        let backups = db.list_backups(&BackupConfig::default()).await.unwrap();
        assert_eq!(backups.len(), 1);
        assert!(
            backups[0].name.contains("pre-migration"),
            "unexpected backup name: {}",
            backups[0].name
        );
        assert!(backups[0].path.ends_with(".db"));
        assert!(std::path::Path::new(&backups[0].path).exists());

        // The backup holds the pre-migration state: open it WITHOUT
        // migrating and check the scratch row is there at the old version.
        let snap = Db::open(&backups[0].path).await.unwrap();
        let conn = snap.connect().await.unwrap();
        let mut rows = conn.query("SELECT v FROM scratch", ()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "pre-migration-data");
        let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
        let version: i64 = rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap();
        assert!(
            version < CURRENT_SCHEMA_VERSION,
            "backup should predate the migration, got version {version}"
        );

        // The live database migrated fully.
        let conn = db.connect().await.unwrap();
        let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
        let version: i64 = rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    /// A failed pre-migration backup aborts the migration (fail closed).
    /// Simulate it with a backup directory that cannot be created: point
    /// `dir` at a path blocked by a regular file.
    #[tokio::test]
    async fn migrate_aborts_when_pre_migration_backup_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();

        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        let cfg = BackupConfig {
            dir: Some(blocker.join("backups")),
            ..BackupConfig::default()
        };
        let err = db.migrate_with_backup(&cfg).await.unwrap_err();
        assert!(
            err.to_string().contains("pre-migration backup failed"),
            "unexpected error: {err}"
        );
        // Nothing migrated.
        let conn = db.connect().await.unwrap();
        let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
        let version: i64 = rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap();
        assert_eq!(version, 0);
    }

    /// Backup -> change -> restore -> reopen: the reopened database shows
    /// exactly the backed-up state (the restore round-trip).
    #[tokio::test]
    async fn restore_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();

        db.set_app_setting("backup_probe", "before").await.unwrap();
        let cfg = BackupConfig::default();
        let backup = db.backup_now(&cfg, BackupReason::Manual).await.unwrap();

        // Change state after the backup.
        db.set_app_setting("backup_probe", "after").await.unwrap();

        db.restore_backup(&cfg, &backup).await.unwrap();

        // Simulate the post-restore restart: a fresh handle on the same path
        // must see the backed-up state.
        drop(db);
        let db2 = Db::open(path.to_str().unwrap()).await.unwrap();
        let value = db2.get_app_setting("backup_probe").await.unwrap();
        assert_eq!(value.as_deref(), Some("before"));

        // The restore also left a pre-restore safety snapshot behind.
        let backups = db2.list_backups(&cfg).await.unwrap();
        assert!(
            backups.iter().any(|b| b.reason == "pre-restore"),
            "expected a pre-restore safety backup, got: {:?}",
            backups.iter().map(|b| &b.name).collect::<Vec<_>>()
        );
    }

    /// Pruning keeps the newest N backups and deletes older sets.
    #[tokio::test]
    async fn prune_keeps_last_n_backups() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        let cfg = BackupConfig {
            keep_last: 3,
            ..BackupConfig::default()
        };
        for _ in 0..5 {
            db.backup_now(&cfg, BackupReason::Scheduled).await.unwrap();
        }
        let backups = db.list_backups(&cfg).await.unwrap();
        assert_eq!(backups.len(), 3);
        // Newest first.
        assert!(backups[0].name > backups[1].name);
        assert!(backups[1].name > backups[2].name);
        // Exactly three .db files on disk.
        let db_files = std::fs::read_dir(db.backup_dir(&cfg))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("db"))
            .count();
        assert_eq!(db_files, 3);
    }

    /// A disabled backup config skips the pre-migration backup entirely.
    #[tokio::test]
    async fn disabled_config_skips_migration_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        let cfg = BackupConfig {
            enabled: false,
            ..BackupConfig::default()
        };
        db.migrate_with_backup(&cfg).await.unwrap();
        assert!(!dir.path().join("backups").exists());
        let conn = db.connect().await.unwrap();
        let mut rows = conn.query("PRAGMA user_version", ()).await.unwrap();
        let version: i64 = rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    /// Restore refuses a backup path outside the backup directory.
    #[tokio::test]
    async fn restore_rejects_paths_outside_backup_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Db::open(path.to_str().unwrap()).await.unwrap();
        db.migrate().await.unwrap();
        let cfg = BackupConfig::default();
        let evil = dir.path().join("evil.db");
        std::fs::write(&evil, b"not a database").unwrap();
        let err = db.restore_backup(&cfg, &evil).await.unwrap_err();
        assert!(
            err.to_string().contains("not inside the backup directory"),
            "unexpected error: {err}"
        );
    }
}
