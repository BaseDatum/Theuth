CREATE TABLE epochs (
    session_id TEXT    NOT NULL REFERENCES sessions(id),
    n          INTEGER NOT NULL,          -- 1, 2, … (epoch 0 has no row)
    at_seq     INTEGER NOT NULL,          -- last message seq when compaction ran (UI divider position)
    tail_from  INTEGER NOT NULL,          -- first message seq replayed after the head
    head       TEXT    NOT NULL,          -- ChatMessage JSON
    created_at TEXT    NOT NULL,
    PRIMARY KEY (session_id, n)
);
