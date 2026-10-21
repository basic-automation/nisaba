-- Per-plugin key-value storage behind `Nisaba.store` (N-7.3; renumbered 025 -> 026 on merge with N-7.2, which also claimed 025).
--
-- One row per (plugin, key). The table lives in the company database, so a
-- plugin's store is scoped to plugin + company by construction: another
-- plugin, or the same plugin under another company, reads a different table
-- entirely. Values hold cursors, ETags and last-seen timestamps — small
-- strings. The 1 MiB per-plugin cap is enforced by the plugin runtime before
-- the write, not by a CHECK here, so the limit message can name the API.
CREATE TABLE IF NOT EXISTS plugin_kv_store (
    plugin_id  TEXT NOT NULL,
    key        TEXT NOT NULL,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (plugin_id, key)
);
