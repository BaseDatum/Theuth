-- 140 R-33 / R-41: who held the machine's build slots when a sample started, and
-- why a finished run produced no value.
ALTER TABLE samples ADD COLUMN slot_holders TEXT NOT NULL DEFAULT '[]';
ALTER TABLE samples ADD COLUMN failure TEXT;
