-- 210 §5.8: the edge's whole schema, including tables later milestones fill (cert_jobs, buckets, counters,
-- usage, dns_records). Routes live in memory only (§5.4).
-- Deviation: `names.owner` keeps the previous owner while `state = 'released'`, because only that mesh may
-- claim the name again before `reclaim_after_ms` (§5.2); every "held" query filters on `state`.
CREATE TABLE meshes (mesh TEXT PRIMARY KEY /* mesh id */, created_ms INTEGER NOT NULL, net TEXT NOT NULL,
  asn INTEGER, invited INTEGER NOT NULL DEFAULT 0, rekeyed_to TEXT, rekeyed_ms INTEGER, stop_all_ms INTEGER,
  banned TEXT /* json {reason, ms, by} */);
CREATE TABLE mesh_engines (mesh TEXT NOT NULL REFERENCES meshes(mesh), engine TEXT NOT NULL,
  first_ms INTEGER NOT NULL, last_ms INTEGER NOT NULL, net TEXT NOT NULL, revoked_ms INTEGER,
  PRIMARY KEY (mesh, engine));
CREATE TABLE names (domain TEXT NOT NULL, name TEXT NOT NULL,
  kind  TEXT NOT NULL CHECK (kind IN ('random','reserved')),
  owner TEXT REFERENCES meshes(mesh),
  state TEXT NOT NULL CHECK (state IN ('allocated','queued','slotted','valid','released','banned')),
  created_ms INTEGER NOT NULL, last_attach_ms INTEGER, first_live_ms INTEGER,
  transfer_to TEXT, transfer_until_ms INTEGER, released_ms INTEGER, reclaim_after_ms INTEGER,
  csr BLOB, csr_spki BLOB,
  ca TEXT, chain_pem TEXT, not_after_ms INTEGER, ari_start_ms INTEGER, ari_end_ms INTEGER,
  records INTEGER NOT NULL DEFAULT 0,
  net TEXT NOT NULL, asn INTEGER, banned TEXT, PRIMARY KEY (domain, name));
CREATE INDEX names_owner ON names(owner);
CREATE TABLE cert_jobs (id TEXT PRIMARY KEY /* uuid v7 */, domain TEXT NOT NULL, name TEXT NOT NULL,
  kind  TEXT NOT NULL CHECK (kind IN ('new','renew','rotate')),
  state TEXT NOT NULL CHECK (state IN ('queued','ordering','done','failed','dropped')),
  class TEXT NOT NULL CHECK (class IN ('invited','open')),
  mesh TEXT NOT NULL, net TEXT NOT NULL, first_of_mesh INTEGER NOT NULL, ca TEXT, order_url TEXT,
  enqueued_ms INTEGER NOT NULL, lease_until_ms INTEGER, attempts INTEGER NOT NULL DEFAULT 0,
  next_at_ms INTEGER NOT NULL, last_error TEXT);
CREATE INDEX cert_jobs_ready ON cert_jobs(state, class, next_at_ms);
CREATE TABLE buckets (name TEXT PRIMARY KEY /* le_invited, le_open, open_new_names, relayed_new_meshes */,
  tokens REAL NOT NULL, updated_ms INTEGER NOT NULL);
CREATE TABLE counters (scope TEXT NOT NULL, key TEXT NOT NULL,
  period TEXT NOT NULL CHECK (period IN ('day','week','month')), start_ms INTEGER NOT NULL,
  count INTEGER NOT NULL, PRIMARY KEY (scope, key, period, start_ms));
CREATE TABLE usage (day INTEGER NOT NULL, mesh TEXT NOT NULL, net TEXT NOT NULL, invited INTEGER NOT NULL,
  bytes INTEGER NOT NULL, PRIMARY KEY (day, mesh, net));
CREATE TABLE invites (hash BLOB PRIMARY KEY, uses_left INTEGER NOT NULL, expires_ms INTEGER NOT NULL,
  created_ms INTEGER NOT NULL, note TEXT);
CREATE TABLE bans (kind TEXT NOT NULL, value TEXT NOT NULL, reason TEXT NOT NULL, ms INTEGER NOT NULL,
  by TEXT NOT NULL, PRIMARY KEY (kind, value));
CREATE TABLE dns_records (host TEXT NOT NULL, type TEXT NOT NULL, value TEXT NOT NULL, name TEXT,
  provider_id TEXT, created_ms INTEGER NOT NULL, PRIMARY KEY (host, type, value));
CREATE TABLE conn_log (ms INTEGER NOT NULL, sni TEXT NOT NULL, visitor TEXT NOT NULL, mesh TEXT, engine TEXT,
  bytes_up INTEGER NOT NULL, bytes_down INTEGER NOT NULL);
CREATE INDEX conn_log_ms ON conn_log(ms);
CREATE TABLE audit (ms INTEGER NOT NULL, cmd TEXT NOT NULL, args TEXT NOT NULL, ref TEXT);
