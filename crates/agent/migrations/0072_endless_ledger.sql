-- 201 §9 (P6a): the state ledger of an endless chat session (JSON body,
-- `endless::ledger::Ledger`), and each generation's frozen ledger snapshot
-- beside its frozen view (NULL: the ledger was off for that generation).
-- Node-local like endless_generations: SESSIONS_WIRE_SCHEMA stays.
CREATE TABLE endless_ledger (
    session_id TEXT PRIMARY KEY NOT NULL,
    body TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
ALTER TABLE endless_generations ADD COLUMN ledger TEXT;
