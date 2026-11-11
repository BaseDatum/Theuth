-- 16 §5 (2026-11-11): a session a contact's turn spawned never reaches the owner's local
-- network through the web tools, nor do its subagents, also after a restart: every actor start
-- re-installs the registry's marker from here. Local table (never replicated).
CREATE TABLE no_lan_sessions (
    session_id TEXT PRIMARY KEY NOT NULL
);
