-- Archived sessions are hidden from the sidebar (51 §3); stored on the root,
-- children resolve the effective value through their lineage like `project`.
ALTER TABLE sessions ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
