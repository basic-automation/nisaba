use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

use rust_decimal::Decimal;

/// Exact decimal (de)serialization for money fields (N-6.1).
///
/// Serializes as a JSON **number** (not a string) so the Tauri ↔ frontend
/// contract keeps `amount: number`, but the emitted text is the `Decimal`'s
/// exact representation — `0.30`, never `0.30000000000000004`.
/// Deserializes from a JSON number or string, parsing the exact text. A JSON
/// float can only carry what the float holds, so `0.1 + 0.2` computed in
/// floating point still arrives as `0.30000000000000004` — exactness is
/// guaranteed only when the value was decimal all the way down.
pub mod decimal_money {
    use super::Decimal;
    use rust_decimal::prelude::FromPrimitive;
    use serde::{de, Deserializer, Serialize, Serializer};
    use std::str::FromStr;

    pub fn serialize<S>(d: &Decimal, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Emit the exact decimal text as a raw JSON number. Decimal's Display
        // never uses scientific notation, so this is always a valid plain
        // JSON number. serde_json::Number must NOT be used here: its FromStr
        // normalizes ("48.00" -> "48.0") and even routes large values through
        // f64, losing precision.
        let raw = serde_json::value::RawValue::from_string(d.to_string())
            .map_err(serde::ser::Error::custom)?;
        raw.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Decimal, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(MoneyVisitor)
    }

    struct MoneyVisitor;

    impl<'de> de::Visitor<'de> for MoneyVisitor {
        type Value = Decimal;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a decimal money amount, as a JSON number or string")
        }

        fn visit_str<E>(self, v: &str) -> Result<Decimal, E>
        where
            E: de::Error,
        {
            Decimal::from_str(v).map_err(de::Error::custom)
        }

        fn visit_string<E>(self, v: String) -> Result<Decimal, E>
        where
            E: de::Error,
        {
            self.visit_str(&v)
        }

        fn visit_i64<E>(self, v: i64) -> Result<Decimal, E>
        where
            E: de::Error,
        {
            Ok(Decimal::from(v))
        }

        fn visit_u64<E>(self, v: u64) -> Result<Decimal, E>
        where
            E: de::Error,
        {
            Ok(Decimal::from(v))
        }

        fn visit_f64<E>(self, v: f64) -> Result<Decimal, E>
        where
            E: de::Error,
        {
            // Shortest round-trip representation: 0.3 stays 0.3.
            Decimal::from_f64(v).ok_or_else(|| de::Error::custom(format!("{v} is not finite")))
        }
    }

    /// `#[serde(with = "decimal_money::option")]` for `Option<Decimal>` money fields.
    pub mod option {
        use super::{Decimal, MoneyVisitor};
        use serde::{de, Deserializer, Serializer};

        pub fn serialize<S>(opt: &Option<Decimal>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match opt {
                Some(d) => super::serialize(d, serializer),
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Decimal>, D::Error>
        where
            D: Deserializer<'de>,
        {
            struct OptVisitor;

            impl<'de> de::Visitor<'de> for OptVisitor {
                type Value = Option<Decimal>;

                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("an optional decimal money amount")
                }

                fn visit_none<E>(self) -> Result<Option<Decimal>, E>
                where
                    E: de::Error,
                {
                    Ok(None)
                }

                fn visit_unit<E>(self) -> Result<Option<Decimal>, E>
                where
                    E: de::Error,
                {
                    Ok(None)
                }

                fn visit_some<D>(self, deserializer: D) -> Result<Option<Decimal>, D::Error>
                where
                    D: Deserializer<'de>,
                {
                    deserializer.deserialize_any(MoneyVisitor).map(Some)
                }
            }

            deserializer.deserialize_option(OptVisitor)
        }
    }
}

/// Typed product weight (N-7.2): a numeric amount in a named unit, e.g.
/// `{ "value": 1.5, "unit": "lb" }`. Replaces the free-form `weight` string
/// that used to hide in a vendor listing's `extras`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Weight {
    pub value: f64,
    /// Unit of `value`, e.g. `"lb"`, `"oz"`, `"kg"`, `"g"`.
    pub unit: String,
}

