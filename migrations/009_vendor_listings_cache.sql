CREATE TABLE IF NOT EXISTS vendor_listings_cache (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    plugin_id          TEXT NOT NULL,
    vendor_item_id     TEXT NOT NULL,
    title              TEXT NOT NULL,
    price              REAL,
    currency           TEXT,
    quantity           INTEGER,
    sku                TEXT,
    image_url          TEXT,
    url                TEXT,
    extras_json        TEXT NOT NULL DEFAULT '{}',
    group_key          TEXT,
    variant_attrs_json TEXT NOT NULL DEFAULT '{}',
    fetched_at         TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(plugin_id, vendor_item_id)
);
CREATE INDEX IF NOT EXISTS idx_vlc_plugin ON vendor_listings_cache(plugin_id);
