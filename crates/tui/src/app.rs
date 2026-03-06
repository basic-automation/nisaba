use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;

use tracing::warn;

use nisaba_core::config::AppConfig;
use nisaba_core::db::Db;
use nisaba_core::sync_engine::{SharedAdapters, SyncCommand, SyncEngineEvent};
use nisaba_core::types::{Platform, PlatformListing, PlatformMapping, Product, ProductAnalytics, ProductVariant, TimeWindow};

use crate::events::AppEvent;

/// Which tab is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Products,
    Inventory,
    Config,
    Logs,
}

impl Tab {
    pub fn index(&self) -> usize {
        match self {
            Tab::Dashboard => 0,
            Tab::Products => 1,
            Tab::Inventory => 2,
            Tab::Config => 3,
            Tab::Logs => 4,
        }
    }

    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Tab::Dashboard,
            1 => Tab::Products,
            2 => Tab::Inventory,
            3 => Tab::Config,
            4 => Tab::Logs,
            _ => Tab::Dashboard,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Tab::Dashboard => "Dashboard",
            Tab::Products => "Products",
            Tab::Inventory => "Inventory",
            Tab::Config => "Config",
            Tab::Logs => "Logs",
        }
    }

    pub fn next(&self) -> Self {
        Self::from_index((self.index() + 1) % 5)
    }
}

/// Sub-view within the Products tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductsView {
    Mapped,
    Unmapped,
}

/// Shared application state.
pub struct App {
    pub running: bool,
    pub tab: Tab,
    pub config: AppConfig,
    pub config_path: std::path::PathBuf,
    pub adapters: SharedAdapters,
    pub db: Arc<Db>,
    pub cmd_tx: mpsc::Sender<SyncCommand>,
    pub app_event_tx: mpsc::Sender<AppEvent>,

    // Dashboard state
    pub products: Vec<Product>,
    pub platform_status: HashMap<Platform, PlatformStatus>,
    pub last_sync: Option<String>,
    pub next_sync: Option<String>,
    pub sync_paused: bool,
    pub dashboard_selected: usize,

    // Products tab state
    pub products_view: ProductsView,
    pub products_selected: usize,
    pub unmapped_count: usize,
    pub search_query: String,
    pub searching: bool,

    // Inventory tab state
    pub inventory_selected: usize,
    pub inventory_time_window: TimeWindow,
    pub analytics: Vec<ProductAnalytics>,
    pub inventory_expanded: bool,

    // Products tab: unmapped listings state
    pub unmapped_listings: HashMap<Platform, Vec<PlatformListing>>,
    pub unmapped_col: usize,         // 0=eBay, 1=Squarespace, 2=XmrBazaar
    pub unmapped_row: usize,         // Selected item in current column
    pub import_popup: bool,          // Import-as-new-product popup visible
    pub import_name: String,         // Editable name for import
    pub import_sku: String,          // Editable SKU for import
    pub import_field: usize,         // 0=name, 1=sku in the import popup
    pub link_mode: bool,             // True when linking to an existing product
    pub link_product_id: Option<String>, // Product being linked to
    pub product_mappings: HashMap<String, Vec<PlatformMapping>>, // Cached mappings per product

    // Config tab state
    pub config_selected: usize,
    pub config_editing: bool,
    pub config_fields: Vec<ConfigField>,

    // Logs tab state
    pub log_scroll: usize,
    pub log_level_filter: Option<String>,

    // Status line
    pub status_message: Option<String>,

    // Refresh flags (set by sync events, acted on in main loop)
    pub needs_product_refresh: bool,
    pub needs_adapter_rebuild: bool,
}

#[derive(Debug, Clone)]
pub struct PlatformStatus {
    pub connected: bool,
    pub last_error: Option<String>,
    pub items_count: usize,
}

#[derive(Debug, Clone)]
pub struct ConfigField {
    pub label: String,
    pub key: String,
    pub value: String,
    pub secret: bool,
}

