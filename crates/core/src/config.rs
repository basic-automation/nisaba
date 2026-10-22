use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::error::SyncError;

/// A company registered in the app database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanyRegistryEntry {
    pub id: String,
    pub name: String,
    pub role: String,
    pub db_path: String,
    pub created_at: String,
    pub updated_at: String,
    pub logo: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub general: GeneralConfig,
    pub database: DatabaseConfig,
    #[serde(default)]
    pub ebay: EbayConfig,
    #[serde(default)]
    pub squarespace: SquarespaceConfig,
    #[serde(default)]
    pub xmrbazaar: XmrBazaarConfig,
    #[serde(default)]
    pub amazon: AmazonConfig,
    #[serde(default)]
    pub alerts: AlertsConfig,
    #[serde(default)]
    pub updates: UpdatesConfig,
    #[serde(default)]
    pub company: CompanyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_sync_schedule")]
    pub sync_schedule: String,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

/// The schedule used when none is configured: sync every five minutes.
pub const DEFAULT_SYNC_SCHEDULE: &str = "0 */5 * * * *";

/// Interval (in seconds) the sync engine falls back to when the configured
/// schedule cannot be parsed — e.g. a hand-edited `config.toml`. Saving a
/// config with an invalid schedule is rejected instead (see
/// [`parse_sync_interval`]); this fallback only guards startup paths, so a
/// bad stored value can never stop the engine from running.
pub const DEFAULT_SYNC_INTERVAL_SECS: u64 = 300;

fn default_sync_schedule() -> String {
    DEFAULT_SYNC_SCHEDULE.to_string()
}

/// Parse a sync schedule into the interval between sync cycles, in seconds.
///
/// The only supported form is `0 */N * * * *` — a cron expression whose
/// minute field steps every `N` minutes and whose other fields are fixed.
/// Anything else is rejected rather than silently reinterpreted: a cron
/// string the engine does not understand must surface as an error at save
/// time, not as a surprise interval — and `*/0` must never reach the
/// engine, where a zero period panics `tokio::time::interval` and kills
/// the sync task.
pub fn parse_sync_interval(schedule: &str) -> Result<u64, SyncError> {
    let invalid = |reason: &str| {
        SyncError::ConfigError(format!(
            "Invalid sync schedule {schedule:?}: {reason}. \
             Use the form \"0 */N * * * *\" with N a whole number of minutes (at least 1)."
        ))
    };

    let parts: Vec<&str> = schedule.split_whitespace().collect();
    if parts.len() != 6 {
        return Err(invalid("expected 6 cron fields"));
    }
    if parts[0] != "0" {
        return Err(invalid("the seconds field must be 0"));
    }
    let Some(step) = parts[1].strip_prefix("*/") else {
        return Err(invalid("the minute field must be */N"));
    };
    let minutes: u64 = step
        .parse()
        .map_err(|_| invalid("the minute step N is not a whole number"))?;
    if minutes == 0 {
        return Err(invalid("the interval must not be zero"));
    }
    if parts[2..].iter().any(|field| *field != "*") {
        return Err(invalid("the hour, day, month and weekday fields must be *"));
    }
    minutes
        .checked_mul(60)
        .ok_or_else(|| invalid("the interval is too large"))
}

impl GeneralConfig {
    /// The configured sync interval in seconds, or an error describing why
    /// the schedule is not one the sync engine supports.
    pub fn sync_interval_secs(&self) -> Result<u64, SyncError> {
        parse_sync_interval(&self.sync_schedule)
    }
}

fn default_max_retries() -> u32 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_path")]
    pub path: String,
    /// Master switch for automatic database backups (N-8.1). When false, no
    /// backup is written before migrations and the scheduled backup task stays
    /// idle. Explicit "Back up now" from the UI still works.
    #[serde(default = "default_backup_enabled")]
    pub backup_enabled: bool,
    /// How many of the newest backups to keep next to the database.
    #[serde(default = "default_backup_keep_last")]
    pub backup_keep_last: usize,
    /// Hours between scheduled backups. 0 disables the schedule
    /// (pre-migration backups still run while `backup_enabled` is true).
    #[serde(default = "default_backup_interval_hours")]
    pub backup_interval_hours: u64,
}

fn default_backup_enabled() -> bool {
    true
}

fn default_backup_keep_last() -> usize {
    7
}

