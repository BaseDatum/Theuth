-- 191 (D4, P3): first-contact holds, mirrored by the machine that owns the held chat so every
-- machine of the mesh shows them (Attention, the allow strip). A record-replica table: one
-- primary-key column plus `hlc TEXT`, no NOT NULL, UNIQUE, CHECK or FK constraint.
--
-- `id` = the A2A task id of the oldest held task of one (contact, export) on its owner;
-- `contact` is the alias, `export` the export's name, `owner` the owning engine id,
-- `waiting` how many tasks wait, `preview` the oldest's first line (cut to 200 bytes),
-- `until_ms` when the newest one expires. Written and deleted only by `owner` (and all of its
-- rows at its boot); every reader cleans and cuts `machine` and `preview`.
CREATE TABLE contact_holds (
    id TEXT PRIMARY KEY,
    contact TEXT,
    machine TEXT,
    export TEXT,
    session TEXT,
    owner TEXT,
    since_ms INTEGER,
    waiting INTEGER,
    preview TEXT,
    until_ms INTEGER,
    hlc TEXT
);
-- The inbox-marker lookup (D3, D4). An index adds no column: `SESSIONS_WIRE_SCHEMA` stays.
CREATE INDEX sessions_contact_inbox ON sessions(contact_inbox) WHERE contact_inbox IS NOT NULL;
