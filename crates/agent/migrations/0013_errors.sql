-- Durable UI-only failures (provider, store, harness): one row per published
-- `AgentEvent::Error`, anchored at the last visible message seq when it
-- happened so it follows its turn through undo/redo/fork/commit_cut like
-- todo_snapshots. Never part of the model frame. `id` keeps the order of
-- several errors behind one anchor.
CREATE TABLE errors (
    id         INTEGER PRIMARY KEY,
    session_id TEXT    NOT NULL REFERENCES sessions(id),
    at_seq     INTEGER NOT NULL,
    text       TEXT    NOT NULL
);
CREATE INDEX errors_by_seq ON errors(session_id, at_seq);
