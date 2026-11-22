-- 208: the Attention graph. Engine-local: neither table is in store::capture::ATTACHED, so nothing replicates and
-- SESSIONS_WIRE_SCHEMA stays 70. No replicated table changes.

-- 208 §2.3: the time of each session's last bus event, upserted in 30 s batches with max().
CREATE TABLE session_last_event (
    session TEXT    PRIMARY KEY NOT NULL,
    at_ms   INTEGER NOT NULL
);

-- 208 §5: the window read of message history (`WHERE updated_at >= ?`).
CREATE INDEX peer_exchanges_updated ON peer_exchanges (updated_at);
