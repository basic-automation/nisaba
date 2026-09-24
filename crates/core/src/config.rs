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
    pub company: CompanyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_sync_schedule")]
    pub sync_schedule: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

fn default_sync_schedule() -> String {
    "0 */5 * * * *".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_max_retries() -> u32 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_path")]
    pub path: String,
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

    /// Save current config to a TOML file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), SyncError> {
        let content =
            toml::to_string_pretty(self).map_err(|e| SyncError::ConfigError(e.to_string()))?;
        std::fs::write(path.as_ref(), content)
            .map_err(|e| SyncError::ConfigError(e.to_string()))?;
        Ok(())
    }

    /// Create a default config. Database path is relative to the data directory.
    pub fn default_config() -> Self {
        let db_path = Self::data_dir()
            .map(|d| d.join("nisaba.db").to_string_lossy().into_owned())
            .unwrap_or_else(|_| "./nisaba.db".into());

        Self {
            general: GeneralConfig {
                sync_schedule: default_sync_schedule(),
                log_level: default_log_level(),
                max_retries: default_max_retries(),
            },
            database: DatabaseConfig { path: db_path },
            ebay: EbayConfig::default(),
            squarespace: SquarespaceConfig::default(),
            xmrbazaar: XmrBazaarConfig::default(),
            amazon: AmazonConfig::default(),
            alerts: AlertsConfig::default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_example_config() {
        let config = AppConfig::load("../../config.example.toml").unwrap();
        assert_eq!(config.general.sync_schedule, "0 */5 * * * *");
        assert_eq!(config.general.log_level, "info");
        assert_eq!(config.general.max_retries, 3);
        assert_eq!(config.database.path, "./nisaba.db");
        assert!(config.ebay.enabled);
        assert!(config.squarespace.enabled);
        assert!(config.xmrbazaar.enabled);
        assert!(!config.amazon.enabled);
        assert_eq!(config.alerts.default_low_stock_threshold, 5);
    }

    #[test]
    fn test_config_roundtrip() {
        let config = AppConfig::load("../../config.example.toml").unwrap();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let _: AppConfig = toml::from_str(&serialized).unwrap();
    }
}
