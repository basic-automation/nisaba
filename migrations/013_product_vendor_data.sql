CREATE TABLE IF NOT EXISTS product_vendor_data (
    product_id         TEXT NOT NULL,
    variant_id         TEXT,
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
    updated_at         TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (product_id, plugin_id, vendor_item_id)
);

CREATE INDEX IF NOT EXISTS idx_pvd_product ON product_vendor_data(product_id);
CREATE INDEX IF NOT EXISTS idx_pvd_plugin_item ON product_vendor_data(plugin_id, vendor_item_id);
