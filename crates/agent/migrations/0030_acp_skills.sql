-- 75 §10: the theuth skills `spawn_subagent.skills` named for an ACP child, as
-- a JSON array of names, so `resume_child` writes the same set back into the
-- cwd (the actor removes them when its connection ends, 75 §4). NULL for a
-- native session or a child spawned before this column.
ALTER TABLE sessions ADD COLUMN acp_skills TEXT;
