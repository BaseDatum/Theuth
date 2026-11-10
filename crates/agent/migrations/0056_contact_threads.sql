-- 186 (D5): a contact share in full is a conversation both ways. The sender mints its
-- A2A `contextId` per (originating session, contact, export); each side keeps it here
-- with the session it belongs to, and every exchange of it names it. The receiver's
-- answers come back over the contact-only return route, admitted while the thread lives
-- (7 days, sliding on each message) and the share is still full.
ALTER TABLE peer_exchanges ADD COLUMN context_id TEXT;
-- 186: the share mode of a contact's message (and of the answers it gets), for the row's
-- provenance; NULL for mesh and HTTP peers.
ALTER TABLE peer_exchanges ADD COLUMN contact_mode TEXT CHECK (contact_mode IN ('full', 'cautious'));

CREATE TABLE contact_threads (
    binding TEXT NOT NULL,
    context_id TEXT NOT NULL,
    session TEXT NOT NULL,
    export TEXT NOT NULL,
    outbound INTEGER NOT NULL CHECK (outbound IN (0, 1)),
    last_ms INTEGER NOT NULL,
    PRIMARY KEY (binding, context_id)
);
CREATE INDEX contact_threads_origin ON contact_threads (binding, session, export, outbound);
-- 186 (D6): an agent export's inbox shared in full runs under the agent's own policy, not
-- the contact limits; every actor start reads this before it re-installs anything.
ALTER TABLE a2a_contexts ADD COLUMN inbox_full INTEGER NOT NULL DEFAULT 0 CHECK (inbox_full IN (0, 1));
