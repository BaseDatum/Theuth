-- 69 decision 3: the paired-device registry as replicated records. Both
-- tables satisfy `replica::attach`: single-column primary key, nullable data
-- columns, `hlc TEXT` (the replica stamps it at capture when NULL).
--
-- `devices`: one row per device static key; revoke = `revoked_ms`, never a
-- delete (a delete loses to a newer column write). `last_seen_ms` is written
-- on session close only — each write is a replicated batch.
-- `pending`: one row per pairing shown (any engine may show one); the PSK
-- is sealed with the mesh key (the file is plaintext at rest and replicated),
-- aad = the row id. Consumed or expired rows are deleted (written once, so
-- the tombstone is always newer).
CREATE TABLE devices (
    id           TEXT PRIMARY KEY,   -- first 8 chars of base64url(static_pub)
    name         TEXT,
    platform     TEXT,
    static_pub   TEXT,               -- base64url, 43 chars
    paired_ms    INTEGER,
    last_seen_ms INTEGER,
    revoked_ms   INTEGER,
    hlc          TEXT
);
CREATE TABLE pending (
    id         TEXT PRIMARY KEY,     -- base64url of 16 random bytes
    psk_sealed TEXT,                 -- base64url(nonce ‖ XChaCha20-Poly1305(mesh key, aad = id, psk))
    name       TEXT,                 -- the harness name the QR shows
    created_ms INTEGER,
    expires_ms INTEGER,
    hlc        TEXT
);
