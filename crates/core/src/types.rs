use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Cached vendor listing from a plugin execution (persisted in DB).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorListingCache {
    pub plugin_id: String,
    pub vendor_item_id: String,
    pub title: String,
    pub price: Option<f64>,
    pub currency: Option<String>,
    pub quantity: Option<i64>,
    pub sku: Option<String>,
    pub image_url: Option<String>,
    pub url: Option<String>,
    pub extras: HashMap<String, String>,
    pub group_key: Option<String>,
    pub variant_attributes: HashMap<String, String>,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Ebay,
    Squarespace,
    XmrBazaar,
    Amazon,
}

impl Platform {
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Ebay => "ebay",
            Platform::Squarespace => "squarespace",
            Platform::XmrBazaar => "xmrbazaar",
            Platform::Amazon => "amazon",
        }
    }

    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "ebay" => Some(Platform::Ebay),
            "squarespace" => Some(Platform::Squarespace),
            "xmrbazaar" | "xmr_bazaar" => Some(Platform::XmrBazaar),
            "amazon" => Some(Platform::Amazon),
            _ => None,
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Product {
    pub id: String,
    pub canonical_sku: String,
    pub name: String,
    pub quantity: i64,
    pub low_stock_threshold: Option<i64>,
    pub is_tracked: bool,
    pub has_variants: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductVariant {
    pub id: String,
    pub product_id: String,
    pub sku: String,
    pub name: String,
    pub attributes: HashMap<String, String>,
    pub quantity: i64,
    pub on_hand_quantity: i64,
    pub image_url: Option<String>,
    pub sort_order: i32,
    pub source_plugin_id: Option<String>,
    pub source_vendor_item_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformMapping {
    pub id: i64,
    pub product_id: String,
    pub platform: Platform,
    pub platform_item_id: String,
    pub platform_sku: Option<String>,
    pub is_active: bool,
    pub variant_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformSnapshot {
    pub id: i64,
    pub product_id: String,
    pub platform: Platform,
    pub last_known_quantity: i64,
    pub last_polled_at: String,
    pub last_pushed_at: Option<String>,
    pub version_tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEvent {
    pub id: i64,
    pub product_id: String,
    pub source_platform: String,
    pub target_platform: String,
    pub old_quantity: i64,
    pub new_quantity: i64,
    pub event_type: String,
    pub sync_cycle_id: String,
    pub error_message: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthToken {
    pub platform: Platform,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
    pub cookies: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryHistoryEntry {
    pub id: i64,
    pub product_id: String,
    pub quantity: i64,
    pub sync_cycle_id: String,
    pub recorded_at: String,
}

/// Item returned by a platform adapter when fetching inventory for mapped items.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformInventoryItem {
    pub platform_item_id: String,
    pub quantity: i64,
}

/// Full listing info returned by a platform adapter — used by the product mapping UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformListing {
    pub platform_item_id: String,
    pub title: String,
    pub sku: Option<String>,
    pub quantity: i64,
    pub price: Option<f64>,
    pub image_url: Option<String>,
    pub group_key: Option<String>,
    #[serde(default)]
    pub variant_attributes: HashMap<String, String>,
}

/// A delta detected during a sync cycle for one product on one platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantityDelta {
    pub product_id: String,
    pub platform: Platform,
    pub previous: i64,
    pub current: i64,
    pub delta: i64,
}

/// Resolved outcome for a single product after summing all platform deltas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedQuantity {
    pub product_id: String,
    pub old_canonical: i64,
    pub new_canonical: i64,
    pub total_delta: i64,
    pub deltas: Vec<QuantityDelta>,
}

/// Per-product analytics computed from inventory_history.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProductAnalytics {
    pub product_id: String,
    pub product_name: String,
    pub current_quantity: i64,
    pub low_stock_threshold: Option<i64>,
    pub history: Vec<(String, i64)>, // (timestamp, quantity)
    pub sales_velocity: f64,         // units sold per day
    pub platform_sales: Vec<(Platform, i64)>,
}

/// Time window for analytics queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeWindow {
    Day,
    Week,
    Month,
    All,
}

impl TimeWindow {
    pub fn next(self) -> Self {
        match self {
            TimeWindow::Day => TimeWindow::Week,
            TimeWindow::Week => TimeWindow::Month,
            TimeWindow::Month => TimeWindow::All,
            TimeWindow::All => TimeWindow::Day,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            TimeWindow::Day => "24h",
            TimeWindow::Week => "7d",
            TimeWindow::Month => "30d",
            TimeWindow::All => "All",
        }
    }

