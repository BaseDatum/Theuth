-- 182 TASK-HOME D1: the engine (mesh id) that works the task — dispatch,
-- checkout, sweeps. NULL = a legacy row (or one from an old-version peer):
-- worked by the hub lease holder only. Replicated with the row.
ALTER TABLE tasks ADD COLUMN home TEXT;

-- 182 D7: engines that ran the one-time home backfill. Local, not replicated.
CREATE TABLE home_backfill (
    engine TEXT PRIMARY KEY,
    at_ms  INTEGER NOT NULL
);
