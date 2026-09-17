-- Pinned sessions sort above the rest of the list.
ALTER TABLE sessions ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
