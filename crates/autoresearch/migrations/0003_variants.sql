-- 159 §5.4 (wave C): every shared column and table the seven variants need. JSON columns hold
-- the serde form of the types.rs type named. Every default reads as a 140 row. Later waves use
-- reserved numbers (W1 = 0004 … W7 = 0010, W-h) only if they discover a need.
ALTER TABLE samples     ADD COLUMN case_values TEXT NOT NULL DEFAULT '{}';   -- {id: {name: value}}
ALTER TABLE samples     ADD COLUMN spend_nanos INTEGER NOT NULL DEFAULT 0;   -- Σ SPEND, USD nanos
ALTER TABLE experiments ADD COLUMN gate TEXT;              -- GateDecision {"v":2,…}; NULL = a 140 scalar row
ALTER TABLE experiments ADD COLUMN bench_spend_nanos INTEGER NOT NULL DEFAULT 0;
ALTER TABLE experiments ADD COLUMN parent_ref TEXT;        -- W1/W4: refs/autoresearch/<slug>/<kind>/<key>
ALTER TABLE experiments ADD COLUMN wait TEXT;              -- W6/W7: WaitInfo {until, why, windows}; NULL = never waited
ALTER TABLE studies     ADD COLUMN scores TEXT NOT NULL DEFAULT '{}';      -- {objective: mean} of the headline incumbent
ALTER TABLE studies     ADD COLUMN spec_v INTEGER NOT NULL DEFAULT 1;      -- 2 once any 159 field is set
ALTER TABLE studies     ADD COLUMN mechanism TEXT;         -- W7: pinned mechanism sha; NULL = compiled defaults

CREATE TABLE archive (                    -- W1 front members, W4 cells: one row per live elite
  slug TEXT NOT NULL, kind TEXT NOT NULL,  -- 'front' | 'cell'
  key TEXT NOT NULL,                       -- front 'f<n>'; cell 'i<island>:<bin>.<bin>'
  sha TEXT NOT NULL, n INTEGER NOT NULL, ref TEXT NOT NULL,
  scores TEXT NOT NULL DEFAULT '{}', descriptor TEXT NOT NULL DEFAULT '{}',
  island INTEGER, children INTEGER NOT NULL DEFAULT 0, at TEXT NOT NULL,
  PRIMARY KEY (slug, kind, key));
CREATE TABLE judgments (                  -- W2 scores (b_sha NULL), W3 pairs
  id INTEGER PRIMARY KEY AUTOINCREMENT, slug TEXT NOT NULL, n INTEGER NOT NULL,
  case_id TEXT NOT NULL, split TEXT NOT NULL, a_sha TEXT NOT NULL, b_sha TEXT, a_first INTEGER,
  model TEXT NOT NULL, score REAL, winner TEXT, reason TEXT NOT NULL DEFAULT '',
  human INTEGER NOT NULL DEFAULT 0, cost_nanos INTEGER NOT NULL DEFAULT 0, at TEXT NOT NULL);
CREATE INDEX judgments_study ON judgments(slug, n, case_id);
CREATE TABLE ratings (slug TEXT NOT NULL, segment INTEGER NOT NULL, sha TEXT NOT NULL,
  theta REAL NOT NULL, se REAL NOT NULL, games INTEGER NOT NULL, at TEXT NOT NULL,
  PRIMARY KEY (slug, segment, sha));     -- W3
CREATE TABLE citations (idea_id INTEGER PRIMARY KEY, url TEXT NOT NULL, title TEXT NOT NULL DEFAULT '',
  quote TEXT NOT NULL, verified INTEGER NOT NULL DEFAULT 0, fetched_at TEXT NOT NULL);  -- W5
CREATE TABLE launches (slug TEXT NOT NULL, n INTEGER NOT NULL, state TEXT NOT NULL,
  token_hash TEXT NOT NULL, exposure REAL NOT NULL, requested_at TEXT NOT NULL,
  decided_at TEXT, by TEXT, PRIMARY KEY (slug, n));   -- W6: requested|approved|refused|deployed|rolled_back
CREATE TABLE observations (id INTEGER PRIMARY KEY AUTOINCREMENT, slug TEXT NOT NULL, n INTEGER NOT NULL,
  arm TEXT NOT NULL, win INTEGER NOT NULL, start_at TEXT NOT NULL, end_at TEXT NOT NULL,
  units INTEGER NOT NULL, mean REAL, var REAL, raw TEXT NOT NULL DEFAULT '{}', at TEXT NOT NULL);  -- W6
