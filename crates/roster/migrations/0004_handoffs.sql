-- 79 §6.6: an agent (in a VM or anywhere) asked a human to take its desktop.
-- Replicated (67 §5.1 rules: single-column PK, nullable columns, LWW via `hlc`)
-- so the hub's attention inbox sees it and `handoff_resolve` on any engine
-- lands in the asking engine's local copy, which the tool polls.

CREATE TABLE handoffs (
    id          TEXT PRIMARY KEY,          -- uuid
    engine      TEXT    NOT NULL,          -- mesh engine id of the asking session
    session     TEXT    NOT NULL,
    agent       TEXT,                      -- roster agent, when the session has one
    reason      TEXT    NOT NULL,          -- "log in to GitHub"
    url         TEXT,
    created_ms  INTEGER NOT NULL,
    resolved_ms INTEGER,
    outcome     TEXT,                      -- done | cancelled | timeout
    hlc         TEXT
);