/// Typed physical dimensions (N-7.2): all three linear measures share `unit`,
/// e.g. `{ "length": 12, "width": 9, "height": 4, "unit": "in" }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dimensions {
    pub length: f64,
    pub width: f64,
    pub height: f64,
    /// Linear unit of the three measures, e.g. `"in"`, `"cm"`.
    pub unit: String,
}

/// Cached vendor listing from a plugin execution (persisted in DB).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorListingCache {
    pub plugin_id: String,
    pub vendor_item_id: String,
    pub title: String,
    /// Exact decimal amount (N-6.1). Serializes as a JSON number.
    #[serde(with = "decimal_money::option")]
    pub price: Option<Decimal>,
    pub currency: Option<String>,
    /// Vendor stock on hand. `None` = unknown (the vendor gave no stock
    /// figure); `Some(0)` = out of stock. The distinction is preserved from
    /// the plugin all the way here — unknown must never be coerced to 0.
    pub quantity: Option<i64>,
    pub sku: Option<String>,
    /// All known image URLs, most representative first (N-7.2).
    #[serde(default)]
    pub images: Vec<String>,
    /// Legacy pre-N-7.2 single-image column. Read-only: rows written before
    /// the freeze are folded into `images` on read; new rows leave it NULL.
    pub image_url: Option<String>,
    pub url: Option<String>,
    /// UPC / GTIN / EAN. A string so leading zeros survive.
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
    pub extras: HashMap<String, String>,
    pub group_key: Option<String>,
    pub variant_attributes: HashMap<String, String>,
    pub fetched_at: String,
}

impl VendorListingCache {
    /// Fold a legacy single `image_url` into `images` when the typed column
    /// has nothing: rows written before N-7.2 only have the old column.
    pub fn images_resolved(&self) -> Vec<String> {
        if self.images.is_empty() {
            self.image_url.clone().into_iter().collect()
        } else {
            self.images.clone()
        }
    }

    /// First image, for single-image consumers (variant thumbnails, the
    /// frontend's `image_url` field).
    pub fn primary_image(&self) -> Option<&str> {
        self.images
            .first()
            .map(String::as_str)
            .or(self.image_url.as_deref())
    }
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
    /// On-hand stock excluding vendor/dropship contributions. Serialized
    /// since format v2; `#[serde(default)]` is the compat shim for older
    /// payloads that lack it (they import as 0 rather than failing).
    #[serde(default)]
    pub on_hand_quantity: i64,
    pub image_url: Option<String>,
    pub sort_order: i32,
    #[serde(default)]
    pub source_plugin_id: Option<String>,
    #[serde(default)]
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
    /// Soft-delete tombstone, faithfully carried so an export/import
    /// round-trip never revives a deleted mapping as active: `Some(ts)`
    /// means the mapping was deleted at that time and must stay deleted.
    /// Live mappings have `None`. `#[serde(default)]` keeps payloads from
    /// before this field existed (export format < 3, older peers)
    /// deserializable; they read as live mappings.
    #[serde(default)]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformSnapshot {
    pub id: i64,
    pub product_id: String,
    pub platform: Platform,
    /// Which product variant this snapshot belongs to. Snapshots are keyed
    /// per (product, platform, variant) so restock detection and pushes are
    /// variant-aware. `None` (stored as `''`) means the product-level row.
    /// `#[serde(default)]` keeps payloads from older peers deserializable.
    #[serde(default)]
    pub variant_id: Option<String>,
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
    /// Exact decimal amount (N-6.1). Serializes as a JSON number.
    #[serde(with = "decimal_money::option")]
    pub price: Option<Decimal>,
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
///
/// `amount` is an exact decimal (N-6.1): parsing `"0.30"` and serializing it
/// back never produces `0.30000000000000004`. It serializes as a JSON number
/// so the Tauri ↔ frontend contract is unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListingPrice {
    #[serde(with = "decimal_money")]
    pub amount: Decimal,
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
    /// Exact decimal amount (N-6.1). Serializes as a JSON number.
    #[serde(with = "decimal_money")]
    pub amount: Decimal,
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
    /// The plugin's declared fetch allowlist as JSON, computed from its files when it was
    /// imported or installed: `["host", …]`, or `null` when it declares none (so it has
    /// unrestricted network access). `None` means not yet computed from the current files
    /// — e.g. a payload from an older peer — and must not be shown as either.
    #[serde(default)]
    pub allowed_hosts_json: Option<String>,
    /// SHA-256 fingerprint of the plugin's code and fetch allowlist, bound when an admin
    /// last reviewed it (see [`crate::plugins`]). A re-import or P2P merge whose code or
    /// allowlist no longer matches this hash resets the plugin to `pending`. `None` for
    /// rows that predate the binding or were never reviewed.
    #[serde(default)]
    pub code_hash: Option<String>,
}

