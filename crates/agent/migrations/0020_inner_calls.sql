-- Design doc 08 as-built (2026-09-24): tool calls made from inside a `python`
-- cell (call id `<parent call id>.<n>`), so a reloaded transcript shows the
-- nested cards again. Never part of the model frame: the model only ever
-- sees the cell's own response. Inserted at `ToolCallStarted` (output NULL),
-- completed at `ToolCallFinished`; `at_seq` is the last visible message seq
-- when the call started, so commit_cut and fork treat rows like `errors`.
CREATE TABLE inner_calls (
    session_id TEXT    NOT NULL REFERENCES sessions(id),
    parent_id  TEXT    NOT NULL,
    n          INTEGER NOT NULL,
    at_seq     INTEGER NOT NULL,
    name       TEXT    NOT NULL,
    args       TEXT    NOT NULL,
    output     TEXT,
    is_error   INTEGER NOT NULL DEFAULT 0,
    files      TEXT    NOT NULL DEFAULT '[]',
    at         TEXT    NOT NULL,
    PRIMARY KEY (session_id, parent_id, n)
);
