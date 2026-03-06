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

/// Metadata exported by a plugin's `metadata` const.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub config_fields: Vec<ConfigField>,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub icon: Option<String>,
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