/// A vendor plugin install (user-local, never syncs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorPluginInstall {
    pub plugin_id: String,
    pub installed: bool,
    pub enabled: bool,
    pub installed_at: String,
    /// This machine's wall-clock limit for one run of the plugin, in minutes. `None`
    /// means the runtime default.
    #[serde(default)]
    pub timeout_minutes: Option<i64>,
    /// This machine's per-plugin auto-sync interval, in minutes (N-7.5). `None`
    /// means the plugin runs on every global sync cycle; `Some(n)` means the
    /// auto-sync path runs it at most once per `n` minutes. Manual runs are
    /// never gated by this.
    #[serde(default)]
    pub sync_interval_minutes: Option<i64>,
}

/// One recorded plugin run (N-7.5): start/end, listings emitted, the error on
/// failure, and the plugin's own log lines captured for that run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRunRecord {
    pub id: i64,
    pub plugin_id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// `running`, `completed`, or `failed`.
    pub status: String,
    pub listings_emitted: i64,
    pub error: Option<String>,
    /// The plugin's `Nisaba.log` lines for this run, oldest first.
    pub log_lines: Vec<String>,
    /// What started the run: `auto`, `manual`, or `fetch`.
    pub trigger_kind: String,
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

/// The kind of a stock-ledger event. Every stock change is recorded as one of
/// these with a unique key; canonical quantity per variant is the sum of its
/// events' deltas (N-4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StockEventKind {
    /// Backfilled starting balance: one per variant, keyed `opening:<variant_id>`.
    Opening,
    /// Units sold on a platform, keyed `sale:<platform>:<order_id>`.
    Sale,
    /// Stock added back (purchase order received, returns, ...).
    Restock,
    /// Manual user adjustment (positive or negative delta).
    Adjustment,
    /// Vendor plugin inventory sync pushed into canonical stock.
    VendorUpdate,
}

impl StockEventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            StockEventKind::Opening => "opening",
            StockEventKind::Sale => "sale",
            StockEventKind::Restock => "restock",
            StockEventKind::Adjustment => "adjustment",
            StockEventKind::VendorUpdate => "vendor_update",
        }
    }

    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s {
            "opening" => Some(StockEventKind::Opening),
            "sale" => Some(StockEventKind::Sale),
            "restock" => Some(StockEventKind::Restock),
            "adjustment" => Some(StockEventKind::Adjustment),
            "vendor_update" => Some(StockEventKind::VendorUpdate),
            _ => None,
        }
    }
}

impl fmt::Display for StockEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One uniquely keyed stock event in the ledger (`stock_ledger` table).
///
/// `event_key` is the idempotency key: sales are keyed by platform + order id,
/// every other kind carries a UUID (or the deterministic `opening:<variant_id>`
/// for backfilled starting balances). Re-recording an event with an existing
/// key is a no-op.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StockEvent {
    pub id: i64,
    pub event_key: String,
    pub product_id: String,
    pub variant_id: String,
    pub kind: StockEventKind,
    pub delta: i64,
    pub platform: Option<String>,
    pub source_order_id: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
}