fn default_backup_interval_hours() -> u64 {
    24
}

fn default_db_path() -> String {
    "./nisaba.db".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EbayConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default = "default_ebay_redirect")]
    pub redirect_uri: String,
    #[serde(default = "default_ebay_env")]
    pub environment: String,
}

fn default_ebay_redirect() -> String {
    "https://localhost:8443/ebay/callback".to_string()
}

fn default_ebay_env() -> String {
    "sandbox".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SquarespaceConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub store_page_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct XmrBazaarConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_xmr_base_url")]
    pub base_url: String,
    /// Monero address used for receiving payments on new listings.
    #[serde(default)]
    pub monero_address: String,
    #[serde(default)]
    pub endpoints: HashMap<String, EndpointConfig>,
}

fn default_xmr_base_url() -> String {
    "https://xmrbazaar.com".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AmazonConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub seller_id: String,
    #[serde(default = "default_amazon_region")]
    pub region: String,
    #[serde(default)]
    pub marketplace_ids: Vec<String>,
}

fn default_amazon_region() -> String {
    "NA".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointConfig {
    pub method: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertsConfig {
    #[serde(default = "default_low_stock_threshold")]
    pub default_low_stock_threshold: i64,
}

impl Default for AlertsConfig {
    fn default() -> Self {
        Self {
            default_low_stock_threshold: default_low_stock_threshold(),
        }
    }
}

fn default_low_stock_threshold() -> i64 {
    5
}

/// Update-check preferences (N-10.1).
///
/// The in-app updater polls the release endpoint at launch and on a timer.
/// That check leaves the machine, so the user can opt out; when disabled no
/// update check is ever issued (a manual "check now" from the update banner
/// still works — it is an explicit user action, not a background poll).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatesConfig {
    #[serde(default = "default_check_for_updates")]
    pub check_for_updates: bool,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self {
            check_for_updates: default_check_for_updates(),
        }
    }
}

fn default_check_for_updates() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanyConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_sync_interval_secs")]
    pub sync_interval_secs: u64,
}

impl Default for CompanyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sync_interval_secs: default_sync_interval_secs(),
        }
    }
}

fn default_sync_interval_secs() -> u64 {
    120
}

impl AppConfig {
    /// Return the Nisaba data directory: `%APPDATA%\nisaba` (Windows) or `~/.config/nisaba` (Unix).
    pub fn data_dir() -> Result<std::path::PathBuf, SyncError> {
        let base = if cfg!(windows) {
            std::env::var("APPDATA")
                .map(std::path::PathBuf::from)
                .map_err(|_| SyncError::ConfigError("%APPDATA% not set".into()))?
        } else {
            dirs_fallback()
                .ok_or_else(|| SyncError::ConfigError("Cannot determine config directory".into()))?
        };
        Ok(base.join("nisaba"))
    }

    /// Return the default config file path inside the data directory.
    pub fn default_config_path() -> Result<std::path::PathBuf, SyncError> {
        Ok(Self::data_dir()?.join("config.toml"))
    }

    /// Resolve which config file to use:
    ///   1. Explicit path (from CLI arg)
    ///   2. `%APPDATA%/nisaba/config.toml` — created with defaults if missing
    pub fn resolve_and_load(
        explicit_path: Option<&str>,
    ) -> Result<(Self, std::path::PathBuf), SyncError> {
        if let Some(p) = explicit_path {
            let path = std::path::PathBuf::from(p);
            if !path.exists() {
                return Err(SyncError::ConfigError(format!(
                    "Config file not found: {}",
                    path.display()
                )));
            }
            let config = Self::load(&path)?;
            return Ok((config, path));
        }

        // Check cwd first
        let cwd_config = std::path::PathBuf::from("config.toml");
        if cwd_config.exists() {
            let config = Self::load(&cwd_config)?;
            return Ok((config, cwd_config));
        }

        // Use appdata directory, create if needed
        let data_dir = Self::data_dir()?;
        let config_path = data_dir.join("config.toml");

        if !config_path.exists() {
            std::fs::create_dir_all(&data_dir).map_err(|e| {
                SyncError::ConfigError(format!("Failed to create {}: {e}", data_dir.display()))
            })?;

            let default_config = Self::default_config();
            default_config.save(&config_path)?;
            eprintln!("Created default config at {}", config_path.display());
        }

        let config = Self::load(&config_path)?;
        Ok((config, config_path))
    }

