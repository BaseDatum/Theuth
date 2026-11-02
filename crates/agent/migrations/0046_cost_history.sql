-- 160 H1: a root delete keeps its books. The subtree's `cost_owners` are
-- tombstoned and its calls/meters/turns stay; this row keeps what the cost
-- dashboard needs to attribute them (spend_report's owner rules) once the
-- session row is gone. The chat's free-text title is not kept: `routine` is
-- the title prefix before ` · ` and only for an unattended root.
CREATE TABLE cost_root_tombstones (
  root_id TEXT PRIMARY KEY,
  desk TEXT NULL,
  unattended INTEGER NOT NULL DEFAULT 0,
  routine TEXT NULL,
  project TEXT NULL,
  created_at TEXT NULL,
  deleted_at TEXT NOT NULL
);

-- 160 H2: every applied remote meter report (cumulative nanos as reported,
-- engine arrival time), so placing meter money in time is exact beyond the
-- eight raw reports `remote_meters.source_metadata` keeps.
CREATE TABLE remote_meter_reports (
  meter_id TEXT NOT NULL REFERENCES remote_meters(id),
  amount_nanos INTEGER NOT NULL CHECK(amount_nanos >= 0),
  currency TEXT NOT NULL,
  at TEXT NOT NULL
);
CREATE INDEX remote_meter_reports_meter ON remote_meter_reports(meter_id, at);

-- 160 §4: `cost_calls` pages the ledger newest first; without this the
-- unfiltered page is `SCAN model_calls` + a temp b-tree sort (1.9 s on a
-- 143k-row ledger), with it an index range search (37 ms).
CREATE INDEX model_calls_created_at ON model_calls(created_at);
