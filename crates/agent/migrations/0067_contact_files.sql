-- 199 CONTACT-FILES. Node-local: not attached to the session log, so
-- SESSIONS_WIRE_SCHEMA is unchanged (67).
CREATE TABLE contact_files (
    id           TEXT PRIMARY KEY,  -- this engine's local id, 32 lowercase hex (random)
    direction    TEXT NOT NULL,     -- out | in
    session      TEXT NOT NULL,
    exchange     TEXT NOT NULL,     -- the exchange that carried it (in) or sent it (out)
    seq          INTEGER NOT NULL,  -- order within the message
    contact_id   TEXT NOT NULL,     -- ContactFacts.id, never the alias
    name         TEXT NOT NULL,     -- sanitised
    media_type   TEXT NOT NULL,
    size         INTEGER,           -- declared, then actual; NULL only for a P3 download in flight
    blake3       TEXT,              -- declared or computed; actual once received
    source       TEXT NOT NULL,     -- out: original path; in: the source URL (holds the sender's id)
    clone        TEXT,              -- out: clone path, if any
    dev          INTEGER,           -- out: the gate handle's; in: the partial's
    inode        INTEGER,
    path         TEXT,              -- in: target path, set before linking
    done         INTEGER NOT NULL DEFAULT 0,
    state        TEXT NOT NULL,     -- in: queued|receiving|publishing|saved|received|failed
                                    -- out: queued|sending|sent|failed
    error        TEXT,
    notice       TEXT,              -- in: the exact notice text, written with the terminal state
    notice_state TEXT,              -- in: queued | sent | delivered
    created_ms   INTEGER NOT NULL,
    deadline_ms  INTEGER NOT NULL
);
CREATE INDEX contact_files_session ON contact_files (session);
CREATE INDEX contact_files_exchange ON contact_files (exchange);
-- Both unfinished outbox states: on restart `queued` and `sent` notices resume.
CREATE INDEX contact_files_outbox ON contact_files (notice_state) WHERE notice_state IN ('queued', 'sent');
