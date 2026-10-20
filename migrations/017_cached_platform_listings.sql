CREATE TABLE IF NOT EXISTS cached_platform_listings (
    platform     TEXT NOT NULL,
    listings_json TEXT NOT NULL,
    cached_at    TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (platform)
);
