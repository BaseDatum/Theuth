-- 183 R1 (S1): which engine introduced each peer row here (local only, never replicated).
-- A row an engine introduced after (or just before) that engine was unpaired is quarantined:
-- tombstoned until a user keeps it (`confirmed` = 1, re-added) or drops it (`confirmed` = 1).
CREATE TABLE peer_origin (
    id          TEXT PRIMARY KEY,           -- the introduced engine
    by_engine   TEXT NOT NULL,              -- the engine whose peer set carried it first
    at_ms       INTEGER NOT NULL,           -- when it arrived here
    quarantined INTEGER NOT NULL DEFAULT 0,
    confirmed   INTEGER NOT NULL DEFAULT 0
);
