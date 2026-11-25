-- 221 §12.4: one row per Bitwarden Send this engine created. Engine-local (not in store::TABLES):
-- the Send belongs to this engine's connection. Never the content; the link is a vault secret
-- (`link_secret`), or, with `send_link_to_agent`, a sealed engine-local row named by `link_meta`.
CREATE TABLE external_sends (
    id              TEXT PRIMARY KEY,
    conn            TEXT NOT NULL,
    send_id         TEXT NOT NULL,
    access_id       TEXT NOT NULL,
    kind            TEXT NOT NULL,          -- text | file
    name            TEXT NOT NULL,
    file_name       TEXT,
    file_bytes      INTEGER,
    source_ref      TEXT NOT NULL,
    source_version  TEXT,
    link_secret     TEXT,
    link_meta       TEXT,
    password_secret TEXT,
    created_ms      INTEGER NOT NULL,
    expiration_ms   INTEGER,
    deletion_ms     INTEGER NOT NULL,
    max_access      INTEGER,
    session         TEXT NOT NULL,
    call_id         TEXT NOT NULL,
    revoked_ms      INTEGER
);
CREATE INDEX external_sends_conn ON external_sends (conn, created_ms);
CREATE UNIQUE INDEX external_sends_link ON external_sends (link_secret) WHERE link_secret IS NOT NULL;
