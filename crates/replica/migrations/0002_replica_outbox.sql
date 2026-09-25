-- Every change batch this engine holds, own and relayed (67 §5.1): the
-- catch-up source for peers and the "already applied" set. A batch is applied
-- iff its (origin, seq) is here. `batch` is the postcard-encoded ChangeBatch.
CREATE TABLE outbox (
    origin  TEXT    NOT NULL,
    seq     INTEGER NOT NULL,
    batch   BLOB    NOT NULL,
    PRIMARY KEY (origin, seq)
);

-- Shadow clocks (WaveSyncDB's shape): the HLC of the write each replicated
-- column currently holds, so an incoming column change can be compared
-- per column. Rows are dropped when the row is deleted.
CREATE TABLE col_clock (
    table_name  TEXT NOT NULL,
    pk          TEXT NOT NULL,
    column_name TEXT NOT NULL,
    hlc         TEXT NOT NULL,
    PRIMARY KEY (table_name, pk, column_name)
);

-- Row tombstones: a delete's HLC. A column write older than the tombstone is
-- dropped; a newer one resurrects the row (when it carries every NOT NULL
-- column) and clears the tombstone.
CREATE TABLE tombstone (
    table_name  TEXT NOT NULL,
    pk          TEXT NOT NULL,
    hlc         TEXT NOT NULL,
    PRIMARY KEY (table_name, pk)
);

-- This replica's origin id: generated once, never changes; the `origin` of
-- every batch it produces and the tie-break in its HLC stamps.
CREATE TABLE identity (
    id    INTEGER PRIMARY KEY CHECK (id = 1),
    peer  TEXT NOT NULL
);
