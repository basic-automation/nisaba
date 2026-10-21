-- Plugin run history and per-plugin schedule (N-7.5).
--
-- `plugin_run_history` holds one row per plugin run (auto, manual or
-- fetch): when it started and finished, whether it completed or failed,
-- how many listings it emitted, the error on failure, and the plugin's own
-- log lines (`Nisaba.log`, already routed into the app log) captured for
-- that run as a JSON array. The table lives in the company database, so a
-- run history is scoped to plugin + company by construction. Rows are
-- pruned to the most recent runs per plugin by the host on finish.
--
-- `vendor_plugin_installs.sync_interval_minutes` is this machine's
-- per-plugin auto-sync interval: NULL means "follow the global sync
-- cycle" (the pre-N-7.5 behaviour); a value means the auto-sync path only
-- runs the plugin when at least that many minutes have passed since its
-- last recorded run started. Manual runs are never gated by it.
CREATE TABLE IF NOT EXISTS plugin_run_history (
    id                INTEGER PRIMARY KEY,
    plugin_id         TEXT NOT NULL,
    started_at        TEXT NOT NULL,
    finished_at       TEXT,
    status            TEXT NOT NULL DEFAULT 'running',
    listings_emitted  INTEGER NOT NULL DEFAULT 0,
    error             TEXT,
    log_lines         TEXT NOT NULL DEFAULT '[]',
    trigger_kind      TEXT NOT NULL DEFAULT 'auto'
);
CREATE INDEX IF NOT EXISTS idx_plugin_run_history_plugin_started
    ON plugin_run_history(plugin_id, started_at DESC);
-- The `vendor_plugin_installs.sync_interval_minutes` column is added inline
-- in Rust (migration step 28 in db.rs), following steps 018/019/025: an
-- ALTER there can ignore the duplicate-column error when the step re-runs
-- on a database that already has the column.
