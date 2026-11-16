-- Immutable root memory provenance, replicated with sessions.
ALTER TABLE sessions ADD COLUMN knowledge_identity TEXT NULL;
