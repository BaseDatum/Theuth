-- Design doc 68 (sessions everywhere) §8: ownership on the session row, the
-- per-commit changeset log, peer cursors, the epoch fence, and text ids on
-- `errors` (integer rowids collide across engines). Forward-only; additive
-- except the `errors` rewrite. `owner = ''` means "this engine" until
-- `Store::open` backfills the mesh EngineId.
ALTER TABLE sessions ADD COLUMN owner TEXT NOT NULL DEFAULT '';
ALTER TABLE sessions ADD COLUMN epoch INTEGER NOT NULL DEFAULT 1;

-- One row per committed write transaction, per session it touched (68 §3):
-- `seq` is this engine's own log position; `changeset` is the sqlite3session
-- changeset restricted to that session's rows. `orphaned = 1` marks rows a
-- demoted owner wrote and will never ship (68 §4.3).
CREATE TABLE session_log (
  seq        INTEGER PRIMARY KEY,
  session_id TEXT    NOT NULL,
  epoch      INTEGER NOT NULL,
  changeset  BLOB    NOT NULL,
  hlc        TEXT    NOT NULL,
  orphaned   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX session_log_session ON session_log(session_id, epoch, seq);

-- Per origin log, how far each peer has applied; `peer = ''` is our own
-- applied cursor for a remote origin.
CREATE TABLE session_cursors (
  origin TEXT NOT NULL,
  peer   TEXT NOT NULL,
  seq    INTEGER NOT NULL,
  PRIMARY KEY (origin, peer)
);

-- Highest epoch seen per session with the owner that holds it (the fence).
CREATE TABLE session_epochs (
  session_id TEXT PRIMARY KEY,
  epoch      INTEGER NOT NULL,
  owner      TEXT NOT NULL
);

-- errors: INTEGER rowid -> TEXT id. Existing rows get random ids that keep
-- their relative order (zero-padded rowid prefix); new rows get uuid v7 text.
CREATE TABLE errors_new (
    id         TEXT    NOT NULL PRIMARY KEY,
    session_id TEXT    NOT NULL REFERENCES sessions(id),
    at_seq     INTEGER NOT NULL,
    text       TEXT    NOT NULL
);
INSERT INTO errors_new (id, session_id, at_seq, text)
  SELECT printf('%016x', id) || '-' || lower(hex(randomblob(8))), session_id, at_seq, text FROM errors;
DROP TABLE errors;
ALTER TABLE errors_new RENAME TO errors;
CREATE INDEX errors_by_seq ON errors(session_id, at_seq);

-- Decision 8: the global tables join the stage-1 record replica later.
ALTER TABLE projects    ADD COLUMN hlc TEXT;
ALTER TABLE cost_owners ADD COLUMN hlc TEXT;
