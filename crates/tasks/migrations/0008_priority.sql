-- 132 ATTENTION-ROWS-AND-ORDER D4: how soon a task wants attention
-- (urgent | high | medium | low); the user alone sets it.

ALTER TABLE tasks ADD COLUMN priority TEXT NOT NULL DEFAULT 'medium';
