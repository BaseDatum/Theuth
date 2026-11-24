-- 214 A1: task ids that were blockers until that task closed (done/cancelled) —
-- the finished dependencies the task graph draws faded. Replicated with the row;
-- a peer without the column skips it.
ALTER TABLE tasks ADD COLUMN resolved_blockers_json TEXT NOT NULL DEFAULT '[]';
