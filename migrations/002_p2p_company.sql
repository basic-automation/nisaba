-- P2P Company Sync tables

CREATE TABLE IF NOT EXISTS company (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    secret      TEXT NOT NULL,
    our_onion   TEXT,
    role        TEXT NOT NULL DEFAULT 'admin',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS company_peers (
    onion_address TEXT PRIMARY KEY,
    name          TEXT NOT NULL DEFAULT '',
    role          TEXT NOT NULL DEFAULT 'member',
    is_authorized INTEGER NOT NULL DEFAULT 1,
    added_at      TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at  TEXT
);

CREATE TABLE IF NOT EXISTS p2p_sync_state (
    peer_address     TEXT PRIMARY KEY,
    last_synced_at   TEXT,
    last_sync_success INTEGER NOT NULL DEFAULT 0,
    last_error       TEXT,
    sync_count       INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS p2p_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Add soft-delete and updated_at to platform_mappings
ALTER TABLE platform_mappings ADD COLUMN deleted_at TEXT;
ALTER TABLE platform_mappings ADD COLUMN updated_at TEXT NOT NULL DEFAULT '';
