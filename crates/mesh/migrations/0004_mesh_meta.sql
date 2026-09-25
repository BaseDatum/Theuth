-- 69 decision 8: mesh-wide key/value record (`name`, …). Replicates with the
-- other mesh tables (single-column pk, `hlc TEXT`); one row per key, LWW.
CREATE TABLE mesh_meta (
    key   TEXT PRIMARY KEY,
    value TEXT,
    hlc   TEXT
);
