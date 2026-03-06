CREATE TABLE IF NOT EXISTS products (
    id                  TEXT PRIMARY KEY,
    canonical_sku       TEXT NOT NULL,
    name                TEXT NOT NULL,
    quantity            INTEGER NOT NULL DEFAULT 0,
    low_stock_threshold INTEGER,
    is_tracked          BOOLEAN NOT NULL DEFAULT TRUE,
    created_at          TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at          TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_products_sku ON products(canonical_sku);

CREATE TABLE IF NOT EXISTS platform_mappings (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id       TEXT NOT NULL REFERENCES products(id),
    platform         TEXT NOT NULL,
    platform_item_id TEXT NOT NULL,
    platform_sku     TEXT,
    is_active        BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_mappings_product_platform ON platform_mappings(product_id, platform);
CREATE UNIQUE INDEX IF NOT EXISTS idx_mappings_platform_item ON platform_mappings(platform, platform_item_id);

CREATE TABLE IF NOT EXISTS platform_snapshots (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id          TEXT NOT NULL REFERENCES products(id),
    platform            TEXT NOT NULL,
    last_known_quantity INTEGER NOT NULL,
    last_polled_at      TEXT NOT NULL,
    last_pushed_at      TEXT,
    version_tag         TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_snapshots_product_platform ON platform_snapshots(product_id, platform);

CREATE TABLE IF NOT EXISTS sync_events (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id      TEXT NOT NULL,
    source_platform TEXT NOT NULL,
    target_platform TEXT NOT NULL,
    old_quantity    INTEGER NOT NULL,
    new_quantity    INTEGER NOT NULL,
    event_type      TEXT NOT NULL,
    sync_cycle_id   TEXT NOT NULL,
    error_message   TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS auth_tokens (
    platform      TEXT PRIMARY KEY,
    access_token  TEXT NOT NULL,
    refresh_token TEXT,
    expires_at    TEXT,
    cookies       TEXT,
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS inventory_history (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id    TEXT NOT NULL REFERENCES products(id),
    quantity      INTEGER NOT NULL,
    sync_cycle_id TEXT NOT NULL,
    recorded_at   TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_inventory_history_product_time
    ON inventory_history(product_id, recorded_at);

CREATE TABLE IF NOT EXISTS listing_descriptions (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id       TEXT NOT NULL REFERENCES products(id),
    platform         TEXT NOT NULL,
    platform_item_id TEXT NOT NULL,
    description_html TEXT,
    description_text TEXT,
    fetched_at       TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_descriptions_product_platform ON listing_descriptions(product_id, platform);

CREATE TABLE IF NOT EXISTS listing_photos (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id       TEXT NOT NULL REFERENCES products(id),
    platform         TEXT NOT NULL,
    platform_item_id TEXT NOT NULL,
    url              TEXT NOT NULL,
    position         INTEGER NOT NULL DEFAULT 0,
    width            INTEGER,
    height           INTEGER,
    alt_text         TEXT,
    fetched_at       TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_listing_photos_product_platform
    ON listing_photos(product_id, platform);

CREATE TABLE IF NOT EXISTS pricing_snapshots (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id  TEXT NOT NULL REFERENCES products(id),
    platform    TEXT NOT NULL,
    amount      REAL NOT NULL,
    currency    TEXT NOT NULL DEFAULT 'USD',
    recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_pricing_snapshots_product_time
    ON pricing_snapshots(product_id, recorded_at);

CREATE TABLE IF NOT EXISTS xmr_processed_orders (
    order_id    TEXT PRIMARY KEY,
    product_id  TEXT,
    listing_id  TEXT NOT NULL,
    quantity    INTEGER NOT NULL DEFAULT 1,
    processed_at TEXT NOT NULL DEFAULT (datetime('now'))
);