impl App {
    pub fn new(
        config: AppConfig,
        config_path: std::path::PathBuf,
        adapters: SharedAdapters,
        db: Arc<Db>,
        cmd_tx: mpsc::Sender<SyncCommand>,
        app_event_tx: mpsc::Sender<AppEvent>,
    ) -> Self {
        let config_fields = build_config_fields(&config);

        Self {
            running: true,
            tab: Tab::Dashboard,
            config,
            config_path,
            adapters,
            db,
            cmd_tx,
            app_event_tx,

            products: Vec::new(),
            platform_status: HashMap::new(),
            last_sync: None,
            next_sync: None,
            sync_paused: false,
            dashboard_selected: 0,

            products_view: ProductsView::Mapped,
            products_selected: 0,
            unmapped_count: 0,
            search_query: String::new(),
            searching: false,

            unmapped_listings: HashMap::new(),
            unmapped_col: 0,
            unmapped_row: 0,
            import_popup: false,
            import_name: String::new(),
            import_sku: String::new(),
            import_field: 0,
            link_mode: false,
            link_product_id: None,
            product_mappings: HashMap::new(),

            inventory_selected: 0,
            inventory_time_window: TimeWindow::Day,
            analytics: Vec::new(),
            inventory_expanded: false,

            config_selected: 0,
            config_editing: false,
            config_fields,

            log_scroll: 0,
            log_level_filter: None,

            status_message: None,

            needs_product_refresh: false,
            needs_adapter_rebuild: false,
        }
    }

    pub fn handle_sync_event(&mut self, event: SyncEngineEvent) {
        match event {
            SyncEngineEvent::CycleComplete {
                products_synced,
                changes_pushed,
                ..
            } => {
                self.last_sync = Some(chrono::Local::now().format("%H:%M:%S").to_string());
                self.status_message = Some(format!(
                    "Sync complete: {products_synced} products, {changes_pushed} changes"
                ));
                self.needs_product_refresh = true;
            }
            SyncEngineEvent::QuantityUpdated {
                product_name,
                platform,
                new_quantity,
                ..
            } => {
                self.status_message = Some(format!(
                    "{product_name} → {platform}: qty={new_quantity}"
                ));
            }
            SyncEngineEvent::PlatformError { platform, message } => {
                self.platform_status
                    .entry(platform)
                    .and_modify(|s| {
                        s.connected = false;
                        s.last_error = Some(message.clone());
                    })
                    .or_insert(PlatformStatus {
                        connected: false,
                        last_error: Some(message),
                        items_count: 0,
                    });
            }
            SyncEngineEvent::AuthExpired { platform } => {
                self.status_message = Some(format!("{platform} auth expired!"));
            }
            SyncEngineEvent::UnmappedListingsDetected { count } => {
                self.unmapped_count = count;
            }
        }
    }

    /// Reload products and their mappings from the database.
    pub async fn refresh_products(&mut self) {
        match self.db.list_products().await {
            Ok(products) => self.products = products,
            Err(e) => {
                self.status_message = Some(format!("Failed to load products: {e}"));
            }
        }
        self.refresh_mappings().await;
    }

    /// Reload analytics from the database.
    pub async fn refresh_analytics(&mut self) {
        match nisaba_core::analytics::compute_all_analytics(&self.db, self.inventory_time_window)
            .await
        {
            Ok(analytics) => self.analytics = analytics,
            Err(e) => {
                self.status_message = Some(format!("Failed to compute analytics: {e}"));
            }
        }
    }

    /// Spawn a background task to fetch all listings from enabled platforms.
    /// Results arrive via AppEvent::ListingsFetched.
    pub fn start_fetch_listings(&self) {
        let adapters = self.adapters.clone();
        let db = self.db.clone();
        let tx = self.app_event_tx.clone();

        tokio::spawn(async move {
            let adapters_guard = adapters.read().await;
            let mut all_listings: HashMap<Platform, Vec<PlatformListing>> = HashMap::new();

            for (platform, adapter) in adapters_guard.iter() {
                match adapter.fetch_all_listings().await {
                    Ok(listings) => {
                        all_listings.insert(*platform, listings);
                    }
                    Err(e) => {
                        warn!(platform = %platform, error = %e, "Failed to fetch listings");
                    }
                }
            }

            drop(adapters_guard);

            // Get all existing mappings to filter out already-mapped items
            let existing_mappings = match db.list_all_active_mappings().await {
                Ok(m) => m,
                Err(e) => {
                    let _ = tx.send(AppEvent::ListingsFetchError(
                        format!("Failed to load mappings: {e}")
                    )).await;
                    return;
                }
            };

            // Build a set of (platform, platform_item_id) that are already mapped
            let mapped_set: std::collections::HashSet<(Platform, String)> = existing_mappings
                .iter()
                .map(|m| (m.platform, m.platform_item_id.clone()))
                .collect();

            // Filter out mapped listings
            for (_platform, listings) in &mut all_listings {
                listings.retain(|l| {
                    !mapped_set.contains(&(_platform.to_owned(), l.platform_item_id.clone()))
                });
            }

            let _ = tx.send(AppEvent::ListingsFetched(all_listings)).await;
        });
    }

