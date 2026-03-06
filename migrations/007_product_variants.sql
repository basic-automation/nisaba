-- Product variants table
CREATE TABLE IF NOT EXISTS product_variants (
    id              TEXT PRIMARY KEY,
    product_id      TEXT NOT NULL REFERENCES products(id),
    sku             TEXT NOT NULL,
    name            TEXT NOT NULL,
    attributes_json TEXT NOT NULL DEFAULT '{}',
    quantity        INTEGER NOT NULL DEFAULT 0,
    image_url       TEXT,
    sort_order      INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_variants_product_sku ON product_variants(product_id, sku);
CREATE INDEX IF NOT EXISTS idx_variants_product ON product_variants(product_id);
