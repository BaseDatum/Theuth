-- 139 Revision 2 (HARNESS-ENVELOPE): engine-local, never in
-- `capture::ATTACHED` — the fence refuses captured writes for a session
-- owned elsewhere, and these rows are exactly about such sessions.

-- Harness-made inputs (a settled job's <job_result>, a fired <reminder>, a
-- lost child's result) for a session another engine owns, waiting to be sent
-- over `Msg::Harness`; a row goes when the owner acks it or when it is older
-- than `sessions.harness_outbox_ttl_secs`.
CREATE TABLE harness_outbox (
    id TEXT PRIMARY KEY,
    session TEXT NOT NULL,
    kind TEXT NOT NULL,
    origin TEXT NOT NULL,
    text TEXT NOT NULL,
    wrapped TEXT NOT NULL,
    scope TEXT,
    at INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);

-- Envelope ids this engine already handed to its supervisor (receiver-side
-- idempotency across a reconnect overlap or a restart); pruned past the TTL.
CREATE TABLE harness_seen (
    id TEXT PRIMARY KEY,
    at INTEGER NOT NULL
);
