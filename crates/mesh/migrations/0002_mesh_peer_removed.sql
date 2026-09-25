-- 67 §4.1: `t mesh remove` leaves a tombstone instead of deleting the row, so
-- a removal replicates and outranks a stale re-add (LWW on
-- max(added_ms, removed_ms); removal wins a tie). Listings filter
-- `removed_ms IS NULL`.
ALTER TABLE peers ADD COLUMN removed_ms INTEGER;
