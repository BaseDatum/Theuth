-- 70 HUD: the append-only activity feed (engine-local; replicated later).
-- Fed by tailing `task_events` (every task change already lands there) and by
-- `activity::Record` bus messages from other plugins (roster, schedule).

CREATE TABLE activity (
    id      INTEGER PRIMARY KEY,
    ts_ms   INTEGER NOT NULL,
    kind    TEXT    NOT NULL,     -- activity::ActivityKind as snake_case
    actor   TEXT    NOT NULL,     -- agent:<name> | user | system
    subject TEXT    NOT NULL,     -- task / agent / session / routine id
    summary TEXT    NOT NULL,
    cost    REAL
);
CREATE INDEX activity_ts ON activity (ts_ms);

-- The tail's position in `task_events`.
CREATE TABLE activity_cursor (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    last_event INTEGER NOT NULL
);
