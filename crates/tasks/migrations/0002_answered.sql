-- Chat messages already answered by some engine (67 §4.7): the hub that
-- answers marks first; a later holder skips what is marked. Replicated.
CREATE TABLE answered (
    id        TEXT    PRIMARY KEY,  -- "<sender>:<timestamp>" (replica wants one pk column)
    sender    TEXT    NOT NULL,
    timestamp INTEGER NOT NULL,
    hlc       TEXT
);
