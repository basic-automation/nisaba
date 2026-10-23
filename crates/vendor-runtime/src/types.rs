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
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(from = "VendorListingWire")]
pub struct VendorListing {
    /// Stable and unique within this vendor — how Nisaba recognises the item
    /// on the next run.
    pub vendor_item_id: String,
    pub title: String,
    /// Exact decimal amount as a string, e.g. `"19.99"` — never binary
    /// floating point. A JSON number is still accepted for plugins written
    /// before the v1 freeze, but new plugins must send the decimal text.
    /// `currency` is required whenever a price is present.
    #[serde(with = "decimal_money::option")]
    #[ts(type = "string | number | null", optional)]
    pub price: Option<Decimal>,
    /// ISO 4217 code, e.g. `"USD"`. Required whenever `price` is present —
    /// the host refuses the listing otherwise.
    #[ts(optional = nullable)]
    pub currency: Option<String>,
    /// Vendor stock on hand. `null` (or omitted) means UNKNOWN — the vendor
    /// gave no stock figure, and Nisaba must not treat it as 0. `0` means OUT
    /// OF STOCK. The import layer preserves the distinction.
    #[ts(type = "number | null", optional)]
    pub quantity: Option<i64>,
    #[ts(optional = nullable)]
    pub sku: Option<String>,
    /// Every known image URL, most representative first. A legacy `image_url`
    /// in a plugin's JSON is folded into `images` (first, unless already
    /// listed), so old plugins keep working; new plugins must send `images`.
    #[serde(default)]
    pub images: Vec<String>,
    /// The item's page on the vendor's site.
    #[ts(optional = nullable)]
    pub url: Option<String>,
    /// UPC / GTIN / EAN — a string so leading zeros survive.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub upc: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub brand: Option<String>,
    /// Manufacturer part number.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub mpn: Option<String>,
    /// Typed weight, e.g. `{ "value": 1.5, "unit": "lb" }`.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub weight: Option<Weight>,
    /// Typed dimensions; all three share `unit`, e.g.
    /// `{ "length": 12, "width": 9, "height": 4, "unit": "in" }`.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub dimensions: Option<Dimensions>,
    /// Free-form strings shown on the vendor card. Nisaba recognises
    /// `description`, `msrp`, `dealer_price` and `map_price`. UPC, weight,
    /// brand, MPN and dimensions are typed fields now — do not also put them
    /// here.
    #[serde(default)]
    pub extras: std::collections::HashMap<String, String>,
    /// Listings sharing a `group_key` are variants of one product, told apart
    /// by `variant_attributes`.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub group_key: Option<String>,
    /// Variant discriminators, e.g. `{ "Size": "L", "Color": "Olive" }`.
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

/// What the host hands a plugin's `fetchListings` as its second argument
/// (N-7.4, incremental sync).
///
/// `since` is the UTC timestamp (`"%Y-%m-%d %H:%M:%S"`) of the last successful
/// sync — `None` on the first run, when the plugin should return the whole
/// catalog. `cursor` is the opaque cursor the plugin returned on the previous
/// run — `None` when the plugin never returned one (or never ran). A plugin
/// may ignore both and keep returning the full catalog; the host then keeps
/// full-pull semantics.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
pub struct FetchListingsArgs {
    /// UTC timestamp (`"%Y-%m-%d %H:%M:%S"`) of the last successful sync —
    /// `null` on the first run, when the plugin should return the whole
    /// catalog.
    #[ts(optional = nullable)]
    pub since: Option<String>,
    /// Opaque cursor the plugin returned on the previous run — `null` when
    /// the plugin never returned one (or never ran).
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
}

/// The delta a plugin may return from `fetchListings` instead of the full
/// catalog: only what changed since `since`/`cursor`, the ids that disappeared,
/// and the cursor for the next run. The host merges it into the cached catalog:
/// `changes` are upserted by `vendor_item_id`, `deleted` ids are removed, and
/// everything else keeps its cached row.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
pub struct ListingDelta {
    /// Only the listings that are new or changed since `since`/`cursor`.
    /// Upserted by `vendor_item_id`.
    pub changes: Vec<VendorListing>,
    /// `vendor_item_id`s that no longer exist at the supplier. Removed from
    /// the cached catalog.
    #[serde(default)]
    pub deleted: Vec<String>,
    /// Opaque cursor for the next run — a page token, an ETag, a timestamp.
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
}

/// What a plugin's `fetchListings` may return.
///
/// An array keeps the pre-N-7.4 full-pull semantics — plugins that ignore
/// `{ since, cursor }` need no changes, and the host replaces the cached
/// catalog with it. An object is a [`ListingDelta`] and is merged into the
/// cached catalog instead.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(untagged)]
pub enum FetchListingsResult {
    /// Full catalog — replaces the cached catalog (pre-N-7.4 semantics).
    Full(Vec<VendorListing>),
    /// Incremental delta — merged into the cached catalog.
    Delta(ListingDelta),
}

/// The plugin API major version this host implements.
///
/// This is the frozen plugin surface documented in `plugins/template` and
/// `SECURITY.md`: bumping it is a breaking change, and a host refuses to
/// install a plugin whose `api_version` major it does not support.
pub const HOST_PLUGIN_API_VERSION: u32 = 1;

/// Metadata exported by a plugin's `metadata` const.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
pub struct PluginMetadata {
    /// Shown in the app; also how re-imports are matched.
    pub name: String,
    /// Bump it to publish an update.
    pub version: String,
    /// Which revision of the frozen plugin surface this plugin targets. A plugin
    /// that does not declare one deserializes to 0, which is not a real revision
    /// and is refused at install — so missing and unknown both fail the same way.
    #[serde(default)]
    pub api_version: u32,
    /// Shown on the plugin's card in the marketplace.
    #[serde(default)]
    pub description: String,
    /// Each field becomes an input on the plugin's Configure dialog; values
    /// arrive in `config` keyed by `key`. `secret: true` masks the input.
    #[serde(default)]
    pub config_fields: Vec<ConfigField>,
    #[serde(default = "default_category")]
    pub category: String,
    /// Optional icon shown on the plugin's card.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub icon: Option<String>,
    /// Hosts `Nisaba.fetch` may reach — exact names, or `*.example.com` for subdomains.
    /// `None` (not declared) means unrestricted network access, which the UI flags;
    /// `Some(vec![])` means no network at all.
    #[serde(default)]
    #[ts(optional = nullable)]
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
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
pub struct ConfigField {
    /// Key under which the value arrives in `config`.
    pub key: String,
    /// Label shown on the Configure dialog.
    pub label: String,
    #[serde(default)]
    pub required: bool,
    /// When true, the input is masked in the UI.
    #[serde(default)]
    pub secret: bool,
    /// Hint text shown inside the input, e.g. where to find the value.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub placeholder: Option<String>,
}

/// Full info about a plugin (metadata + runtime status).
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
pub struct PluginInfo {
    pub file_name: String,
    pub metadata: PluginMetadata,
}
