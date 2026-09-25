-- mesh.sqlite (67 §4): the peer set, what every engine advertises, the
-- availability observations the stability score is computed from, and the
-- lease history. `peers` and `engine_stats` are replicated (67 §5.1, `hlc`).

CREATE TABLE peers (
    id         TEXT PRIMARY KEY,            -- iroh endpoint id
    name       TEXT    NOT NULL,
    addrs_json TEXT    NOT NULL DEFAULT '[]',
    hub_pref   TEXT    NOT NULL DEFAULT 'eligible',  -- prefer | eligible | never
    added_ms   INTEGER NOT NULL
);

CREATE TABLE engine_stats (
    id              TEXT PRIMARY KEY,
    uptime_since_ms INTEGER NOT NULL,
    class           TEXT    NOT NULL,       -- server | desktop | laptop | container
    harnesses_json  TEXT    NOT NULL DEFAULT '[]',
    repos_json      TEXT    NOT NULL DEFAULT '[]',
    display         INTEGER NOT NULL DEFAULT 0,
    gpu             INTEGER NOT NULL DEFAULT 0,
    updated_ms      INTEGER NOT NULL,
    hlc             TEXT
);

-- Reachability of `subject` as seen by `observer` over [from_ms, to_ms).
CREATE TABLE availability (
    observer  TEXT    NOT NULL,
    subject   TEXT    NOT NULL,
    from_ms   INTEGER NOT NULL,
    to_ms     INTEGER NOT NULL,
    reachable INTEGER NOT NULL,
    PRIMARY KEY (observer, subject, from_ms)
);

CREATE TABLE lease_log (
    at_ms  INTEGER NOT NULL,
    holder TEXT,                              -- NULL = released
    reason TEXT    NOT NULL
);
