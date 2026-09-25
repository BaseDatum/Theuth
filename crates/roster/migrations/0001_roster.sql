-- roster.sqlite (67 §6). `agents` mirrors <data_dir>/agents/<name>.md and is
-- replicated (67 §5.1, `hlc`; a hand edit is a record write). `episodes` is
-- the desk history (67 §6.2); `proposals` the pending hires (67 §6.5).

CREATE TABLE agents (
    name              TEXT PRIMARY KEY,     -- [a-z0-9-]{1,32}
    front_matter_json TEXT    NOT NULL,     -- AgentFile without `body`
    body              TEXT    NOT NULL,     -- the charter
    mtime_ms          INTEGER NOT NULL,
    hlc               TEXT
);

CREATE TABLE episodes (
    id        TEXT PRIMARY KEY,             -- uuidv7
    agent     TEXT    NOT NULL,
    session   TEXT    NOT NULL,             -- the desk session
    opened_ms INTEGER NOT NULL,
    closed_ms INTEGER,
    reason    TEXT                          -- idle | context | thread_end | lease | shutdown
);
CREATE INDEX episodes_agent ON episodes (agent, opened_ms);

CREATE TABLE proposals (
    name        TEXT PRIMARY KEY,
    proposed_by TEXT    NOT NULL,           -- agent:<name>
    approval_id TEXT    NOT NULL,           -- the hire_agent approval (00 §1.7)
    created_ms  INTEGER NOT NULL
);
