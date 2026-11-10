-- 186 review 1: an agent inbox shared in full keeps the exported agent's own `tools:` ceiling
-- across a restart (the persona is not in memory at an actor start); NULL = no ceiling.
ALTER TABLE a2a_contexts ADD COLUMN inbox_ceiling TEXT
    CHECK (inbox_ceiling IN ('read_only', 'mutating', 'destructive'));