    /// Handle the results of a background listing fetch.
    pub fn apply_fetched_listings(&mut self, listings: HashMap<Platform, Vec<PlatformListing>>) {
        let total_unmapped: usize = listings.values().map(|v| v.len()).sum();
        self.unmapped_listings = listings;
        self.unmapped_count = total_unmapped;
        self.unmapped_col = 0;
        self.unmapped_row = 0;
        self.status_message = Some(format!("Found {total_unmapped} unmapped listings"));
    }

    /// Create a new canonical product from an unmapped listing and map it.
    pub async fn import_listing_as_product(
        &mut self,
        platform: Platform,
        listing: &PlatformListing,
        name: &str,
        sku: &str,
    ) -> bool {
        let product_id = uuid::Uuid::new_v4().to_string();
        let product = Product {
            id: product_id.clone(),
            canonical_sku: sku.to_string(),
            name: name.to_string(),
            quantity: listing.quantity,
            low_stock_threshold: None,
            is_tracked: true,
            created_at: String::new(),
            updated_at: String::new(),
        };

        if let Err(e) = self.db.insert_product(&product).await {
            self.status_message = Some(format!("Failed to create product: {e}"));
            return false;
        }

        // Every mapping needs a variant — create one from the listing
        let variant_id = uuid::Uuid::new_v4().to_string();
        let variant = ProductVariant {
            id: variant_id.clone(),
            product_id: product_id.clone(),
            sku: listing.sku.clone().unwrap_or_else(|| listing.platform_item_id.clone()),
            name: name.to_string(),
            attributes: std::collections::HashMap::new(),
            quantity: listing.quantity,
            on_hand_quantity: listing.quantity,
            image_url: listing.image_url.clone(),
            sort_order: 0,
            source_plugin_id: None,
            source_vendor_item_id: None,
            created_at: String::new(),
            updated_at: String::new(),
        };

        if let Err(e) = self.db.insert_variant(&variant).await {
            self.status_message = Some(format!("Product created but variant failed: {e}"));
            return false;
        }

        if let Err(e) = self
            .db
            .insert_mapping(
                &product_id,
                platform.as_str(),
                &listing.platform_item_id,
                listing.sku.as_deref(),
                &variant_id,
            )
            .await
        {
            self.status_message = Some(format!("Product created but mapping failed: {e}"));
            return false;
        }

        // Remove from unmapped list
        if let Some(list) = self.unmapped_listings.get_mut(&platform) {
            list.retain(|l| l.platform_item_id != listing.platform_item_id);
        }

        // Update counts
        self.unmapped_count = self
            .unmapped_listings
            .values()
            .map(|v| v.len())
            .sum();

        self.status_message = Some(format!("Created product '{}' with {} mapping", name, platform));
        self.needs_product_refresh = true;
        true
    }

    /// Link an unmapped listing to an existing product.
    /// Tries to match an existing variant by SKU, creates one if not found.
    pub async fn link_listing_to_product(
        &mut self,
        product_id: &str,
        platform: Platform,
        listing: &PlatformListing,
    ) -> bool {
        // Try to match an existing variant by SKU
        let listing_sku = listing.sku.clone().unwrap_or_else(|| listing.platform_item_id.clone());
        let variant_id = match self.db.list_variants_for_product(product_id).await {
            Ok(variants) => {
                if let Some(v) = variants.iter().find(|v| v.sku.eq_ignore_ascii_case(&listing_sku)) {
                    v.id.clone()
                } else {
                    // No match — create a new variant
                    let vid = uuid::Uuid::new_v4().to_string();
                    let variant = ProductVariant {
                        id: vid.clone(),
                        product_id: product_id.to_string(),
                        sku: listing_sku,
                        name: listing.title.clone(),
                        attributes: std::collections::HashMap::new(),
                        quantity: listing.quantity,
                        on_hand_quantity: listing.quantity,
                        image_url: listing.image_url.clone(),
                        sort_order: 0,
                        source_plugin_id: None,
                        source_vendor_item_id: None,
                        created_at: String::new(),
                        updated_at: String::new(),
                    };
                    if let Err(e) = self.db.insert_variant(&variant).await {
                        self.status_message = Some(format!("Failed to create variant: {e}"));
                        return false;
                    }
                    vid
                }
            }
            Err(e) => {
                self.status_message = Some(format!("Failed to list variants: {e}"));
                return false;
            }
        };

        if let Err(e) = self
            .db
            .insert_mapping(
                product_id,
                platform.as_str(),
                &listing.platform_item_id,
                listing.sku.as_deref(),
                &variant_id,
            )
            .await
        {
            self.status_message = Some(format!("Failed to create mapping: {e}"));
            return false;
        }

        // Remove from unmapped list
        if let Some(list) = self.unmapped_listings.get_mut(&platform) {
            list.retain(|l| l.platform_item_id != listing.platform_item_id);
        }

        self.unmapped_count = self
            .unmapped_listings
            .values()
            .map(|v| v.len())
            .sum();

        self.status_message = Some(format!("Linked listing to product on {platform}"));
        self.needs_product_refresh = true;
        true
    }

