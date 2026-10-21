use serde::{Deserialize, Serialize};

use nisaba_core::types::{decimal_money, Dimensions, Weight};
use rust_decimal::Decimal;

/// A single listing returned by a vendor plugin (plugin API v1, frozen by N-7.2).
///
/// `price` is an exact decimal: plugins send the decimal text (`"19.99"`), and
/// the host also accepts a JSON number for plugins written before the freeze
/// (converted with the shortest round-trip, so `19.99` stays `19.99`).
/// `currency` is required whenever a price is present — enforced at the core
/// boundary, not here, so this type stays a pure data carrier.
///
/// `quantity`: `None` means the vendor gave no stock figure (unknown);
/// `Some(0)` means out of stock. The import layer preserves the distinction.
///
/// `images` replaced the pre-freeze single `image_url`: a legacy `image_url`
/// in a plugin's JSON is folded into `images` (first, unless already listed),
/// so old plugins keep working. New plugins must send `images`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "VendorListingWire")]
pub struct VendorListing {
    pub vendor_item_id: String,
    pub title: String,
    #[serde(with = "decimal_money::option")]
    pub price: Option<Decimal>,
    pub currency: Option<String>,
    pub quantity: Option<i64>,
    pub sku: Option<String>,
    #[serde(default)]
    pub images: Vec<String>,
    pub url: Option<String>,
    /// UPC / GTIN / EAN — a string so leading zeros survive.
    #[serde(default)]
    pub upc: Option<String>,
    #[serde(default)]
    pub brand: Option<String>,
    /// Manufacturer part number.
    #[serde(default)]
    pub mpn: Option<String>,
    #[serde(default)]
    pub weight: Option<Weight>,
    #[serde(default)]
    pub dimensions: Option<Dimensions>,
    #[serde(default)]
    pub extras: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub group_key: Option<String>,
    #[serde(default)]
    pub variant_attributes: std::collections::HashMap<String, String>,
}

/// The wire shape `VendorListing` deserializes from: the frozen v1 fields plus
/// the legacy `image_url`, which [`VendorListing`] folds into `images`.
#[derive(Debug, Deserialize)]
struct VendorListingWire {
    vendor_item_id: String,
    title: String,
    #[serde(default, with = "decimal_money::option")]
    price: Option<Decimal>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    quantity: Option<i64>,
    #[serde(default)]
    sku: Option<String>,
    #[serde(default)]
    images: Vec<String>,
    /// Pre-freeze single image URL. Folded into `images` on the way in;
    /// never serialized back out.
    #[serde(default)]
    image_url: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    upc: Option<String>,
    #[serde(default)]
    brand: Option<String>,
    #[serde(default)]
    mpn: Option<String>,
    #[serde(default)]
    weight: Option<Weight>,
    #[serde(default)]
    dimensions: Option<Dimensions>,
    #[serde(default)]
    extras: std::collections::HashMap<String, String>,
    #[serde(default)]
    group_key: Option<String>,
    #[serde(default)]
    variant_attributes: std::collections::HashMap<String, String>,
}

impl From<VendorListingWire> for VendorListing {
    fn from(w: VendorListingWire) -> Self {
        let mut images = w.images;
        if let Some(url) = w.image_url {
            if !images.iter().any(|u| u == &url) {
                images.insert(0, url);
            }
        }
        VendorListing {
            vendor_item_id: w.vendor_item_id,
            title: w.title,
            price: w.price,
            currency: w.currency,
            quantity: w.quantity,
            sku: w.sku,
            images,
            url: w.url,
            upc: w.upc,
            brand: w.brand,
            mpn: w.mpn,
            weight: w.weight,
            dimensions: w.dimensions,
            extras: w.extras,
            group_key: w.group_key,
            variant_attributes: w.variant_attributes,
        }
    }
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
