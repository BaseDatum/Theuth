-- 183 F11: a contact's first task held for the owner's answer waits ASIDE: stored
-- `auth_required` (what the contact reads) but `held = 1`, so it is neither the session's
-- one active task nor in the way of the next one. Any later transition clears it. Local
-- table (never replicated).
ALTER TABLE a2a_tasks ADD COLUMN held INTEGER NOT NULL DEFAULT 0 CHECK (held IN (0, 1));
