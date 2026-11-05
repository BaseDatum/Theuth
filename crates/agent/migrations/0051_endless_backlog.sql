-- 137G: `endless_pause` — settling paused for the session (no compactor
-- calls); NULL / 0 = running. Survives a restart.
ALTER TABLE sessions ADD COLUMN endless_paused INTEGER NULL;
-- 137G: a node made by the mechanical fold (`endless_fold`, G7): an excerpt
-- of the unit, not a model summary.
ALTER TABLE endless_nodes ADD COLUMN mechanical INTEGER NOT NULL DEFAULT 0;
