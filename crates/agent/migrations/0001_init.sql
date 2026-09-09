CREATE TABLE sessions (
    id         TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    title      TEXT NOT NULL DEFAULT '',
    model      TEXT NOT NULL,
    parent     TEXT NULL REFERENCES sessions(id)
);

CREATE TABLE messages (
    session_id TEXT    NOT NULL REFERENCES sessions(id),
    seq        INTEGER NOT NULL,
    body       TEXT    NOT NULL,
    PRIMARY KEY (session_id, seq)
);
