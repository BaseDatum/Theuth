-- A user-set expiry on what a `secret_request` card stored. Absolute ms since the epoch on the
-- `vault` row (per (name, version)), so it belongs to the write that set it and replicates with
-- that row: the current version's `expires_ms` is the secret's expiry, and a later version written
-- without one (a plain replace, `secret_capture`, a restore) clears it. NULL = never.
ALTER TABLE vault ADD COLUMN expires_ms INTEGER;

-- The agent's suggestion for the card's expiry control (`30m`, `2h`, `3d`, or an RFC 3339
-- instant); it only pre-fills the control. NULL = Never.
ALTER TABLE secret_requests ADD COLUMN expires TEXT;
