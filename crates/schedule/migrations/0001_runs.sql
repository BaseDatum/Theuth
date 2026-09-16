CREATE TABLE runs (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,            -- file stem at the time; a renamed file is a new name
    session     TEXT,                     -- the schedule's session; NULL for a skip before the first fire
    fired_at    TEXT NOT NULL,            -- RFC 3339, local offset
    finished_at TEXT,                     -- NULL while running
    result      TEXT                      -- end_turn | error: <message> | timeout | cancelled | skipped: busy
);
CREATE INDEX runs_name ON runs (name, fired_at DESC);
