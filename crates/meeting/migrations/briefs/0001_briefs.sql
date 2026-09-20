-- The brief cache (05 §5): `briefs` is the truth, `briefs_fts` its FTS5 mirror
-- (aliases/tags indexed as space-joined text, weighted in `Cache::search`).
CREATE TABLE briefs (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  bullets TEXT NOT NULL,      -- json array
  aliases TEXT NOT NULL,      -- json array; first = display label
  tags TEXT NOT NULL,         -- json array
  source TEXT NOT NULL,       -- "meeting:<id>" | "session:<id>" | "user"
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  uses INTEGER NOT NULL DEFAULT 0,
  last_used_at TEXT
);
CREATE VIRTUAL TABLE briefs_fts USING fts5(
  id UNINDEXED,
  aliases,
  title,
  tags,
  bullets,
  tokenize = 'porter unicode61 remove_diacritics 2'
);