impl StockEvent {
    /// A sale of `units_sold` on `platform` for `order_id`. Recording the same
    /// order twice is a no-op because the key is deterministic.
    pub fn sale(
        platform: &str,
        order_id: &str,
        product_id: &str,
        variant_id: &str,
        units_sold: i64,
    ) -> Self {
        Self {
            id: 0,
            event_key: format!("sale:{platform}:{order_id}"),
            product_id: product_id.to_string(),
            variant_id: variant_id.to_string(),
            kind: StockEventKind::Sale,
            delta: -units_sold,
            platform: Some(platform.to_string()),
            source_order_id: Some(order_id.to_string()),
            note: None,
            created_at: String::new(),
        }
    }

    /// A restock, manual adjustment or vendor update with a fresh UUID key.
    /// Callers that need a deterministic key (e.g. retries) should set
    /// `event_key` themselves after construction.
    pub fn keyed(kind: StockEventKind, product_id: &str, variant_id: &str, delta: i64) -> Self {
        Self {
            id: 0,
            event_key: uuid::Uuid::new_v4().to_string(),
            product_id: product_id.to_string(),
            variant_id: variant_id.to_string(),
            kind,
            delta,
            platform: None,
            source_order_id: None,
            note: None,
            created_at: String::new(),
        }
    }

    /// The backfilled starting balance for a variant. The migration writes these
    /// as SQL; this constructor is for tests and manual repair.
    pub fn opening(product_id: &str, variant_id: &str, quantity: i64) -> Self {
        Self {
            id: 0,
            event_key: format!("opening:{variant_id}"),
            product_id: product_id.to_string(),
            variant_id: variant_id.to_string(),
            kind: StockEventKind::Opening,
            delta: quantity,
            platform: None,
            source_order_id: None,
            note: None,
            created_at: String::new(),
        }
    }
}

/// The export format version written by this build (N-8.2).
///
/// Format history (all 1.x-era formats stay importable — see
/// [`ExportData::validate_version`]):
/// - **v1**: products, platform mappings, platform snapshots and XMR
///   processed orders. No `product_variants` section (the field defaults
///   to empty; import's fixup rebuilds a default variant per product).
/// - **v2**: adds the `product_variants` section. The importer dropped
///   `on_hand_quantity` and the variant source links on the floor, and
///   mappings carried no `deleted_at`, so soft-deleted mappings came
///   back as active.
/// - **v3** (current): lossless. Import preserves `on_hand_quantity`,
///   `source_plugin_id` / `source_vendor_item_id`, mapping and snapshot
///   ids, snapshot `variant_id`, and mappings carry `deleted_at` so
///   tombstones stay tombstones.
pub const EXPORT_FORMAT_VERSION: u32 = 3;

/// The oldest export format version this build can import.
pub const MIN_EXPORT_FORMAT_VERSION: u32 = 1;

/// A full, lossless export of the catalog database (see
/// [`EXPORT_FORMAT_VERSION`] for the format history). Contains no
/// secrets: products, variants, platform mappings/snapshots and XMR
/// processed-order markers only.
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

