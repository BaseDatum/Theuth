-- 180 follow-up: when a change set was parked, for the 7-day expiry. Entries
-- parked before this release count from now.
ALTER TABLE parked ADD COLUMN parked_at INTEGER NOT NULL DEFAULT 0;
UPDATE parked SET parked_at = CAST(strftime('%s', 'now') AS INTEGER) * 1000;
