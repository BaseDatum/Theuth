-- 101 DECISION-SURFACE: the packet an agent prepares for the person who must
-- decide (`decision:<id>`) or review (`review:<task>`, the latest replaces).
-- `json` is the whole `packet::Packet`. Replicated like `tasks` (67 §5.1:
-- single-column PK, LWW via `hlc`, stamped by the store on every put).

CREATE TABLE packets (
    key    TEXT PRIMARY KEY,          -- decision:<uuid> | review:<task>
    task   TEXT    NOT NULL,
    kind   TEXT    NOT NULL,          -- decision | review
    json   TEXT    NOT NULL,
    at_ms  INTEGER NOT NULL,
    hlc    TEXT
);
CREATE INDEX packets_task ON packets (task);
