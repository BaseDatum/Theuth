-- tasks.sqlite (67 §7). `tasks` and `wakes` are replicated (67 §5.1, `hlc`);
-- `task_events` is the field-level activity log; `write_budget` is the
-- per-session cross-task write cap (67 §7.3, decision 12), local only.

CREATE TABLE tasks (
    id                 TEXT PRIMARY KEY,     -- uuidv7
    title              TEXT    NOT NULL,
    brief              TEXT    NOT NULL DEFAULT '',
    owner              TEXT    NOT NULL,     -- agent:<name> | user
    state              TEXT    NOT NULL,     -- todo | doing | review | blocked | done | cancelled
    reviewer           TEXT,                 -- agent:<name> | user | NULL
    blockers_json      TEXT    NOT NULL DEFAULT '[]',
    parent             TEXT,
    project            TEXT,                 -- agent's projects.name label (51 §3); NULL = none
    where_             TEXT,                 -- engine name
    harness            TEXT,                 -- native | omp | opencode | claude
    external_json      TEXT    NOT NULL DEFAULT '[]',
    thread             TEXT,                 -- document id (59)
    checkout_json      TEXT,                 -- { session, engine, epoch, progress_at_ms }
    schedule_name      TEXT,
    schedule_fire_ms   INTEGER,
    created_by         TEXT    NOT NULL,
    created_ms         INTEGER NOT NULL,
    updated_ms         INTEGER NOT NULL,
    no_progress_count  INTEGER NOT NULL DEFAULT 0,
    next_wake_after_ms INTEGER,
    hlc                TEXT
);
CREATE INDEX tasks_project ON tasks (project);
CREATE INDEX tasks_owner_state ON tasks (owner, state);
CREATE UNIQUE INDEX tasks_schedule ON tasks (schedule_name, schedule_fire_ms)
    WHERE schedule_name IS NOT NULL;

CREATE TABLE wakes (
    id         TEXT PRIMARY KEY,
    agent      TEXT    NOT NULL,
    task       TEXT,
    reason     TEXT    NOT NULL,             -- assigned | comment | mention | schedule | answer | review | resume | manual
    key        TEXT    NOT NULL UNIQUE,      -- coalescing key: agent|task|reason
    count      INTEGER NOT NULL DEFAULT 1,
    claimed_by TEXT,                         -- desk session id
    created_ms INTEGER NOT NULL,
    hlc        TEXT
);
CREATE INDEX wakes_agent ON wakes (agent, claimed_by, created_ms);

CREATE TABLE task_events (
    id      INTEGER PRIMARY KEY,
    task    TEXT    NOT NULL,
    at_ms   INTEGER NOT NULL,
    actor   TEXT    NOT NULL,
    session TEXT,
    field   TEXT    NOT NULL,
    before  TEXT,
    after   TEXT
);
CREATE INDEX task_events_task ON task_events (task, at_ms);

CREATE TABLE write_budget (
    session TEXT NOT NULL,
    task    TEXT NOT NULL,
    PRIMARY KEY (session, task)
);
