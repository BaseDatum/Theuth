-- Design doc 144 §8: a chat is bound to at most one notebook (one context
-- budget). Like `project` (0015) the column lives on the root row only;
-- children resolve the effective value through their lineage.
ALTER TABLE sessions ADD COLUMN notebook TEXT NULL;
