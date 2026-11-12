-- 194: this chat's own compactor effort (`set_session_compactor_effort`);
-- NULL = the effort of the link that chose the model (none = thinking off).
ALTER TABLE sessions ADD COLUMN endless_effort TEXT NULL;
-- 194: the desk agent's `memory_effort` (written with `endless_agent_model`).
ALTER TABLE sessions ADD COLUMN endless_agent_effort TEXT NULL;
