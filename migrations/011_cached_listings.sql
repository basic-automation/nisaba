CREATE TABLE IF NOT EXISTS cached_listings (
    product_id       TEXT NOT NULL,
    platform         TEXT NOT NULL,
    platform_item_id TEXT NOT NULL,
    listing_json     TEXT NOT NULL,
    fetched_at       TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (product_id, platform, platform_item_id)
);
