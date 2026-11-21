-- 201 §5 (P1): a node's quality — `free` (final, no model), `draft`
-- (deterministic, may be refined) or `model` (a guard-passing model line).
-- Existing rows: `free` where free is set, `draft` where mechanical is set,
-- else `model`. `endless_nodes` is replicated by the session log:
-- SESSIONS_WIRE_SCHEMA becomes 70.
ALTER TABLE endless_nodes ADD COLUMN quality TEXT NOT NULL DEFAULT 'model';
UPDATE endless_nodes SET quality = CASE
    WHEN free THEN 'free'
    WHEN mechanical THEN 'draft'
    ELSE 'model'
END;
