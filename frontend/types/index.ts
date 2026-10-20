// Platform types - mirrors Rust types
export type Platform = 'ebay' | 'squarespace' | 'xmrbazaar' | 'amazon'

export interface Product {
  id: string
  canonical_sku: string
  name: string
  quantity: number
  low_stock_threshold: number | null
  is_tracked: boolean
  has_variants: boolean
  created_at: string
  updated_at: string
}

export interface ProductVariant {
  id: string
  product_id: string
  sku: string
  name: string
  attributes: Record<string, string>
  quantity: number
  on_hand_quantity: number
  image_url: string | null
  sort_order: number
  created_at: string
  updated_at: string
}

export interface PlatformMapping {
  id: number
  product_id: string
  platform: Platform
  platform_item_id: string
  platform_sku: string | null
  is_active: boolean
  variant_id: string | null
}

export interface PlatformListing {
  platform_item_id: string
  title: string
  sku: string | null
  quantity: number
  price: number | null
  image_url: string | null
  group_key: string | null
  variant_attributes: Record<string, string>
}

export interface VariantSkuEntry {
  variant_id: string
  product_id: string
  sku: string
}

export interface SyncEvent {
  id: number
  product_id: string
  source_platform: string
  target_platform: string
  old_quantity: number
  new_quantity: number
  event_type: string
  sync_cycle_id: string
  error_message: string | null
  created_at: string
}

export interface ListingDescription {
  platform: Platform
  platform_item_id: string
  html: string | null
  plain_text: string | null
  fetched_at: string
}

export interface ListingPhoto {
  url: string
  position: number
  width: number | null
  height: number | null
  alt_text: string | null
}

export interface ListingPrice {
  amount: number
  currency: string
}

export interface FullListing {
  platform: Platform
  platform_item_id: string
  title: string
  sku: string | null
  quantity: number
  price: ListingPrice | null
  description: ListingDescription | null
  photos: ListingPhoto[]
  url: string | null
  fetched_at: string
  extras?: Record<string, string>
  variant_id?: string | null
}

export interface PricingSnapshot {
  id: number
  product_id: string
  platform: Platform
  amount: number
  currency: string
  recorded_at: string
}

export interface PlatformHealth {
  platform: Platform
  enabled: boolean
  authenticated: boolean
}

export interface PlatformCapabilities {
  can_fetch_full_listing: boolean
  can_fetch_description: boolean
  can_fetch_photos: boolean
  can_fetch_price: boolean
  can_set_price: boolean
  can_set_description: boolean
  can_upload_photos: boolean
  can_create_listing: boolean
}

export interface ProductAnalytics {
  product_id: string
  product_name: string
  current_quantity: number
  low_stock_threshold: number | null
  history: [string, number][]
  sales_velocity: number
  platform_sales: [Platform, number][]
}

export type TimeWindow = 'Day' | 'Week' | 'Month' | 'All'

// Sync engine events emitted via Tauri
export type SyncEngineEvent =
  | { CycleComplete: { cycle_id: string; products_synced: number; changes_pushed: number } }
  | { QuantityUpdated: { product_id: string; product_name: string; platform: Platform; old_quantity: number; new_quantity: number } }
  | { PlatformError: { platform: Platform; message: string } }
  | { AuthExpired: { platform: Platform } }
  | { UnmappedListingsDetected: { count: number } }

// Multi-company registry
export interface CompanyRegistryEntry {
  id: string
  name: string
  role: string
  db_path: string
  created_at: string
  updated_at: string
  logo: string | null
}

// P2P Company types
export interface CompanyInfo {
  id: string
  name: string
  our_onion: string | null
  role: string
  peers: CompanyPeer[]
  logo: string | null
}

export interface CompanyPeer {
  onion_address: string
  name: string
  role: string
  is_authorized: boolean
  added_at: string
  last_seen_at: string | null
  is_online: boolean
  sync_state: PeerSyncState | null
}

export interface PeerSyncState {
  last_synced_at: string | null
  last_sync_success: boolean
  last_error: string | null
  sync_count: number
}

export interface MergeSummary {
  products_updated: number
  products_created: number
  mappings_updated: number
  snapshots_updated: number
  orders_added: number
}

