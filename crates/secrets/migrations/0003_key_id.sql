-- 183 §6.9 (D7): which mesh key sealed this row. '' = the legacy key
-- (rows written before 183, and rows replicated from a pre-183 engine).
-- The keyring itself (`mesh_key_info`, `mesh_keys`, `rotation`) lives in the
-- local-only `meta` table (§9.5), which needs no schema change.
ALTER TABLE vault ADD COLUMN key_id TEXT NOT NULL DEFAULT '';
CREATE INDEX vault_key_id ON vault(key_id);
