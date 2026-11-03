-- E1 (171 lead pin L11): the press-time facts the run card shows, so a stored run carries them.
ALTER TABLE cell_runs ADD COLUMN masked INTEGER NOT NULL DEFAULT 0;  -- values masked in the agent's copy
ALTER TABLE cell_runs ADD COLUMN private INTEGER NOT NULL DEFAULT 0; -- `#| private` at press
ALTER TABLE cell_runs ADD COLUMN name TEXT;                          -- `#| name` at press
