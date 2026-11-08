-- 183 §4.5 (D4, W-4): per-session settings that travel with the session.
-- `approvals`: NULL inherits `tools.approvals`, 'on' / 'off' pin it.
-- `communication`: NULL inherits, 0 = opted out (1 reserved for an explicit on).
ALTER TABLE sessions ADD COLUMN approvals TEXT NULL CHECK (approvals IN ('on', 'off'));
ALTER TABLE sessions ADD COLUMN communication INTEGER NULL CHECK (communication IN (0, 1));
-- 183 §5.3 (W-8): typed moved notice beside a user row (the advisory pattern,
-- 0034_advisor.sql); JSON `SessionMoved`; NULL for every ordinary message.
ALTER TABLE messages ADD COLUMN moved TEXT NULL;
