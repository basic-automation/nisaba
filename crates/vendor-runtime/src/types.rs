use serde::{Deserialize, Serialize};

/// A single listing returned by a vendor plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorListing {
    pub vendor_item_id: String,
    pub title: String,
    pub price: Option<f64>,
    pub currency: Option<String>,
    pub quantity: Option<i64>,
    pub sku: Option<String>,
    pub image_url: Option<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub extras: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub group_key: Option<String>,
    #[serde(default)]
    pub variant_attributes: std::collections::HashMap<String, String>,
}

/// The plugin API major version this host implements.
///
/// This is the frozen plugin surface documented in `plugins/template` and
/// `SECURITY.md`: bumping it is a breaking change, and a host refuses to
/// install a plugin whose `api_version` major it does not support.
pub const HOST_PLUGIN_API_VERSION: u32 = 1;

/// Metadata exported by a plugin's `metadata` const.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    pub name: String,
    pub version: String,
    /// Which revision of the frozen plugin surface this plugin targets. A plugin
    /// that does not declare one deserializes to 0, which is not a real revision
    /// and is refused at install — so missing and unknown both fail the same way.
    #[serde(default)]
    pub api_version: u32,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub config_fields: Vec<ConfigField>,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub icon: Option<String>,
    /// Hosts `Nisaba.fetch` may reach — exact names, or `*.example.com` for subdomains.
    /// `None` (not declared) means unrestricted network access, which the UI flags;
    /// `Some(vec![])` means no network at all.
    #[serde(default)]
    pub allowed_hosts: Option<Vec<String>>,
}

impl PluginMetadata {
    /// Refuse a plugin whose API major this host does not implement. The message
    /// is shown to the user at install time, naming both versions so the fix —
    /// a plugin build made for this host's API — is obvious.
    pub fn check_api_version(&self) -> Result<(), String> {
        if self.api_version == HOST_PLUGIN_API_VERSION {
            Ok(())
        } else {
            Err(format!(
                "Plugin '{}' targets plugin API version {}, but this host supports only version {}. \
                 Install a build of the plugin made for API v{}.",
                self.name, self.api_version, HOST_PLUGIN_API_VERSION, HOST_PLUGIN_API_VERSION
            ))
        }
    }
}

fn default_category() -> String {
    "vendor".into()
}

/// A configuration field declared by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigField {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub placeholder: Option<String>,
}

/// Full info about a plugin (metadata + runtime status).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub file_name: String,
    pub metadata: PluginMetadata,
}
