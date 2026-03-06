-- Company-level plugin registry (syncs via P2P)
CREATE TABLE IF NOT EXISTS vendor_plugin_registry (
    id            TEXT PRIMARY KEY,
    plugin_file   TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    description   TEXT NOT NULL DEFAULT '',
    code          TEXT NOT NULL,
    version       TEXT NOT NULL DEFAULT '0.0.0',
    config_fields TEXT,                                -- JSON array of ConfigField
    config_json   TEXT,                                -- encrypted company-wide credentials
    status        TEXT NOT NULL DEFAULT 'pending',     -- pending | approved | rejected
    submitted_by  TEXT,                                -- peer onion address
    approved_by   TEXT,                                -- admin peer onion address
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

-- User-level install state (local only, never syncs)
CREATE TABLE IF NOT EXISTS vendor_plugin_installs (
    plugin_id   TEXT PRIMARY KEY REFERENCES vendor_plugin_registry(id),
    installed   BOOLEAN NOT NULL DEFAULT 1,
    enabled     BOOLEAN NOT NULL DEFAULT 1,
    installed_at TEXT NOT NULL DEFAULT (datetime('now'))
);
