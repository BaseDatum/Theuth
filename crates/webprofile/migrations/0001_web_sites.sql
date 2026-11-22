-- 209 §4.1: one browser profile per owner mesh, one row per cookie and one per origin's
-- localStorage snapshot, so replica's per-row last-writer-wins is per-cookie LWW. Satisfies
-- `replica::attach`: single-column primary key, nullable data columns, `hlc TEXT` (stamped at
-- capture).
--
-- `id`: first 32 hex of HMAC-SHA256(id key, JSON ["cookie", site, name, domain, path]) or
-- ["storage", site, origin]; `site_id`: the same of ["site", site]. The id key is
-- HMAC-SHA256(current mesh key, "theuth/webprofile/id/v1"), so every engine of the mesh computes
-- the same ids and the file at rest does not name the sites. `sealed`: base64url(nonce ‖
-- XChaCha20-Poly1305(mesh key, aad = "web_sites\n" ‖ id, entry JSON)) — the entry carries the
-- site, the profile epoch, the writer, the write time and the cookie or snapshot, so a row is one
-- atomic value; NULL = removed (a tombstone, kept so a removal replicates and wins over an older
-- set; deleted 30 days after its write). The row `id = 'epoch'` (no `site_id`) seals the epoch.
CREATE TABLE web_sites (
    id       TEXT PRIMARY KEY,
    site_id  TEXT,
    sealed   TEXT,
    hlc      TEXT
);

CREATE INDEX web_sites_site ON web_sites (site_id);

-- Local, not replicated: live rows and sealed bytes per site, kept by the triggers below on every
-- write (own and replicated), so the profile caps never re-read the sealed rows.
CREATE TABLE web_site_stats (
    site_id       TEXT PRIMARY KEY,
    n             INTEGER NOT NULL,
    sealed_bytes  INTEGER NOT NULL
);

CREATE TRIGGER web_sites_stats_insert AFTER INSERT ON web_sites
WHEN NEW.site_id IS NOT NULL AND NEW.sealed IS NOT NULL
BEGIN
    INSERT INTO web_site_stats (site_id, n, sealed_bytes) VALUES (NEW.site_id, 1, length(NEW.sealed))
    ON CONFLICT (site_id) DO UPDATE SET n = n + 1, sealed_bytes = sealed_bytes + excluded.sealed_bytes;
END;

CREATE TRIGGER web_sites_stats_delete AFTER DELETE ON web_sites
WHEN OLD.site_id IS NOT NULL AND OLD.sealed IS NOT NULL
BEGIN
    UPDATE web_site_stats SET n = n - 1, sealed_bytes = sealed_bytes - length(OLD.sealed)
    WHERE site_id = OLD.site_id;
END;

CREATE TRIGGER web_sites_stats_update AFTER UPDATE OF site_id, sealed ON web_sites
BEGIN
    UPDATE web_site_stats SET n = n - 1, sealed_bytes = sealed_bytes - length(OLD.sealed)
    WHERE OLD.site_id IS NOT NULL AND OLD.sealed IS NOT NULL AND site_id = OLD.site_id;
    INSERT INTO web_site_stats (site_id, n, sealed_bytes)
    SELECT NEW.site_id, 1, length(NEW.sealed)
    WHERE NEW.site_id IS NOT NULL AND NEW.sealed IS NOT NULL
    ON CONFLICT (site_id) DO UPDATE SET n = n + 1, sealed_bytes = sealed_bytes + excluded.sealed_bytes;
END;
