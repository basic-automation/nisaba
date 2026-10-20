-- Stock ledger (N-4.1): every stock change is a uniquely keyed event.
-- Canonical quantity per variant is the sum of its events' deltas.
-- A sale is keyed "sale:<platform>:<order_id>"; restocks, manual adjustments
-- and vendor updates carry a UUID key. The UNIQUE(event_key) constraint makes
-- re-recording the same event (e.g. a retried sale) a no-op.
CREATE TABLE IF NOT EXISTS stock_ledger (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    event_key       TEXT NOT NULL,
    product_id      TEXT NOT NULL,
    variant_id      TEXT NOT NULL REFERENCES product_variants(id),
    event_kind      TEXT NOT NULL, -- opening | sale | restock | adjustment | vendor_update
    delta           INTEGER NOT NULL, -- negative for sales, positive for restocks/opening
    platform        TEXT,
    source_order_id TEXT,
    note            TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(event_key)
);
CREATE INDEX IF NOT EXISTS idx_stock_ledger_variant ON stock_ledger(variant_id);
CREATE INDEX IF NOT EXISTS idx_stock_ledger_product ON stock_ledger(product_id);
