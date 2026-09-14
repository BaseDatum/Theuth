-- Last activity (creation or last appended message); the session list order.
ALTER TABLE sessions ADD COLUMN updated_at TEXT NOT NULL DEFAULT '';
UPDATE sessions SET updated_at = created_at;
