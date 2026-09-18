-- Structured todo plan (design doc 47 §5.2): one row per committed snapshot,
-- anchored at the history seq that produced it (a tool response, or the
-- compaction `at_seq` for a clear). `k` orders snapshots sharing an anchor so a
-- clear follows a todo update at the same seq. Visible iff at_seq < cut_at, like
-- messages/epochs. Legacy sessions have no rows: the plan starts empty and is
-- never backfilled from old tool text.
CREATE TABLE todo_snapshots (
    session_id TEXT    NOT NULL REFERENCES sessions(id),
    at_seq     INTEGER NOT NULL,
    k          INTEGER NOT NULL,
    source     TEXT    NOT NULL,   -- 'native' | 'acp'
    items      TEXT    NOT NULL,   -- Vec<Todo> JSON
    PRIMARY KEY (session_id, at_seq, k)
);
-- Monotonic projection version of the visible plan: bumps on every material
-- change, including undo/redo selecting an older snapshot.
ALTER TABLE sessions ADD COLUMN todo_version INTEGER NOT NULL DEFAULT 0;
