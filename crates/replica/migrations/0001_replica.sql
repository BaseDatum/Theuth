-- replica.sqlite: sync bookkeeping only (67 §5.1). The replicated rows live in
-- the owning plugin's own file; this file remembers what each peer has seen
-- and this engine's hybrid logical clock.

-- Per peer, per attached table: the last change-feed `seq` acknowledged.
CREATE TABLE sync_state (
    peer        TEXT    NOT NULL,
    table_name  TEXT    NOT NULL,
    last_seq    INTEGER NOT NULL DEFAULT 0,
    updated_ms  INTEGER NOT NULL,
    PRIMARY KEY (peer, table_name)
);

-- This engine's HLC, one row (id = 1), so a restart never goes backwards.
CREATE TABLE clock (
    id       INTEGER PRIMARY KEY CHECK (id = 1),
    wall_ms  INTEGER NOT NULL,
    counter  INTEGER NOT NULL
);