    pub fn sql_interval(&self) -> Option<&'static str> {
        match self {
            TimeWindow::Day => Some("-1 day"),
            TimeWindow::Week => Some("-7 days"),
            TimeWindow::Month => Some("-30 days"),
            TimeWindow::All => None,
        }
    }
}

/// Description content fetched from a platform listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListingDescription {
    pub platform: Platform,
    pub platform_item_id: String,
    pub html: Option<String>,
    pub plain_text: Option<String>,
    pub fetched_at: String,
}

/// A photo attached to a platform listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListingPhoto {
    pub url: String,
    pub position: i32,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub alt_text: Option<String>,
}

/// Price information for a listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListingPrice {
    pub amount: f64,
    pub currency: String, // ISO 4217
}

/// Complete listing data fetched from a platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullListing {
    pub platform: Platform,
    pub platform_item_id: String,
    pub title: String,
    pub sku: Option<String>,
    pub quantity: i64,
    pub price: Option<ListingPrice>,
    pub description: Option<ListingDescription>,
    pub photos: Vec<ListingPhoto>,
    pub url: Option<String>,
    pub fetched_at: String,
    #[serde(default)]
    pub extras: std::collections::HashMap<String, String>,
    /// Which product variant this listing is associated with (populated from mapping).
    #[serde(default)]
    pub variant_id: Option<String>,
}

/// Request to create a new listing on a platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateListingRequest {
    pub title: String,
    pub description_html: Option<String>,
    pub sku: Option<String>,
    pub quantity: i64,
    pub price: Option<ListingPrice>,
    pub photo_urls: Vec<String>,
    /// Platform-specific key-value overrides (e.g. XMR Bazaar form fields).
    #[serde(default)]
    pub extras: std::collections::HashMap<String, String>,
}

/// Request to update an existing listing on a platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateListingRequest {
    pub title: Option<String>,
    pub description_html: Option<String>,
    pub price: Option<ListingPrice>,
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub extras: Option<std::collections::HashMap<String, String>>,
}

/// A pricing snapshot recorded for analytics/tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingSnapshot {
    pub id: i64,
    pub product_id: String,
    pub platform: Platform,
    pub amount: f64,
    pub currency: String,
    pub recorded_at: String,
}

/// A sale detected on a platform that uses stock-mode inventory (e.g. XMR Bazaar).
/// Instead of polling numeric quantities, these platforms report sales via scraping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaleDetection {
    pub platform_item_id: String,
    pub units_sold: i64,
    pub order_id: Option<String>,
    pub sold_at: Option<String>,
}

/// A vendor plugin row from the registry table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorPluginRow {
    pub id: String,
    pub plugin_file: String,
    pub display_name: String,
    pub description: String,
    pub files_json: String,
    pub version: String,
    pub config_fields: Option<String>,
    pub config_json: Option<String>,
    pub status: String,
    pub submitted_by: Option<String>,
    pub approved_by: Option<String>,
    pub category: String,
    pub icon: Option<String>,
    pub include_vendor_stock: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// A vendor plugin install (user-local, never syncs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorPluginInstall {
    pub plugin_id: String,
    pub installed: bool,
    pub enabled: bool,
    pub installed_at: String,
}

/// Flags indicating which capabilities a platform adapter supports.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlatformCapabilities {
    pub can_fetch_full_listing: bool,
    pub can_fetch_description: bool,
    pub can_fetch_photos: bool,
    pub can_fetch_price: bool,
    pub can_set_price: bool,
    pub can_set_description: bool,
    pub can_upload_photos: bool,
    pub can_create_listing: bool,
    /// Platform uses stock-mode (unlimited/one-time) instead of numeric quantities.
    /// When true, the sync engine uses detect_sales() instead of polling quantities,
    /// and set_quantity() toggles between active/inactive stock modes.
    pub has_stock_mode_inventory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XmrProcessedOrder {
    pub order_id: String,
    pub product_id: Option<String>,
    pub listing_id: String,
    pub quantity: i64,
    pub processed_at: String,
}

/// Parameters for inserting a new sync event (avoids too-many-arguments on the DB method).
pub struct NewSyncEvent<'a> {
    pub product_id: &'a str,
    pub source_platform: &'a str,
    pub target_platform: &'a str,
    pub old_quantity: i64,
    pub new_quantity: i64,
    pub event_type: &'a str,
    pub sync_cycle_id: &'a str,
    pub error_message: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportData {
    pub version: u32,
    pub exported_at: String,
    pub products: Vec<Product>,
    #[serde(default)]
    pub product_variants: Vec<ProductVariant>,
    pub platform_mappings: Vec<PlatformMapping>,
    pub platform_snapshots: Vec<PlatformSnapshot>,
    pub xmr_processed_orders: Vec<XmrProcessedOrder>,
}
