-- 210 FIX4 #1: the owner's approval of a share as this engine's consent path recorded it, local
-- and never replicated. A replicated `shares` row naming this engine mounts only with a matching
-- row here (same approval id and the same host, path, source and access the owner approved); a row
-- that arrived or changed by replication alone stays unmounted.
CREATE TABLE share_approvals (
    share TEXT PRIMARY KEY,
    approval_id TEXT NOT NULL,
    binding TEXT NOT NULL,
    approved_ms INTEGER NOT NULL
);
