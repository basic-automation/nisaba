use std::any::Any;

use async_trait::async_trait;

use crate::error::SyncError;
use crate::types::{
    CreateListingRequest, FullListing, ListingDescription, ListingPhoto, ListingPrice, Platform,
    PlatformCapabilities, PlatformInventoryItem, PlatformListing, SaleDetection,
    UpdateListingRequest,
};

#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    /// Downcast support for accessing concrete adapter types.
    fn as_any(&self) -> &dyn Any;
    /// Which platform this adapter handles.
    fn platform(&self) -> Platform;

    /// Fetch current quantities for all mapped items (used by sync engine).
    async fn fetch_inventory(&self) -> Result<Vec<PlatformInventoryItem>, SyncError>;

    /// Fetch ALL listings from the platform (used by product mapping / import).
    async fn fetch_all_listings(&self) -> Result<Vec<PlatformListing>, SyncError>;

    /// Set the quantity for a single item on this platform.
    async fn set_quantity(&self, platform_item_id: &str, qty: i64) -> Result<(), SyncError>;

    /// Check whether current auth credentials are valid.
    async fn is_authenticated(&self) -> bool;

    /// Attempt to refresh authentication (token refresh, re-login, etc.).
    async fn refresh_auth(&self) -> Result<(), SyncError>;

    /// Fetch a complete listing with description, photos, price, etc.
    async fn fetch_full_listing(
        &self,
        _platform_item_id: &str,
    ) -> Result<FullListing, SyncError> {
        Err(SyncError::Other(format!(
            "{} does not support fetch_full_listing",
            self.platform()
        )))
    }

    /// Fetch the description for a listing.
    async fn fetch_description(
        &self,
        _platform_item_id: &str,
    ) -> Result<Option<ListingDescription>, SyncError> {
        Err(SyncError::Other(format!(
            "{} does not support fetch_description",
            self.platform()
        )))
    }

    /// Fetch photos for a listing.
    async fn fetch_photos(
        &self,
        _platform_item_id: &str,
    ) -> Result<Vec<ListingPhoto>, SyncError> {
        Err(SyncError::Other(format!(
            "{} does not support fetch_photos",
            self.platform()
        )))
    }

    /// Fetch the current price for a listing.
    async fn fetch_price(
        &self,
        _platform_item_id: &str,
    ) -> Result<Option<ListingPrice>, SyncError> {
        Err(SyncError::Other(format!(
            "{} does not support fetch_price",
            self.platform()
        )))
    }

    /// Set the price for a listing.
    async fn set_price(
        &self,
        _platform_item_id: &str,
        _price: ListingPrice,
    ) -> Result<(), SyncError> {
        Err(SyncError::Other(format!(
            "{} does not support set_price",
            self.platform()
        )))
    }

    /// Upload a photo to a listing.
    async fn upload_photo(
        &self,
        _platform_item_id: &str,
        _url: &str,
        _position: i32,
    ) -> Result<String, SyncError> {
        Err(SyncError::Other(format!(
            "{} does not support upload_photo",
            self.platform()
        )))
    }

    /// Set the HTML description for a listing.
    async fn set_description(
        &self,
        _platform_item_id: &str,
        _html: &str,
    ) -> Result<(), SyncError> {
        Err(SyncError::Other(format!(
            "{} does not support set_description",
            self.platform()
        )))
    }

    /// Create a new listing on this platform. Returns the new platform_item_id.
    async fn create_listing(
        &self,
        _request: CreateListingRequest,
    ) -> Result<String, SyncError> {
        Err(SyncError::Other(format!(
            "{} does not support create_listing",
            self.platform()
        )))
    }

    /// Update an existing listing on this platform.
    async fn update_listing(
        &self,
        _platform_item_id: &str,
        _request: UpdateListingRequest,
    ) -> Result<(), SyncError> {
        Err(SyncError::ApiError {
            platform: self.platform(),
            message: "update_listing not supported".into(),
        })
    }

    /// Detect recent sales on this platform.
    /// Only meaningful for platforms with `has_stock_mode_inventory` capability.
    /// Default implementation returns an empty list (platform uses normal polling).
    async fn detect_sales(&self) -> Result<Vec<SaleDetection>, SyncError> {
        Ok(Vec::new())
    }

    /// Return the set of capabilities this adapter supports.
    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities::default()
    }
}
