-- 186 review 1, 2: an agent inbox shared in full runs under the exported agent's own `tools:`
-- ceiling, re-read from the roster at every admission and actor start (the stricter of the
-- stored one and the live one wins). `inbox_agent`: the exported agent (bare roster name);
-- `inbox_ceiling`: the ceiling of its last real persona, `'none'` when it has none; NULL with
-- `inbox_full` set is unknown and runs capped.
ALTER TABLE a2a_contexts ADD COLUMN inbox_agent TEXT;
ALTER TABLE a2a_contexts ADD COLUMN inbox_ceiling TEXT
    CHECK (inbox_ceiling IN ('none', 'read_only', 'mutating', 'destructive'));