export type P2PEvent =
  | { type: 'OnionServiceReady'; onion_address: string }
  | { type: 'SyncStarted'; peer: string }
  | { type: 'SyncCompleted'; peer: string; summary: MergeSummary }
  | { type: 'SyncFailed'; peer: string; error: string }
  | { type: 'PeerJoinRequest'; onion_address: string; name: string }

export interface CompanyConfig {
  enabled: boolean
  sync_interval_secs: number
}

// Vendor plugin types
export interface VendorListing {
  vendor_item_id: string
  title: string
  price: number | null
  currency: string | null
  quantity: number | null
  sku: string | null
  image_url: string | null
  url: string | null
  extras?: Record<string, string>
  group_key?: string | null
  variant_attributes?: Record<string, string>
}

export interface VendorListingForProduct {
  plugin_name: string
  variant_id: string | null
  vendor_item_id: string
  title: string
  price: number | null
  currency: string | null
  quantity: number | null
  sku: string | null
  image_url: string | null
  url: string | null
  extras: Record<string, string>
  group_key: string | null
  variant_attributes: Record<string, string>
  fetched_at: string
}

export interface VendorImportItem {
  vendor_item_id: string
  group_key: string | null
  title: string
  sku: string
  quantity: number
  price: number | null
  currency: string | null
  image_url: string | null
  url: string | null
  extras: Record<string, string>
  variant_attributes: Record<string, string>
}

export interface ImportResult {
  products_created: number
  variants_created: number
}

export interface VendorConfigField {
  key: string
  label: string
  required: boolean
  secret: boolean
  placeholder?: string
}

export type VendorPluginStatus = 'pending' | 'approved' | 'rejected'

export interface VendorPluginInfo {
  id: string
  plugin_file: string
  display_name: string
  description: string
  version: string
  config_fields: VendorConfigField[]
  status: VendorPluginStatus
  submitted_by: string | null
  approved_by: string | null
  has_config: boolean
  installed: boolean
  enabled: boolean
  category: string
  icon: string | null
  include_vendor_stock: boolean
  /** What the plugin's `Nisaba.fetch` can reach, computed from its files. */
  network_access: 'restricted' | 'unrestricted' | 'unknown'
  /** The declared hosts when `network_access` is `restricted` (empty = no network). */
  allowed_hosts: string[]
  /** This machine's time limit for one run, in minutes. */
  timeout_minutes: number
  timeout_is_default: boolean
}

// Vendor sync types
export interface VendorSyncStatus {
  syncing: boolean
  last_synced_at: string | null
  cached_count: number
}

export interface VendorSyncEvent {
  plugin_id: string
  status: 'started' | 'completed' | 'failed'
  listing_count: number
  message: string
}

export interface VendorSyncBatchEvent {
  plugin_id: string
  listings_json: string
}

// eBay Taxonomy types
export interface EbayCategorySuggestion {
  category: { category_id: string; category_name: string }
  ancestors?: { category_id: string; category_name: string }[]
}

export interface EbayAspectMetadata {
  name: string
  constraint?: { required?: boolean; mode?: string; data_type?: string; cardinality?: string }
  values?: { value: string }[]
}

export interface EbayBusinessPolicy {
  id: string
  name: string
}

export interface AppConfig {
  general: {
    sync_schedule: string
    log_level: string
    max_retries: number
  }
  database: {
    path: string
  }
  ebay: {
    enabled: boolean
    client_id: string
    client_secret: string
    redirect_uri: string
    environment: string
  }
  squarespace: {
    enabled: boolean
    api_key: string
    user_agent: string
  }
  xmrbazaar: {
    enabled: boolean
    username: string
    password: string
    base_url: string
    monero_address: string
    endpoints: Record<string, { method: string; path: string }>
  }
  amazon: {
    enabled: boolean
    client_id: string
    client_secret: string
    refresh_token: string
    seller_id: string
    region: string
    marketplace_ids: string[]
  }
  alerts: {
    default_low_stock_threshold: number
  }
  company: CompanyConfig
}

// Notification system
export type NotificationType = 'success' | 'error' | 'warning' | 'info'

export interface AppNotification {
  id: string
  type: NotificationType
  title: string
  message: string
  detail?: string
  source?: string
  timestamp: number
  read: boolean
  dismissed: boolean
}