impl ExportData {
    /// The compat shim for older exports: every format version from
    /// [`MIN_EXPORT_FORMAT_VERSION`] through [`EXPORT_FORMAT_VERSION`]
    /// is accepted, and serde defaults fill in the sections/fields the
    /// older formats lacked (see the history on
    /// [`EXPORT_FORMAT_VERSION`]). Anything outside that range — a
    /// version-0 file, or one written by a newer build — is rejected so
    /// an import never silently mangles a format it does not understand.
    pub fn validate_version(&self) -> Result<(), String> {
        if (MIN_EXPORT_FORMAT_VERSION..=EXPORT_FORMAT_VERSION).contains(&self.version) {
            Ok(())
        } else {
            Err(format!(
                "Unsupported export version {}. This build imports versions {} through {}.",
                self.version, MIN_EXPORT_FORMAT_VERSION, EXPORT_FORMAT_VERSION
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn export_version_shim_accepts_earlier_1x_and_rejects_the_rest() {
        let data = |version: u32| ExportData {
            version,
            exported_at: String::new(),
            products: Vec::new(),
            product_variants: Vec::new(),
            platform_mappings: Vec::new(),
            platform_snapshots: Vec::new(),
            xmr_processed_orders: Vec::new(),
        };
        for v in MIN_EXPORT_FORMAT_VERSION..=EXPORT_FORMAT_VERSION {
            assert!(data(v).validate_version().is_ok(), "v{v} must import");
        }
        assert!(data(0).validate_version().is_err());
        assert!(data(EXPORT_FORMAT_VERSION + 1).validate_version().is_err());
    }

    #[test]
    fn variant_payload_without_newer_fields_uses_shim_defaults() {
        // Export payloads from before on_hand_quantity / source links
        // were serialized must still deserialize (compat shim), with the
        // missing fields defaulted rather than dropping the variant.
        let v: ProductVariant = serde_json::from_str(
            r#"{"id":"v1","product_id":"p1","sku":"S","name":"N",
                "attributes":{},"quantity":5,"image_url":null,
                "sort_order":0,"created_at":"","updated_at":""}"#,
        )
        .unwrap();
        assert_eq!(v.on_hand_quantity, 0);
        assert!(v.source_plugin_id.is_none());
        assert!(v.source_vendor_item_id.is_none());
    }

    #[test]
    fn decimal_money_never_emits_f64_artifacts() {
        // The motivating case from N-6.1: 0.1 + 0.2 in f64 is
        // 0.30000000000000004. In Decimal it is exactly 0.3, and the
        // serialized form must show that.
        let amount = Decimal::from_str("0.1").unwrap() + Decimal::from_str("0.2").unwrap();
        assert_eq!(amount.to_string(), "0.3");

        let price = ListingPrice {
            amount,
            currency: "USD".to_string(),
        };
        let json = serde_json::to_string(&price).unwrap();
        assert!(
            json.contains("\"amount\":0.3"),
            "price must serialize exactly, got: {json}"
        );
        assert!(
            !json.contains("0.30000000000000004"),
            "f64 artifact leaked into JSON: {json}"
        );
    }

    #[test]
    fn decimal_money_round_trips_through_json_numbers() {
        let price = ListingPrice {
            amount: Decimal::from_str("48.00").unwrap(),
            currency: "EUR".to_string(),
        };
        let json = serde_json::to_string(&price).unwrap();
        assert!(json.contains("\"amount\":48.00"), "got: {json}");

        // Deserializes back from a JSON number...
        let back: ListingPrice = serde_json::from_str(&json).unwrap();
        assert_eq!(back.amount, Decimal::from_str("48.00").unwrap());

        // ...and from a JSON string (some platforms quote amounts).
        let from_str: ListingPrice =
            serde_json::from_str(r#"{"amount":"0.30","currency":"USD"}"#).unwrap();
        assert_eq!(from_str.amount.to_string(), "0.30");
    }

    #[test]
    fn decimal_money_rejects_garbage() {
        let bad: Result<ListingPrice, _> =
            serde_json::from_str(r#"{"amount":"not-a-price","currency":"USD"}"#);
        assert!(bad.is_err());
    }

    #[test]
    fn optional_price_serializes_as_number_or_null() {
        let listing = PlatformListing {
            platform_item_id: "x".to_string(),
            title: "t".to_string(),
            sku: None,
            quantity: 1,
            price: Some(Decimal::from_str("19.99").unwrap()),
            image_url: None,
            group_key: None,
            variant_attributes: HashMap::new(),
        };
        let json = serde_json::to_string(&listing).unwrap();
        assert!(json.contains("\"price\":19.99"), "got: {json}");

        let listing = PlatformListing {
            price: None,
            ..listing
        };
        let json = serde_json::to_string(&listing).unwrap();
        assert!(json.contains("\"price\":null"), "got: {json}");

        let back: PlatformListing = serde_json::from_str(&json).unwrap();
        assert_eq!(back.price, None);
    }
}
