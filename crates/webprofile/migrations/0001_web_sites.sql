-- 209 §4.1: one browser profile per owner mesh, one row per site (registrable domain). Satisfies
-- `replica::attach`: single-column primary key, nullable data columns, `hlc TEXT` (stamped at capture).
--
-- `id`: first 32 hex of sha256(salt ‖ "\n" ‖ site), salt = vault secret `webprofile/salt`, so the file
-- at rest does not name the sites. `sealed`: base64url(nonce ‖ XChaCha20-Poly1305(mesh key,
-- aad = "web_sites\n" ‖ id, SiteJar JSON)); NULL = forgotten (a column write, never a DELETE: a
-- delete loses to a newer column write). The row `id = 'epoch'` carries the profile epoch.
CREATE TABLE web_sites (
    id          TEXT PRIMARY KEY,
    sealed      TEXT,
    epoch       INTEGER NOT NULL,
    bytes       INTEGER,
    updated_ms  INTEGER,
    writer      TEXT,
    hlc         TEXT
);
