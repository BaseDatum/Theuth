-- 67 §4.5, §5.1: all three mesh tables attach to replication. `replica::attach`
-- needs a single-column primary key and an `hlc TEXT` column per table.
--
-- `peers`: add the clock column.
-- `availability`: composite key (observer, subject, from_ms) → `id` =
-- `observer|subject|from_ms`; the three columns stay, indexed, for the
-- ledger queries. Every engine writes only its own `observer` rows.
-- `engine_stats`: the row is now the whole advertisement (heartbeats carry
-- only `{id, is_hub, stats_hlc}`), so the election inputs that rode on the
-- heartbeat move here.
ALTER TABLE peers ADD COLUMN hlc TEXT;

ALTER TABLE engine_stats ADD COLUMN hub_pref TEXT NOT NULL DEFAULT 'eligible';
ALTER TABLE engine_stats ADD COLUMN first_seen_ms INTEGER NOT NULL DEFAULT 0;

CREATE TABLE availability_new (
    id        TEXT    PRIMARY KEY,           -- observer|subject|from_ms
    observer  TEXT    NOT NULL,
    subject   TEXT    NOT NULL,
    from_ms   INTEGER NOT NULL,
    to_ms     INTEGER NOT NULL,
    reachable INTEGER NOT NULL,
    hlc       TEXT
);
INSERT INTO availability_new (id, observer, subject, from_ms, to_ms, reachable)
    SELECT observer || '|' || subject || '|' || from_ms, observer, subject, from_ms, to_ms, reachable
    FROM availability;
DROP TABLE availability;
ALTER TABLE availability_new RENAME TO availability;
CREATE INDEX availability_subject ON availability (subject, to_ms);
CREATE INDEX availability_observer_subject ON availability (observer, subject, from_ms);
