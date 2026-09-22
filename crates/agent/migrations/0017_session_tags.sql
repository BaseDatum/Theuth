-- Design doc 07 §15 / 50 §5.1 as-built: keywords the title model assigns
-- beside the title; comma-joined `[a-z0-9-]` words, empty until the first
-- model-written title.
ALTER TABLE sessions ADD COLUMN tags TEXT NOT NULL DEFAULT '';
