-- 183 §7.6, Z3-R2: a context row whose session is a contact's inbox (an agent export's
-- fresh capped root), as opposed to an exported chat's context. Local table (never
-- replicated, SESSIONS_WIRE_SCHEMA unchanged): every actor start of such a session
-- re-installs its default-deny limits before anything runs. `inbox_identity`: the exported
-- agent's persona at its last admission, which a resumed turn speaks with (Z3-R3).
ALTER TABLE a2a_contexts ADD COLUMN inbox INTEGER NOT NULL DEFAULT 0 CHECK (inbox IN (0, 1));
ALTER TABLE a2a_contexts ADD COLUMN inbox_identity TEXT;
-- An inbox opened by an E3 build: its first contact row carries the `<contact_inbox …/>`
-- marker (an exported chat's rows never do; a stray match only caps that chat).
UPDATE a2a_contexts SET inbox = 1
WHERE session IN (SELECT session_id FROM messages WHERE body LIKE '%<contact_inbox %');
