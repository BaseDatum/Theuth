-- 218 §5 WHITEBOARD. Node-local: not attached to the session log, so
-- SESSIONS_WIRE_SCHEMA is unchanged (70). Re-derivable: the engine that runs a
-- turn re-binds from the marker, the composer re-binds on every send.
CREATE TABLE IF NOT EXISTS session_whiteboards (
    session_root TEXT    NOT NULL,   -- root session id
    whiteboard   TEXT    NOT NULL,   -- DocId
    bound_ms     INTEGER NOT NULL,
    by           TEXT    NOT NULL,   -- 'marker' | 'composer' | 'tool' | 'rpc'
    PRIMARY KEY (session_root, whiteboard)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS session_whiteboards_wb ON session_whiteboards (whiteboard);
