-- Migration 003: Multi-company support
-- Applied to ALL databases (app DB and company DBs)

-- Company registry: tracks all companies this instance belongs to (app DB only)
CREATE TABLE IF NOT EXISTS company_registry (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    role        TEXT NOT NULL DEFAULT 'admin',
    secret_hash TEXT NOT NULL DEFAULT '',
    db_path     TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- App-level settings (app DB only)
CREATE TABLE IF NOT EXISTS app_settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Per-company encrypted config (company DBs)
CREATE TABLE IF NOT EXISTS company_config (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    config_json TEXT NOT NULL DEFAULT '{}',
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Peer identity: stable UUIDs instead of onion address as PK
CREATE TABLE IF NOT EXISTS company_peers_v2 (
    peer_id       TEXT PRIMARY KEY,
    onion_address TEXT NOT NULL,
    name          TEXT NOT NULL DEFAULT '',
    role          TEXT NOT NULL DEFAULT 'member',
    is_authorized INTEGER NOT NULL DEFAULT 1,
    added_at      TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at  TEXT
);

-- Add peer_id column to company table if it exists
-- (SQLite doesn't support IF NOT EXISTS for ALTER TABLE, so we use a trick)
-- This is handled in Rust code to avoid ALTER TABLE errors

-- Migrate existing peers from company_peers to company_peers_v2
-- (handled in Rust migration code)
