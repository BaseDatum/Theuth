-- 40 §3.4: per-watcher sensor state; derived like `runs` (delete the file → first run again).
CREATE TABLE watch (
    name TEXT PRIMARY KEY,
    state TEXT NOT NULL DEFAULT '{}',
    last_hash TEXT,
    last_fired_at TEXT,
    last_check_at TEXT,
    last_outcome TEXT,
    last_elapsed_ms INTEGER,
    holding_since TEXT,
    strikes INTEGER NOT NULL DEFAULT 0,
    fired_count INTEGER NOT NULL DEFAULT 0,
    session_id TEXT
);
-- The payload's first line on fired rows, for history grouping (40 §1.5).
ALTER TABLE runs ADD COLUMN envelope TEXT;
