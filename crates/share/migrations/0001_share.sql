-- 210 §5.8, client `share.sqlite`: four replicated tables (single-column key + hlc) and three local ones.
CREATE TABLE shares (id TEXT PRIMARY KEY, engine TEXT NOT NULL, edge TEXT NOT NULL, domain TEXT NOT NULL,
  host TEXT NOT NULL, path TEXT NOT NULL, url TEXT NOT NULL, source TEXT NOT NULL, access TEXT NOT NULL,
  interstitial INTEGER NOT NULL, created_ms INTEGER NOT NULL, live_ms INTEGER, expires_ms INTEGER,
  created_by TEXT NOT NULL, approved_by TEXT NOT NULL, state TEXT NOT NULL, stopped_ms INTEGER,
  stopped_by TEXT, hlc TEXT);
CREATE TABLE share_names (id TEXT PRIMARY KEY /* "<edge>/<domain>/<name>" */, edge TEXT NOT NULL,
  domain TEXT NOT NULL, name TEXT NOT NULL, kind TEXT NOT NULL, is_primary INTEGER NOT NULL DEFAULT 0,
  released_ms INTEGER, hlc TEXT);
CREATE TABLE share_edges (id TEXT PRIMARY KEY /* "<edge>/<domain>" */, edge TEXT NOT NULL, domain TEXT NOT NULL,
  ticket TEXT NOT NULL, registered_ms INTEGER NOT NULL, deleted_ms INTEGER, hlc TEXT);
CREATE TABLE share_rekeys (id TEXT PRIMARY KEY /* uuid v7 */, removed TEXT NOT NULL /* json [engine] */,
  tombstone_ms INTEGER NOT NULL, from_pub TEXT, to_pub TEXT,
  state TEXT NOT NULL,
  edges_done TEXT NOT NULL DEFAULT '[]', attempts INTEGER NOT NULL DEFAULT 0, next_at_ms INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL, hlc TEXT);
-- local, never replicated
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE share_secrets (share TEXT PRIMARY KEY, password_phc TEXT NOT NULL);
CREATE TABLE edge_state (edge TEXT PRIMARY KEY, last_attach_ms INTEGER, terms_rev TEXT, clock_skew_ms INTEGER,
  live_key TEXT, retired_keys TEXT);
