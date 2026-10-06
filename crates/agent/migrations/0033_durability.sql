-- Design doc 109 §3.1: the durability journal. Six tables written inside the
-- same `WriteTx` as every other store write (D1), so one commit is one
-- `session_log` row and 68 replicates the journal for free. Times are RFC
-- 3339 text like `model_calls.created_at`, except `jobs` / `pty_screens`,
-- which use unix milliseconds for cheap liveness arithmetic.

-- 109 §3: one row per step of a turn. A turn is open while its newest row is
-- not `turn_finished`. `payload` is JSON whose shape depends on `kind`
-- (§3.2). `(session_id, turn_id, n)` orders the steps of one turn.
CREATE TABLE turn_steps (
    session_id  TEXT    NOT NULL REFERENCES sessions(id),
    turn_id     TEXT    NOT NULL,
    n           INTEGER NOT NULL,
    kind        TEXT    NOT NULL,   -- input_accepted | model_call | assistant_partial |
                                    -- tool_batch | turn_finished | suspended
    at_seq      INTEGER NOT NULL,   -- last message seq when the step was written
    call_id     TEXT,               -- model_calls.call_id for model_call / assistant_partial
    payload     TEXT    NOT NULL DEFAULT '{}',
    at          TEXT    NOT NULL,
    PRIMARY KEY (session_id, turn_id, n)
);
CREATE INDEX turn_steps_open ON turn_steps (session_id, kind, at);

-- 109 §3.3: one row per provider tool call, written in the same transaction
-- as the assistant `tool_use` row (planned), updated before the body runs
-- (started) and in the same transaction as the call's output (finished).
CREATE TABLE tool_calls (
    session_id  TEXT    NOT NULL REFERENCES sessions(id),
    call_id     TEXT    NOT NULL,
    turn_id     TEXT    NOT NULL,
    at_seq      INTEGER NOT NULL,   -- seq of the assistant row that carries the tool_use
    name        TEXT    NOT NULL,
    args        TEXT    NOT NULL,
    risk        TEXT    NOT NULL,   -- read_only | mutating | destructive (tools::Risk::as_str)
    state       TEXT    NOT NULL,   -- planned | started | finished | interrupted | skipped
    attempt     INTEGER NOT NULL DEFAULT 1,
    started_at  TEXT,
    finished_at TEXT,
    output      TEXT,               -- the spill-capped content (07 §6), NULL until finished
    is_error    INTEGER NOT NULL DEFAULT 0,
    files       TEXT    NOT NULL DEFAULT '[]',
    PRIMARY KEY (session_id, call_id)
);
CREATE INDEX tool_calls_turn ON tool_calls (session_id, turn_id, state);

-- 109 §6: a human wait. `tool_call_id` is the call that asked (NULL for an
-- approval raised by the registry before the call is `started`).
CREATE TABLE questions (
    id            TEXT    PRIMARY KEY,         -- ApprovalId / AskId / HandoffId / SecretRequestId
    session_id    TEXT    NOT NULL REFERENCES sessions(id),
    tool_call_id  TEXT,
    kind          TEXT    NOT NULL,            -- approval | ask | handoff | secret
    payload       TEXT    NOT NULL,            -- ApprovalRequested / AskRequested / … as JSON
    state         TEXT    NOT NULL,            -- pending | answered | superseded | timed_out
    asked_at      TEXT    NOT NULL,
    deadline      TEXT    NOT NULL,            -- asked_at + the kind's timeout
    answered_at   TEXT,
    answer        TEXT                         -- Decision / answers JSON / outcome
);
CREATE INDEX questions_pending ON questions (session_id, state);

-- 109 §4.4: what a `started` tool call is blocked on, so resume re-arms the
-- wait with the time left instead of declaring the call not run.
CREATE TABLE waits (
    session_id    TEXT    NOT NULL REFERENCES sessions(id),
    tool_call_id  TEXT    NOT NULL,
    kind          TEXT    NOT NULL,   -- peer_reply | child_result | job | pty_wait | question
    target        TEXT    NOT NULL,   -- exchange id / child session / job id / question id
    deadline      TEXT,               -- NULL = no deadline (a child result)
    payload       TEXT    NOT NULL DEFAULT '{}',   -- pty_wait: {pattern, cursor}; peer_reply: {request_id}
    PRIMARY KEY (session_id, tool_call_id)
);

-- 109 §5: every shell / PTY job, for accounting and last-known state — a job
-- is never revived. `pgid` is the leader's id (platform::ProcessTree::id), kept
-- so the reaper can kill a group a SIGKILL of the engine leaked.
CREATE TABLE jobs (
    session_id   TEXT    NOT NULL REFERENCES sessions(id),
    id           TEXT    NOT NULL,              -- j<n> per session
    kind         TEXT    NOT NULL,              -- shell | pty
    call_id      TEXT    NOT NULL,              -- the `shell` / `start` call
    command      TEXT    NOT NULL,
    name         TEXT,                          -- `start`'s label
    pgid         INTEGER,
    log          TEXT,                          -- log path; NULL for a quiet job
    started_ms   INTEGER NOT NULL,
    state        TEXT    NOT NULL,              -- running | settled | lost
    exit         TEXT,                          -- exited <n> | timeout | killed | lost
    settled_ms   INTEGER,
    PRIMARY KEY (session_id, id)
);
CREATE INDEX jobs_running ON jobs (state) WHERE state = 'running';

-- 109 §5.3: the last rendered screen of a PTY job, scrubbed (37 §3.4).
-- Engine-local: not in `capture::ATTACHED` (§3.5).
CREATE TABLE pty_screens (
    session_id   TEXT    NOT NULL REFERENCES sessions(id),
    job_id       TEXT    NOT NULL,
    revision     INTEGER NOT NULL,              -- PtyHandle.revision at capture
    rows         INTEGER NOT NULL,
    cols         INTEGER NOT NULL,
    cursor_row   INTEGER NOT NULL,
    cursor_col   INTEGER NOT NULL,
    text         TEXT    NOT NULL,              -- rows joined by '\n'
    at_ms        INTEGER NOT NULL,
    PRIMARY KEY (session_id, job_id),
    FOREIGN KEY (session_id, job_id) REFERENCES jobs (session_id, id) ON DELETE CASCADE
);

-- 109 §4.5: the turn's end survives the process (109-recon.md §8 gap 8).
ALTER TABLE sessions ADD COLUMN last_stop   TEXT NULL;   -- stop_reason of the last finished turn
ALTER TABLE sessions ADD COLUMN finished_at TEXT NULL;   -- when it finished
