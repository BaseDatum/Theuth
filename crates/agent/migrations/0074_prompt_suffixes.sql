-- Prompt suffixes: the user's saved suffixes (`prompt_suffixes`) and each chat's "every message"
-- suffix (`session_suffixes`, keyed by session id) are mesh-wide records, replicated
-- last-writer-wins per column by the record replica (`Store::replicate_records`), like the
-- 191 contact tables. The replica's rules shape both: one primary-key column plus `hlc TEXT`,
-- no NOT NULL, UNIQUE, CHECK or FK constraint. Removal is a column (`deleted_ms`, set once and
-- never cleared), never a row delete, so a late edit from another machine cannot resurrect a
-- deleted suffix. Neither table is in the session log: `SESSIONS_WIRE_SCHEMA` stays.
CREATE TABLE prompt_suffixes (
    id TEXT PRIMARY KEY,
    name TEXT,
    body TEXT,
    created_ms INTEGER,
    deleted_ms INTEGER,
    hlc TEXT
);
-- `suffix` = a `prompt_suffixes.id`, NULL when the chat's suffix was turned off.
CREATE TABLE session_suffixes (
    session TEXT PRIMARY KEY,
    suffix TEXT,
    hlc TEXT
);
