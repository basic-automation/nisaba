use serde::{Deserialize, Serialize};

use nisaba_core::types::{Platform, PlatformSnapshot, Product, XmrProcessedOrder};

pub const PROTOCOL_VERSION: u32 = 2;

/// Full sync request sent to a peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRequest {
    pub protocol_version: u32,
    /// Stable peer identity (UUID).
    #[serde(default)]
    pub sender_peer_id: String,
    pub sender_onion: String,
    pub sender_time: String,
    pub payload: SyncPayload,
}

/// The data exchanged during a sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPayload {
    pub products: Vec<Product>,
    pub platform_mappings: Vec<SyncableMapping>,
    pub platform_snapshots: Vec<PlatformSnapshot>,
    pub xmr_processed_orders: Vec<XmrProcessedOrder>,
    pub platform_sync_meta: PlatformSyncMeta,
    /// Platform config (cleartext in transit, encrypted at rest).
    #[serde(default)]
    pub company_config: Option<CompanyConfigPayload>,
    /// Auth tokens (cleartext in transit, encrypted at rest).
    #[serde(default)]
    pub auth_tokens: Vec<SyncableAuthToken>,
    /// Company logo (base64 data URL).
    #[serde(default)]
    pub company_logo: Option<CompanyLogoPayload>,
    /// Vendor plugin registry entries.
    #[serde(default)]
    pub vendor_plugins: Vec<SyncableVendorPlugin>,
    /// Product variants.
    #[serde(default)]
    pub product_variants: Vec<SyncableVariant>,
}

/// Full sync response from a peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResponse {
    pub protocol_version: u32,
    pub responder_onion: String,
    pub responder_time: String,
    pub payload: SyncPayload,
    pub merge_summary: MergeSummary,
}

/// Mapping with soft-delete and timestamp support for sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncableMapping {
    pub product_id: String,
    pub platform: Platform,
    pub platform_item_id: String,
    pub platform_sku: Option<String>,
    pub is_active: bool,
    pub deleted_at: Option<String>,
    pub updated_at: String,
    #[serde(default)]
    pub variant_id: Option<String>,
}

/// Product variant for P2P sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncableVariant {
    pub id: String,
    pub product_id: String,
    pub sku: String,
    pub name: String,
    pub attributes_json: String,
    pub quantity: i64,
    #[serde(default)]
    pub on_hand_quantity: i64,
    pub image_url: Option<String>,
    pub sort_order: i32,
    pub updated_at: String,
    /// The vendor plugin and item a dropship variant is sourced from. A variant's
    /// effective quantity includes that plugin's vendor stock, which each peer fetches with
    /// its own vendor sync — without the link a receiving peer cannot attach it and counts
    /// on-hand stock only. Absent from peers older than this field.
    #[serde(default)]
    pub source_plugin_id: Option<String>,
    #[serde(default)]
    pub source_vendor_item_id: Option<String>,
}

/// Coordination metadata for round-robin platform sync.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlatformSyncMeta {
    pub last_platform_sync_at: Option<String>,
    pub last_platform_sync_by: Option<String>,
}

/// Company config payload for P2P sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanyConfigPayload {
    /// Cleartext JSON config (encrypted at rest on each peer).
    pub config_json: String,
    /// ISO 8601 timestamp for last-write-wins merge.
    pub updated_at: String,
}

/// Company logo payload for P2P sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanyLogoPayload {
    /// Base64 data URL of the logo image.
    pub logo: String,
    /// ISO 8601 timestamp for last-write-wins merge.
    pub updated_at: String,
}

/// Auth token payload for P2P sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncableAuthToken {
    pub platform: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
    pub cookies: Option<String>,
    pub updated_at: String,
}

/// Vendor plugin registry entry for P2P sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncableVendorPlugin {
    pub id: String,
    pub plugin_file: String,
    pub display_name: String,
    pub description: String,
    /// JSON map of filename → content (multi-file plugins).
    #[serde(default)]
    pub files_json: String,
    /// Legacy single-file code field for backward compat with old peers.
    #[serde(default)]
    pub code: String,
    pub version: String,
    pub config_fields: Option<String>,
    pub config_json: Option<String>,
    pub status: String,
    pub submitted_by: Option<String>,
    pub approved_by: Option<String>,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub include_vendor_stock: bool,
    pub updated_at: String,
}

fn default_category() -> String {
    "vendor".into()
}

impl SyncableVendorPlugin {
    /// Resolve the files_json, falling back to wrapping legacy `code` as `{"index.ts": code}`.
    pub fn resolved_files_json(&self) -> String {
        if !self.files_json.is_empty() {
            self.files_json.clone()
        } else if !self.code.is_empty() {
            serde_json::json!({"index.ts": self.code}).to_string()
        } else {
            "{}".to_string()
        }
    }
}

/// Summary of a merge operation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MergeSummary {
    pub products_updated: usize,
    pub products_created: usize,
    pub mappings_updated: usize,
    pub snapshots_updated: usize,
    pub orders_added: usize,
    #[serde(default)]
    pub vendor_plugins_updated: usize,
    #[serde(default)]
    pub variants_updated: usize,
    /// Remote rows refused because their timestamp was unparseable or too far ahead of
    /// our clock to be a real edit.
    #[serde(default)]
    pub rejected_timestamps: usize,
}

/// Health check response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingResponse {
    pub onion_address: String,
    pub protocol_version: u32,
    pub timestamp: String,
}

/// Company info returned to peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanyInfo {
    pub id: String,
    pub name: String,
    pub our_onion: Option<String>,
    pub role: String,
    pub peers: Vec<PeerInfo>,
    #[serde(default)]
    pub logo: Option<String>,
}

/// Info about a peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub onion_address: String,
    pub name: String,
    pub role: String,
    pub is_authorized: bool,
    pub added_at: String,
    pub last_seen_at: Option<String>,
    pub is_online: bool,
    pub sync_state: Option<PeerSyncState>,
}

/// Sync state for a peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerSyncState {
    pub last_synced_at: Option<String>,
    pub last_sync_success: bool,
    pub last_error: Option<String>,
    pub sync_count: i64,
}

/// Join request sent by a new peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRequest {
    pub onion_address: String,
    pub name: String,
}

/// Request to set a peer's role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetRoleRequest {
    pub role: String,
}

/// Address update announcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressUpdate {
    pub peer_id: String,
    pub new_onion_address: String,
}

/// Secret rotation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotateSecretRequest {
    pub new_secret: String,
    pub rotated_by: String,
    pub rotated_at: String,
}

/// Events emitted by the P2P system.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum P2PEvent {
    OnionServiceReady { onion_address: String },
    SyncStarted { peer: String },
    SyncCompleted { peer: String, summary: MergeSummary },
    SyncFailed { peer: String, error: String },
    PeerJoinRequest { onion_address: String, name: String },
}

/// Commands sent to the P2P manager.
#[derive(Debug, Clone)]
pub enum P2PCommand {
    SyncNow,
    Stop,
}
