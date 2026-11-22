-- 209 §4.1 (P2 review fixes): local tables, never replicated.
--
-- `web_profile_local`: small local facts. `epoch_seen` is the highest profile epoch this engine
-- has seen (its own clear, or a replicated epoch row): the replicated epoch row is last-writer-wins
-- on its column, so two engines clearing apart could leave a lower number behind; the store answers
-- the highest seen, writes `seen + 1` on a clear and rewrites a lower replicated row upward.
CREATE TABLE web_profile_local (
    key    TEXT PRIMARY KEY,
    value  INTEGER NOT NULL
);

-- `web_sites_removed`: when each tombstone was written here (own or replicated), in Unix ms, so
-- tombstones are purged 30 days later also while no replica stamps `hlc` (a local-only profile).
CREATE TABLE web_sites_removed (
    id          TEXT PRIMARY KEY,
    removed_ms  INTEGER NOT NULL
);

INSERT INTO web_sites_removed (id, removed_ms)
SELECT id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
FROM web_sites WHERE sealed IS NULL AND id != 'epoch';

CREATE TRIGGER web_sites_removed_insert AFTER INSERT ON web_sites
WHEN NEW.sealed IS NULL
BEGIN
    INSERT INTO web_sites_removed (id, removed_ms)
    VALUES (NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER))
    ON CONFLICT (id) DO UPDATE SET removed_ms = excluded.removed_ms;
END;

CREATE TRIGGER web_sites_removed_update AFTER UPDATE OF sealed ON web_sites
BEGIN
    DELETE FROM web_sites_removed WHERE id = NEW.id AND NEW.sealed IS NOT NULL;
    INSERT INTO web_sites_removed (id, removed_ms)
    SELECT NEW.id, CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
    WHERE NEW.sealed IS NULL
    ON CONFLICT (id) DO UPDATE SET removed_ms = excluded.removed_ms;
END;

CREATE TRIGGER web_sites_removed_delete AFTER DELETE ON web_sites
BEGIN
    DELETE FROM web_sites_removed WHERE id = OLD.id;
END;
