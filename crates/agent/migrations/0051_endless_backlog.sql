-- 137G: `endless_pause` — who paused settling for the session: 'user' or
-- 'spend' (the G11 day cap); NULL = running. Survives a restart.
ALTER TABLE sessions ADD COLUMN endless_paused TEXT NULL;
-- 137G G11: the UTC day (YYYY-MM-DD) on which the user resumed past the
-- session's day cap; the cap does not pause again that day.
ALTER TABLE sessions ADD COLUMN endless_spend_override TEXT NULL;
-- 137G: this chat's own compactor model (`set_session_compactor_model`);
-- NULL = inherit (agent default → agent.endless.model → title → session).
ALTER TABLE sessions ADD COLUMN endless_model TEXT NULL;
-- 137G: the desk agent's `memory_model` (written by the roster at desk open).
ALTER TABLE sessions ADD COLUMN endless_agent_model TEXT NULL;
-- 137G: a node made by the mechanical fold (`endless_fold`, G7): an excerpt
-- of the unit, not a model summary.
ALTER TABLE endless_nodes ADD COLUMN mechanical INTEGER NOT NULL DEFAULT 0;
