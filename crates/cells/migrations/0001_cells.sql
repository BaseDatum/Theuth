-- 171 CELLS run store (design doc 171 §8.4), `<data_dir>/cells/cells.sqlite`, engine-local.
-- Owner: C (frozen). Every column the wave needs: audit (D17), delivery/parking (§9.3),
-- keystroke counters (`typed`, counts only), shell log ranges. A worker's extra statements go
-- into its reserved file (0002 E1, 0003 E2), never here.
CREATE TABLE IF NOT EXISTS cell_runs (
  id TEXT PRIMARY KEY,              -- r_<ulid>, minted by the pressing client
  scope TEXT NOT NULL,              -- session:<uuid> | doc:<id>
  cell TEXT NOT NULL,               -- CellRef JSON
  lineage TEXT,                     -- (scope, name) key when named
  mode TEXT NOT NULL,               -- cell | line | selection
  pass TEXT,                        -- run-all pass id
  kernel TEXT NOT NULL,             -- KernelId JSON
  interpreter TEXT NOT NULL,        -- "bash 5.2 (Homebrew) /opt/homebrew/bin/bash"
  cwd TEXT NOT NULL,
  env_names TEXT NOT NULL,          -- JSON array of names, never values
  presented_sha TEXT NOT NULL,      -- digest the user saw
  code_sha TEXT NOT NULL,           -- digest of what ran
  edited INTEGER NOT NULL,
  pressed_by TEXT NOT NULL,         -- device id + desktop|web|phone
  typed TEXT,                       -- JSON {device: keystroke count}, never contents
  state TEXT NOT NULL,              -- queued|running|done|failed|stopped|timeout|killed|lost|cancelled
  exit INTEGER, signal TEXT, secs REAL,
  expect TEXT,                      -- JSON [{item, met}]
  summary TEXT,
  output_sha TEXT, output_bytes INTEGER,
  log TEXT, log_from INTEGER, log_to INTEGER,   -- job log path and byte range (shells)
  queued_ms INTEGER NOT NULL, started_ms INTEGER, ended_ms INTEGER,
  delivery TEXT,                    -- joined | parked | woke | none
  consumed_ms INTEGER               -- the parked result joined a turn
);
CREATE INDEX IF NOT EXISTS cell_runs_scope ON cell_runs(scope, queued_ms);
CREATE INDEX IF NOT EXISTS cell_runs_lineage ON cell_runs(lineage, ended_ms);
