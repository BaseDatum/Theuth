-- 210 FIX-A (M8): what the owner approved for a share that is not live yet, local and never
-- replicated (only the publishing engine promotes its own pending share).
CREATE TABLE share_requests (share TEXT PRIMARY KEY, expiry_ms INTEGER NOT NULL);