    /// Load config from a TOML file with environment variable overrides.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, SyncError> {
        let config: AppConfig = Figment::new()
            .merge(Toml::file(path.as_ref()))
            .merge(Env::prefixed("NISABA_").split("__"))
            .extract()
            .map_err(|e| SyncError::ConfigError(e.to_string()))?;
        Ok(config)
    }

    /// Check the config values the rest of the app relies on being sane.
    ///
    /// Currently this validates the sync schedule: it must name an interval
    /// the sync engine actually supports (see [`parse_sync_interval`]).
    /// Loading stays permissive — a hand-edited `config.toml` with a bad
    /// schedule still loads, and the engine falls back to
    /// [`DEFAULT_SYNC_INTERVAL_SECS`] — but saving such a config is
    /// rejected so the bad value is fixed instead of persisted.
    pub fn validate(&self) -> Result<(), SyncError> {
        self.general.sync_interval_secs()?;
        Ok(())
    }

    /// Save current config to a TOML file.
    ///
    /// The config is validated first ([`AppConfig::validate`]); an invalid
    /// config is rejected and nothing is written.
    ///
    /// The write is atomic — the content goes to a temporary file in the same
    /// directory, which is then renamed over the target — so a crash can never
    /// leave a half-written `config.toml` behind. On Unix the new file is
    /// created with `0600` permissions (owner read/write only); renaming over
    /// an existing file replaces its permissions too.
    ///
    /// Values currently sourced from `NISABA_*` environment variables are
    /// never written back: they are stripped from the serialized document
    /// first, so a save cannot persist an env-provided secret (for example
    /// `NISABA_EBAY__CLIENT_SECRET`) to disk in plaintext.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), SyncError> {
        self.validate()?;
        let mut doc =
            toml::Value::try_from(self).map_err(|e| SyncError::ConfigError(e.to_string()))?;
        strip_env_sourced(&mut doc);
        let content =
            toml::to_string_pretty(&doc).map_err(|e| SyncError::ConfigError(e.to_string()))?;
        atomic_write_secure(path.as_ref(), content.as_bytes())
    }

    /// Create a default config. Database path is relative to the data directory.
    pub fn default_config() -> Self {
        let db_path = Self::data_dir()
            .map(|d| d.join("nisaba.db").to_string_lossy().into_owned())
            .unwrap_or_else(|_| "./nisaba.db".into());

        Self {
            general: GeneralConfig {
                sync_schedule: default_sync_schedule(),
                max_retries: default_max_retries(),
            },
            database: DatabaseConfig {
                path: db_path,
                backup_enabled: default_backup_enabled(),
                backup_keep_last: default_backup_keep_last(),
                backup_interval_hours: default_backup_interval_hours(),
            },
            ebay: EbayConfig::default(),
            squarespace: SquarespaceConfig::default(),
            xmrbazaar: XmrBazaarConfig::default(),
            amazon: AmazonConfig::default(),
            alerts: AlertsConfig::default(),
            updates: UpdatesConfig::default(),
            company: CompanyConfig::default(),
        }
    }
}

/// Fallback config dir for non-Windows (no extra dependency needed).
fn dirs_fallback() -> Option<std::path::PathBuf> {
    std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".config"))
                .ok()
        })
}

/// Remove every value currently provided through a `NISABA_*` environment
/// variable from a serialized config document.
///
/// This mirrors the mapping [`AppConfig::load`] uses —
/// `Env::prefixed("NISABA_").split("__")` with figment's default lower-casing:
/// the prefix is stripped, `__` becomes one nesting level, and the key is
/// ASCII-lower-cased, so `NISABA_EBAY__CLIENT_SECRET` addresses
/// `ebay.client_secret`. Variables figment itself would ignore (for example
/// ones producing an empty path segment) are ignored here as well.
fn strip_env_sourced(doc: &mut toml::Value) {
    for path in env_sourced_key_paths() {
        remove_dotted_path(doc, &path);
    }
}

