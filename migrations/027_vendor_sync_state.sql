-- Incremental vendor-sync state (N-7.4).
--
-- The host's own bookkeeping for `fetchListings({ since, cursor })`, per plugin:
-- `last_since` is the UTC timestamp ("%Y-%m-%d %H:%M:%S") of the last successful
-- sync and `cursor` the opaque cursor the plugin returned on that run, both
-- handed back to the plugin on the next run. Unlike `Nisaba.store`
-- (plugin-managed, cleared on uninstall), this is host state — but it is still
-- cleared when the plugin is uninstalled, so a re-install starts from a full
-- pull and never resumes from a stale cursor.
CREATE TABLE IF NOT EXISTS vendor_sync_state (
    plugin_id TEXT PRIMARY KEY,
    last_since TEXT NOT NULL,
    cursor TEXT,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%d %H:%M:%S', 'now'))
);
