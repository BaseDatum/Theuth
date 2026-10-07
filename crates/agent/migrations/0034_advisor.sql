-- 125 §6.1: the kind of a session row; `advisor` rows are hidden children (ruling 1).
ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'chat';
-- 125 §4.1/§4.14: one advisor link per advised session (the "advised row stores" of D7 as a 1:1
-- side table rather than five nullable columns spliced into `session_cols!()`).
CREATE TABLE session_advisors (
  session    TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
  advisor    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  model      TEXT NOT NULL,
  effort     TEXT NULL,
  cursor     INTEGER NOT NULL DEFAULT 0,
  state      TEXT NOT NULL,            -- idle | reviewing | paused | halted
  updated_at TEXT NOT NULL
);
-- 125 §5.4: typed advisory metadata beside a user row (the `vision` pattern, 0014_vision.sql);
-- JSON `Advisory`; NULL for every ordinary message. Only this column makes a row an Advisory card.
ALTER TABLE messages ADD COLUMN advisory TEXT NULL;
