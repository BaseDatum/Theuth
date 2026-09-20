-- Meetings index (01 §5): derived from `<data_dir>/meetings/*/manifest.json`,
-- rebuilt from them when this file is missing.
CREATE TABLE meetings (
  id TEXT PRIMARY KEY,
  started_at TEXT NOT NULL,
  ended_at TEXT,
  title TEXT,
  session TEXT NOT NULL,
  process_name TEXT NOT NULL,
  duration_ms INTEGER,
  state TEXT NOT NULL
);
