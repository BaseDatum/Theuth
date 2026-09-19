-- Design doc 51 §3 / §6.4: one nullable project label on the root row only
-- (children resolve the effective value through their lineage), plus the
-- accounting ledger. `cost_owners` deliberately has no FK to `sessions`:
-- deleting conversation rows must never cascade spend away.
ALTER TABLE sessions ADD COLUMN project TEXT NULL;

CREATE TABLE cost_owners (
  id TEXT PRIMARY KEY,
  root_id TEXT NOT NULL,
  parent_id TEXT NULL REFERENCES cost_owners(id),
  deleted_at TEXT NULL,
  history_coverage TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX cost_owners_root ON cost_owners(root_id);
CREATE INDEX cost_owners_parent ON cost_owners(parent_id);

CREATE TABLE model_calls (
  call_id TEXT PRIMARY KEY,
  owner_id TEXT NOT NULL REFERENCES cost_owners(id),
  request_id TEXT NOT NULL,
  attempt INTEGER NOT NULL,
  turn_id TEXT NULL,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  provider TEXT NULL,
  requested_model TEXT NULL,
  served_model TEXT NULL,
  provider_request_id TEXT NULL,
  input_tokens INTEGER NULL,
  output_tokens INTEGER NULL,
  cache_read_tokens INTEGER NULL,
  cache_write_tokens INTEGER NULL,
  context_tokens INTEGER NULL,
  returned_nanos INTEGER NULL CHECK(returned_nanos >= 0),
  returned_currency TEXT NULL,
  returned_source TEXT NULL,
  estimated_nanos INTEGER NULL CHECK(estimated_nanos >= 0),
  estimated_currency TEXT NULL,
  estimate_source TEXT NULL,
  unknown_reason TEXT NULL,
  coverage TEXT NOT NULL,
  source_metadata TEXT NULL,
  created_at TEXT NOT NULL,
  dispatched_at TEXT NULL,
  settled_at TEXT NULL
);
CREATE INDEX model_calls_owner ON model_calls(owner_id);

CREATE TABLE remote_meters (
  id TEXT PRIMARY KEY,
  owner_id TEXT NOT NULL REFERENCES cost_owners(id),
  origin TEXT NOT NULL,
  external_session TEXT NOT NULL,
  epoch INTEGER NOT NULL,
  source TEXT NOT NULL,
  currency TEXT NOT NULL,
  baseline_nanos INTEGER NULL CHECK(baseline_nanos >= 0),
  high_water_nanos INTEGER NULL CHECK(high_water_nanos >= 0),
  attributed_nanos INTEGER NULL CHECK(attributed_nanos >= 0),
  baseline_kind TEXT NOT NULL,
  scope TEXT NOT NULL,
  coverage TEXT NOT NULL,
  coverage_boundary TEXT NULL,
  last_report_id TEXT NULL,
  source_metadata TEXT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(origin, external_session, epoch)
);
CREATE INDEX remote_meters_owner ON remote_meters(owner_id);

-- §9: every existing live session becomes an accounting owner with unknown
-- historical coverage; lineage mirrors the current parent chain.
INSERT INTO cost_owners (id, root_id, parent_id, deleted_at, history_coverage, revision)
WITH RECURSIVE up(id, root, parent) AS (
  SELECT id, id, parent FROM sessions WHERE parent IS NULL
  UNION ALL
  SELECT s.id, up.root, s.parent FROM sessions s JOIN up ON s.parent = up.id
)
SELECT id, root, parent, NULL, 'unknown', 0 FROM up;
