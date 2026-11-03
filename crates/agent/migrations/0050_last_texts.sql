-- 176 E2: the roster's cache of `Store::last_turn_text`, valid while `rev`
-- equals the session's `context_revision` (every append, cut and edit bumps
-- it; seqs are reused after a committed cut, so `MAX(seq)` is no key).
-- Engine-local, never captured (not in `capture::ATTACHED`): a missing or
-- stale row is recomputed on read and written back, with no backfill.
CREATE TABLE last_texts (session TEXT PRIMARY KEY NOT NULL, rev INTEGER NOT NULL, text TEXT NOT NULL);
