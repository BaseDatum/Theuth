-- 180 D1b: one table's changes of an applied batch that could not be written
-- here (a constraint a local row breaks). The batch itself is recorded (the
-- stream moves on); these changes are retried, under last-writer-wins, after
-- every later batch that touches the table and when the table attaches, and
-- dropped once they apply. `changes` is the postcard-encoded Vec<Change>.
CREATE TABLE parked (
    origin  TEXT    NOT NULL,
    seq     INTEGER NOT NULL,
    tbl     TEXT    NOT NULL,
    changes BLOB    NOT NULL,
    PRIMARY KEY (origin, seq, tbl)
);