    /// Refresh cached product mappings from the database.
    pub async fn refresh_mappings(&mut self) {
        match self.db.list_all_active_mappings().await {
            Ok(mappings) => {
                self.product_mappings.clear();
                for m in mappings {
                    self.product_mappings
                        .entry(m.product_id.clone())
                        .or_insert_with(Vec::new)
                        .push(m);
                }
            }
            Err(e) => {
                self.status_message = Some(format!("Failed to load mappings: {e}"));
            }
        }
    }

    /// Get the platform columns in a fixed order for the unmapped view.
    pub fn platform_columns(&self) -> [Platform; 3] {
        [Platform::Ebay, Platform::Squarespace, Platform::XmrBazaar]
    }

    /// Get listings for the currently selected column.
    pub fn current_column_listings(&self) -> &[PlatformListing] {
        let platforms = self.platform_columns();
        if self.unmapped_col < platforms.len() {
            let platform = platforms[self.unmapped_col];
            self.unmapped_listings
                .get(&platform)
                .map(|v| v.as_slice())
                .unwrap_or(&[])
        } else {
            &[]
        }
    }

    pub fn filtered_products(&self) -> Vec<&Product> {
        if self.search_query.is_empty() {
            self.products.iter().collect()
        } else {
            let q = self.search_query.to_lowercase();
            self.products
                .iter()
                .filter(|p| {
                    p.name.to_lowercase().contains(&q)
                        || p.canonical_sku.to_lowercase().contains(&q)
                })
                .collect()
        }
    }
}

fn build_config_fields(config: &AppConfig) -> Vec<ConfigField> {
    vec![
        ConfigField {
            label: "Sync Schedule".into(),
            key: "general.sync_schedule".into(),
            value: config.general.sync_schedule.clone(),
            secret: false,
        },
        ConfigField {
            label: "Log Level".into(),
            key: "general.log_level".into(),
            value: config.general.log_level.clone(),
            secret: false,
        },
        ConfigField {
            label: "Database Path".into(),
            key: "database.path".into(),
            value: config.database.path.clone(),
            secret: false,
        },
        ConfigField {
            label: "eBay Enabled".into(),
            key: "ebay.enabled".into(),
            value: config.ebay.enabled.to_string(),
            secret: false,
        },
        ConfigField {
            label: "eBay Client ID".into(),
            key: "ebay.client_id".into(),
            value: config.ebay.client_id.clone(),
            secret: true,
        },
        ConfigField {
            label: "eBay Environment".into(),
            key: "ebay.environment".into(),
            value: config.ebay.environment.clone(),
            secret: false,
        },
        ConfigField {
            label: "Squarespace Enabled".into(),
            key: "squarespace.enabled".into(),
            value: config.squarespace.enabled.to_string(),
            secret: false,
        },
        ConfigField {
            label: "Squarespace API Key".into(),
            key: "squarespace.api_key".into(),
            value: config.squarespace.api_key.clone(),
            secret: true,
        },
        ConfigField {
            label: "XMR Bazaar Enabled".into(),
            key: "xmrbazaar.enabled".into(),
            value: config.xmrbazaar.enabled.to_string(),
            secret: false,
        },
        ConfigField {
            label: "XMR Bazaar Username".into(),
            key: "xmrbazaar.username".into(),
            value: config.xmrbazaar.username.clone(),
            secret: false,
        },
        ConfigField {
            label: "Low Stock Threshold".into(),
            key: "alerts.default_low_stock_threshold".into(),
            value: config.alerts.default_low_stock_threshold.to_string(),
            secret: false,
        },
    ]
}
