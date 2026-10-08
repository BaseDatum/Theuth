-- 137 D1: a session's memory mode, `epochs` (07/11 as before) or `endless`.
ALTER TABLE sessions ADD COLUMN memory TEXT NOT NULL DEFAULT 'epochs';

-- 137 §4: the unit cache — one row per (message row, part) of an endless
-- session; `i` is assigned once and never renumbered. `turn` is the count of
-- rendered turns when the unit committed (the frozen-context key, D6).
CREATE TABLE endless_units (
  session_id TEXT    NOT NULL REFERENCES sessions(id),
  i          INTEGER NOT NULL,
  seq        INTEGER NOT NULL,
  part       INTEGER NOT NULL,
  kind       TEXT    NOT NULL,
  size       INTEGER NOT NULL,
  turn       INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (session_id, i)
);

-- 137 §5.1: the summary tree. node(0, i) = unit i; node(l, i) = merge of
-- (l-1, 2i) and (l-1, 2i+1). `tries` < 0 flags a node that exhausted its
-- tries without a guard-passing candidate (§6).
CREATE TABLE endless_nodes (
  session_id TEXT    NOT NULL REFERENCES sessions(id),
  l          INTEGER NOT NULL,
  i          INTEGER NOT NULL,
  text       TEXT    NOT NULL,
  size       INTEGER NOT NULL,
  free       INTEGER NOT NULL DEFAULT 0,
  model      TEXT    NULL,
  tries      INTEGER NOT NULL DEFAULT 0,
  created_at TEXT    NOT NULL,
  PRIMARY KEY (session_id, l, i)
);

-- 137 §6: every compactor attempt (the Memory panel's calls / trips / cost).
CREATE TABLE endless_calls (
  call_id      TEXT    PRIMARY KEY,
  session_id   TEXT    NOT NULL REFERENCES sessions(id),
  l            INTEGER NOT NULL,
  i            INTEGER NOT NULL,
  try          INTEGER NOT NULL,
  bytes        INTEGER NULL,
  guard        TEXT    NULL,
  usage_json   TEXT    NULL,
  usd_estimate REAL    NULL,
  latency_ms   INTEGER NOT NULL,
  created_at   TEXT    NOT NULL
);
CREATE INDEX endless_calls_session ON endless_calls(session_id, created_at);

-- 137 D9: `recall` — FTS5 over the level-0 lines, kept by triggers.
CREATE VIRTUAL TABLE endless_lines_fts USING fts5(
  text, session_id UNINDEXED, i UNINDEXED, tokenize = 'porter unicode61'
);
CREATE TRIGGER endless_nodes_ai AFTER INSERT ON endless_nodes WHEN NEW.l = 0 BEGIN
  INSERT INTO endless_lines_fts (text, session_id, i) VALUES (NEW.text, NEW.session_id, NEW.i);
END;
CREATE TRIGGER endless_nodes_ad AFTER DELETE ON endless_nodes WHEN OLD.l = 0 BEGIN
  DELETE FROM endless_lines_fts WHERE session_id = OLD.session_id AND i = OLD.i;
END;
CREATE TRIGGER endless_nodes_au AFTER UPDATE ON endless_nodes WHEN NEW.l = 0 BEGIN
  DELETE FROM endless_lines_fts WHERE session_id = OLD.session_id AND i = OLD.i;
  INSERT INTO endless_lines_fts (text, session_id, i) VALUES (NEW.text, NEW.session_id, NEW.i);
END;
