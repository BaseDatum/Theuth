-- 210 M2 (§5.7, §6.3): the rekey history `GET /v1/mesh` reports, and whether the pending rotation of a name
-- replaces a key a removed engine held (the old certificate is then revoked `keyCompromise`, else `superseded`).
-- `old` is the primary key: a mesh id is rekeyed away at most once (first valid rekey wins).
CREATE TABLE mesh_rekeys (old TEXT PRIMARY KEY, new TEXT NOT NULL, ms INTEGER NOT NULL);
CREATE INDEX mesh_rekeys_new ON mesh_rekeys(new);
ALTER TABLE names ADD COLUMN rotate_compromised INTEGER NOT NULL DEFAULT 0;
