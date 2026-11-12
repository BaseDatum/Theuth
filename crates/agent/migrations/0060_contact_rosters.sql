-- 191 (D1, D2, P2): signed rosters. The replicated tables follow the record replica's rules
-- (one primary-key column plus `hlc TEXT`, no NOT NULL, UNIQUE, CHECK or FK constraint);
-- validity is the a2a reconcile's.
--
-- `contact_rosters` (ours only): one signed copy per row, `id` =
-- `<contact>|<chain>|<version>|<signer>`, immutable once written (the reconcile keys on
-- `(count, max hlc)`, never the content). `roster` is the copy as JSON
-- (`{chain, version, members, signer, sig}`), `hash` the hex SHA-256 of its content.
CREATE TABLE contact_rosters (
    id TEXT PRIMARY KEY,
    contact TEXT,
    chain TEXT,
    version INTEGER,
    signer TEXT,
    hash TEXT,
    roster TEXT,
    hlc TEXT
);
-- `contact_roster_heads`: `id` = `<record>|<machine eid>`, written only by the machine it
-- names: its accepted head of the other side's chain (`history`: the last 64
-- `[version, hash]` pairs, JSON), the head's members (untrusted dial hints) and its chain
-- (an untrusted hint too), `conflict` (0/1), and `at_ms`, when it was last written (a
-- conflict not written again within three refreshes expires).
CREATE TABLE contact_roster_heads (
    id TEXT PRIMARY KEY,
    record TEXT,
    machine TEXT,
    chain TEXT,
    version INTEGER,
    hash TEXT,
    history TEXT,
    members TEXT,
    conflict INTEGER,
    at_ms INTEGER,
    hlc TEXT
);
-- Local (never attached): this machine's own verified state of the other side's chain, per
-- record. `roster` is the head as JSON (`{members, signers, base}`), `history` the last 64
-- `[version, hash, members]` entries, the head last.
CREATE TABLE contact_roster_local (
    record TEXT PRIMARY KEY,
    chain TEXT NOT NULL,
    version INTEGER NOT NULL,
    hash TEXT NOT NULL,
    roster TEXT NOT NULL,
    history TEXT NOT NULL
);
-- The machine of ours that ran the ceremony writing this pin (a roster's proof path starts
-- from the pins the contact holds).
ALTER TABLE contact_endpoints ADD COLUMN pinned_by TEXT;
-- Superseded by the signed rosters (D1).
DROP TABLE contact_votes;