fn env_sourced_key_paths() -> Vec<Vec<String>> {
    std::env::vars_os()
        .filter(|(k, _)| !k.is_empty())
        .filter_map(|(k, _)| {
            let key = k.to_string_lossy();
            let rest = key.trim().strip_prefix("NISABA_")?.trim();
            let dotted = rest.replace("__", ".");
            if dotted.split('.').any(|s| s.is_empty()) {
                return None;
            }
            Some(
                dotted
                    .to_ascii_lowercase()
                    .split('.')
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

fn remove_dotted_path(value: &mut toml::Value, path: &[String]) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut current = value;
    for segment in parents {
        match current {
            toml::Value::Table(table) => match table.get_mut(segment) {
                Some(next) => current = next,
                None => return,
            },
            _ => return,
        }
    }
    if let toml::Value::Table(table) = current {
        table.remove(last);
    }
}

/// Atomically write `content` to `path` with owner-only permissions.
///
/// The bytes go to a temporary file in the same directory (so the final
/// rename is atomic on one filesystem). On Unix the temp file is restricted
/// to `0600` *before* any bytes are written, so secrets are never briefly
/// world-readable, then it is `fsync`ed and renamed over the target. The temp
/// file is removed if any step fails.
fn atomic_write_secure(path: &Path, content: &[u8]) -> Result<(), SyncError> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config.toml");
    let tmp_name = format!(".{file_name}.tmp-{}", std::process::id());
    let tmp_path = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(tmp_name),
        _ => std::path::PathBuf::from(tmp_name),
    };

    let write = (|| {
        let mut file =
            std::fs::File::create(&tmp_path).map_err(|e| SyncError::ConfigError(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| SyncError::ConfigError(e.to_string()))?;
        }
        use std::io::Write;
        file.write_all(content)
            .map_err(|e| SyncError::ConfigError(e.to_string()))?;
        file.sync_all()
            .map_err(|e| SyncError::ConfigError(e.to_string()))?;
        drop(file);
        std::fs::rename(&tmp_path, path).map_err(|e| SyncError::ConfigError(e.to_string()))?;
        Ok::<(), SyncError>(())
    })();

    if write.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    write
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sets an env var for the duration of a test, removing it on drop so a
    /// panic cannot leak it into other tests running in the same process.
    struct EnvGuard {
        key: &'static str,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            std::env::set_var(key, value);
            Self { key }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var(self.key);
        }
    }

    #[test]
    fn test_load_example_config() {
        let config = AppConfig::load("../../config.example.toml").unwrap();
        assert_eq!(config.general.sync_schedule, "0 */5 * * * *");
        assert_eq!(config.general.max_retries, 3);
        assert_eq!(config.database.path, "./nisaba.db");
        assert!(config.ebay.enabled);
        assert!(config.squarespace.enabled);
        assert!(config.xmrbazaar.enabled);
        assert!(!config.amazon.enabled);
        assert_eq!(config.alerts.default_low_stock_threshold, 5);
    }

    #[test]
    fn test_updates_opt_out_defaults_to_on_and_roundtrips() {
        // Update checks are on unless the user explicitly opts out.
        let config = AppConfig::default_config();
        assert!(config.updates.check_for_updates);

        // A config written before the [updates] section existed still loads,
        // with checks on.
        let legacy = "[general]\nsync_schedule = \"0 */5 * * * *\"\n";
        let parsed: AppConfig = toml::from_str(legacy).unwrap();
        assert!(parsed.updates.check_for_updates);

        // Opting out survives a save/load roundtrip.
        let mut opted_out = AppConfig::default_config();
        opted_out.updates.check_for_updates = false;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        opted_out.save(&path).unwrap();
        let reloaded = AppConfig::load(&path).unwrap();
        assert!(!reloaded.updates.check_for_updates);
    }

    #[test]
    fn test_config_roundtrip() {
        let config = AppConfig::load("../../config.example.toml").unwrap();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let _: AppConfig = toml::from_str(&serialized).unwrap();
    }

    #[test]
    fn test_save_does_not_persist_env_sourced_secret() {
        // A secret merged in from NISABA_* at load time must never be written
        // back to config.toml.
        let _guard = EnvGuard::set("NISABA_EBAY__CLIENT_SECRET", "env-supplied-super-secret");
        let config = AppConfig::load("../../config.example.toml").unwrap();
        assert_eq!(config.ebay.client_secret, "env-supplied-super-secret");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        config.save(&path).unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            !written.contains("env-supplied-super-secret"),
            "env-sourced secret was persisted to {}",
            path.display()
        );

        // Loading the saved file still resolves the secret from the
        // environment — stripping only affected what hit the disk.
        let reloaded = AppConfig::load(&path).unwrap();
        assert_eq!(reloaded.ebay.client_secret, "env-supplied-super-secret");
    }

    #[test]
    fn test_save_does_not_persist_env_sourced_non_secret_values() {
        // The rule is not limited to secrets: no NISABA_*-sourced value may be
        // written back.
        let _guard = EnvGuard::set("NISABA_EBAY__ENVIRONMENT", "production");
        let config = AppConfig::load("../../config.example.toml").unwrap();
        assert_eq!(config.ebay.environment, "production");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        config.save(&path).unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            !written.contains("environment = \"production\""),
            "env-sourced value was persisted to {}",
            path.display()
        );
    }

    #[test]
    fn test_save_roundtrip_keeps_user_values() {
        let mut config = AppConfig::load("../../config.example.toml").unwrap();
        config.general.sync_schedule = "0 */10 * * * *".to_string();
        config.alerts.default_low_stock_threshold = 9;
        config.squarespace.api_key = "file-api-key".to_string();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        config.save(&path).unwrap();

        let reloaded = AppConfig::load(&path).unwrap();
        assert_eq!(reloaded.general.sync_schedule, "0 */10 * * * *");
        assert_eq!(reloaded.alerts.default_low_stock_threshold, 9);
        assert_eq!(reloaded.squarespace.api_key, "file-api-key");

        // Atomic write leaves no temp files behind.
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].as_ref().unwrap().file_name().to_str(),
            Some("config.toml")
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_save_creates_file_with_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        AppConfig::default_config().save(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "saved config.toml must be owner-only");
    }

    #[test]
    #[cfg(unix)]
    fn test_save_tightens_existing_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Simulate a config.toml created earlier with lax permissions.
        std::fs::write(&path, "[general]\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        AppConfig::default_config().save(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "re-saving must tighten existing permissions");
    }

    #[test]
    fn test_parse_sync_interval_accepts_supported_forms() {
        assert_eq!(parse_sync_interval("0 */1 * * * *").unwrap(), 60);
        assert_eq!(parse_sync_interval("0 */5 * * * *").unwrap(), 300);
        assert_eq!(parse_sync_interval("0 */10 * * * *").unwrap(), 600);
        assert_eq!(parse_sync_interval("0 */60 * * * *").unwrap(), 3600);
        assert_eq!(parse_sync_interval("0 */1440 * * * *").unwrap(), 86_400);
        // Surrounding and repeated whitespace is fine.
        assert_eq!(parse_sync_interval("  0   */5   * * * * ").unwrap(), 300);
        // The default schedule is a supported form.
        assert_eq!(
            AppConfig::default_config()
                .general
                .sync_interval_secs()
                .unwrap(),
            DEFAULT_SYNC_INTERVAL_SECS
        );
    }

    #[test]
    fn test_parse_sync_interval_rejects_zero() {
        // */0 used to produce a 0 s period that panicked
        // tokio::time::interval and killed the sync engine task.
        let err = parse_sync_interval("0 */0 * * * *").unwrap_err();
        assert!(
            err.to_string().contains("must not be zero"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_parse_sync_interval_rejects_garbage() {
        for schedule in [
            "garbage",
            "",
            "every five minutes",
            "*/5 * * * *",      // missing the seconds field
            "0 */5 * * *",      // too few fields
            "0 */5 * * * * *",  // too many fields
            "5 */5 * * * *",    // seconds field must be 0
            "0 5 * * * *",      // minute field must be */N
            "0 */five * * * *", // N must be a number
            "0 */-5 * * * *",   // N must be unsigned
            "0 */5 2 * * *",    // hour field must be *
            "0 */5 * * * MON",  // weekday field must be *
        ] {
            assert!(
                parse_sync_interval(schedule).is_err(),
                "schedule {schedule:?} must be rejected"
            );
        }
    }

    #[test]
    fn test_save_rejects_invalid_sync_schedule() {
        for schedule in ["0 */0 * * * *", "not a schedule"] {
            let mut config = AppConfig::default_config();
            config.general.sync_schedule = schedule.to_string();

            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            let err = config.save(&path).unwrap_err();
            assert!(
                err.to_string().contains("Invalid sync schedule"),
                "unexpected error for {schedule:?}: {err}"
            );
            assert!(
                !path.exists(),
                "an invalid config must not be written to disk"
            );
        }
    }
}
