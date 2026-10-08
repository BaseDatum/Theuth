-- 140 §12.6: `<data_dir>/autoresearch/autoresearch.sqlite`. JSON columns hold
-- the serde form of the `types.rs` type named in the comment.

CREATE TABLE studies (
  slug TEXT PRIMARY KEY,
  state TEXT NOT NULL,              -- StudyState tag
  state_reason TEXT,                -- JSON PauseReason / failure text
  spec TEXT NOT NULL,               -- JSON StudySpec
  repo TEXT NOT NULL,
  base_sha TEXT,
  branch TEXT,
  worktree TEXT,
  schedule TEXT,                    -- schedule file stem `autoresearch-<slug>`
  session TEXT,                     -- bound study session (SessionId)
  origin TEXT,                      -- the chat the draft card lived in
  incumbent_sha TEXT,
  incumbent_mean REAL,
  sigma REAL,
  segment INTEGER NOT NULL DEFAULT 0,
  playbook TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  started_at TEXT,
  paused_ms INTEGER NOT NULL DEFAULT 0,
  finished_at TEXT,
  stop_reason TEXT,                 -- JSON StopReason
  report_doc TEXT,
  created_by TEXT
);

CREATE TABLE segments (
  slug TEXT NOT NULL,
  segment INTEGER NOT NULL,
  opened_at TEXT NOT NULL,
  opened_by TEXT NOT NULL,          -- By tag
  reason TEXT,
  custody TEXT NOT NULL DEFAULT '{}', -- JSON {path: sha256}
  baseline_mean REAL,
  baseline_n INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (slug, segment)
);

CREATE TABLE experiments (
  slug TEXT NOT NULL,
  n INTEGER NOT NULL,
  wave INTEGER,
  slot INTEGER,
  segment INTEGER NOT NULL,
  parent_sha TEXT NOT NULL,
  state TEXT NOT NULL,              -- ExperimentState
  verdict TEXT,                     -- JSON Verdict
  idea TEXT NOT NULL DEFAULT '',
  category TEXT NOT NULL DEFAULT '',
  hypothesis TEXT NOT NULL DEFAULT '',
  description TEXT NOT NULL DEFAULT '',
  learned TEXT NOT NULL DEFAULT '',
  next TEXT,
  decision TEXT,                    -- Decision
  justification TEXT,
  deviations TEXT NOT NULL DEFAULT '[]', -- JSON [path]
  delta REAL,
  delta_pct REAL,
  z REAL,
  n_samples INTEGER NOT NULL DEFAULT 0,
  metric REAL,
  commit_sha TEXT,
  patch_path TEXT,
  patch_hash TEXT,
  review TEXT,                      -- JSON ReviewVerdict
  line TEXT NOT NULL DEFAULT '',
  cost_nanos INTEGER NOT NULL DEFAULT 0,
  started_at TEXT NOT NULL,
  finished_at TEXT,
  PRIMARY KEY (slug, n)
);

CREATE TABLE samples (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  slug TEXT NOT NULL,
  n INTEGER NOT NULL,
  k INTEGER NOT NULL,
  tree_sha TEXT NOT NULL,
  role TEXT,                        -- SampleRole
  value REAL,
  secondaries TEXT NOT NULL DEFAULT '{}',
  cases TEXT NOT NULL DEFAULT '{}',
  asi TEXT NOT NULL DEFAULT '{}',
  exit INTEGER,
  wall_ms INTEGER NOT NULL DEFAULT 0,
  load REAL,
  noisy INTEGER NOT NULL DEFAULT 0,
  lost INTEGER NOT NULL DEFAULT 0,
  log_path TEXT,
  at TEXT NOT NULL
);
CREATE INDEX samples_study ON samples(slug, n, k);
CREATE INDEX samples_tree ON samples(slug, tree_sha);

CREATE TABLE ideas (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  slug TEXT NOT NULL,
  text TEXT NOT NULL,
  category TEXT NOT NULL DEFAULT '',
  source TEXT NOT NULL,             -- IdeaSource
  priority INTEGER NOT NULL DEFAULT 1,
  status TEXT NOT NULL DEFAULT 'open', -- IdeaStatus
  tried_in TEXT NOT NULL DEFAULT '[]',
  at TEXT NOT NULL
);
CREATE INDEX ideas_study ON ideas(slug, status, priority);

CREATE TABLE steers (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  slug TEXT NOT NULL,
  text TEXT NOT NULL,
  by TEXT NOT NULL,                 -- By
  at TEXT NOT NULL,
  seen_in INTEGER
);
CREATE INDEX steers_study ON steers(slug, id);

CREATE TABLE events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  slug TEXT NOT NULL,
  kind TEXT NOT NULL,               -- incident | drift | overfit | review | stop | finalize | …
  payload TEXT NOT NULL DEFAULT '{}',
  at TEXT NOT NULL
);
CREATE INDEX events_study ON events(slug, id);

-- 140 §7.3: the novelty check and dead-end dedup search idea + description + learned.
CREATE VIRTUAL TABLE experiments_fts USING fts5(
  slug UNINDEXED, n UNINDEXED, idea, description, learned, tokenize = 'porter unicode61'
);
CREATE TRIGGER experiments_ai AFTER INSERT ON experiments BEGIN
  INSERT INTO experiments_fts(slug, n, idea, description, learned)
  VALUES (NEW.slug, NEW.n, NEW.idea, NEW.description, NEW.learned);
END;
CREATE TRIGGER experiments_au AFTER UPDATE OF idea, description, learned ON experiments BEGIN
  DELETE FROM experiments_fts WHERE slug = OLD.slug AND n = OLD.n;
  INSERT INTO experiments_fts(slug, n, idea, description, learned)
  VALUES (NEW.slug, NEW.n, NEW.idea, NEW.description, NEW.learned);
END;
CREATE TRIGGER experiments_ad AFTER DELETE ON experiments BEGIN
  DELETE FROM experiments_fts WHERE slug = OLD.slug AND n = OLD.n;
END;
