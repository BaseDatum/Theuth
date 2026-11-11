-- 191 (D2): contacts are mesh-wide records, replicated last-writer-wins per column by the
-- record replica (`Store::replicate_records`). The replica's rules shape every table: one
-- primary-key column plus `hlc TEXT`, and no NOT NULL, UNIQUE, CHECK or FK constraint (a
-- remote INSERT that fails one is skipped forever). Validity and uniqueness are the a2a
-- reconcile's: a bad row is quarantined there, never refused here. Removal is a column
-- (`removed_ms`), never a row delete.
CREATE TABLE contacts (
    id TEXT PRIMARY KEY,
    alias TEXT,
    name TEXT,
    machine TEXT,
    binding TEXT,
    paired_ms INTEGER,
    paired_on TEXT,
    blocked INTEGER,
    removed_ms INTEGER,
    merged_into TEXT,
    hlc TEXT
);
-- `id` = `<contact>|<eid>`: an endpoint a human pinned in a ceremony (an anchor).
CREATE TABLE contact_endpoints (
    id TEXT PRIMARY KEY,
    contact TEXT,
    eid TEXT,
    paired_ms INTEGER,
    barred_ms INTEGER,
    hlc TEXT
);
-- `id` = `<contact>|<voter eid>|<at>`: written from P2 on (members votes).
CREATE TABLE contact_votes (
    id TEXT PRIMARY KEY,
    contact TEXT,
    voter TEXT,
    at INTEGER,
    members TEXT,
    hlc TEXT
);
-- `id` = `<contact>|<export id>`; `first_contact` NULL = ask.
CREATE TABLE contact_grants (
    id TEXT PRIMARY KEY,
    contact TEXT,
    export TEXT,
    granted INTEGER,
    first_contact TEXT,
    hlc TEXT
);
-- `id` minted when the export is made; `name` is the card skill and `/agents/<name>` path,
-- made unique by the reconcile; `home` the engine id of the machine that made it.
CREATE TABLE a2a_exports (
    id TEXT PRIMARY KEY,
    name TEXT,
    session TEXT,
    agent TEXT,
    title TEXT,
    description TEXT,
    home TEXT,
    removed_ms INTEGER,
    hlc TEXT
);
-- Local (never attached): the direct addresses this machine last saw for a contact's
-- endpoint. They differ by LAN, so they would flap if replicated.
CREATE TABLE contact_addrs (
    eid TEXT PRIMARY KEY,
    addrs TEXT NOT NULL,
    at_ms INTEGER NOT NULL
);
-- 191 (D4): `<binding>|<export id>` on an agent export's inbox session. `sessions` is a
-- session-log table, so the marker travels with the session row.
ALTER TABLE sessions ADD COLUMN contact_inbox TEXT;

-- 191 (D2): `contact_threads` joins the record replica: one key column (`<binding>|<context_id>`)
-- and no constraints.
CREATE TABLE contact_threads_191 (
    id TEXT PRIMARY KEY,
    binding TEXT,
    context_id TEXT,
    session TEXT,
    export TEXT,
    outbound INTEGER,
    last_ms INTEGER,
    hlc TEXT
);
INSERT INTO contact_threads_191 (id, binding, context_id, session, export, outbound, last_ms)
    SELECT binding || '|' || context_id, binding, context_id, session, export, outbound, last_ms
    FROM contact_threads;
DROP TABLE contact_threads;
ALTER TABLE contact_threads_191 RENAME TO contact_threads;
CREATE INDEX contact_threads_origin ON contact_threads (binding, session, export, outbound);
