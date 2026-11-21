-- 201 §3/§4.3 (P2): one row per generation of an endless session — the
-- frozen view a generation's turns open with, reused byte for byte until the
-- next trim and after a restart. `t_cut` is the first unit the view does not
-- cover (the tail starts there), `first_seq` the first message row still in
-- the tail, `blocks` the rendered view as JSON `[[text, mark], …]`.
-- Node-local (not attached to the session log): SESSIONS_WIRE_SCHEMA stays.
CREATE TABLE endless_generations (
    session_id TEXT NOT NULL,
    n INTEGER NOT NULL,
    t_cut INTEGER NOT NULL,
    first_seq INTEGER NOT NULL,
    blocks TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (session_id, n)
);
