-- 207 R4: the session-stopped card — every stop outcome of a root / desk turn, not only `end_turn`.
-- Engine-local like 0006 (not replicated). Existing rows read as a clean finish.
ALTER TABLE attention_finished ADD COLUMN outcome TEXT NOT NULL DEFAULT 'finished';   -- finished | failed | cancelled | timed_out
ALTER TABLE attention_finished ADD COLUMN stop_reason TEXT NOT NULL DEFAULT 'end_turn';
ALTER TABLE attention_finished ADD COLUMN error TEXT;                                  -- <= 160 chars
