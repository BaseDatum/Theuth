-- 210 §6.3: durable revocations. A chain leaves `names.chain_pem` and lands here in the same transaction that
-- releases, bans, supersedes or orphans it, so a failed or interrupted `revoke` is retried with backoff
-- (FIX-EDGE M6) instead of being lost.
CREATE TABLE revocations (id INTEGER PRIMARY KEY AUTOINCREMENT, domain TEXT NOT NULL, name TEXT NOT NULL,
  chain_pem TEXT NOT NULL,
  reason TEXT NOT NULL CHECK (reason IN ('cessation_of_operation','superseded','key_compromise')),
  state TEXT NOT NULL DEFAULT 'queued' CHECK (state IN ('queued','done','failed')),
  attempts INTEGER NOT NULL DEFAULT 0, next_at_ms INTEGER NOT NULL, last_error TEXT,
  created_ms INTEGER NOT NULL);
CREATE INDEX revocations_due ON revocations (state, next_at_ms);
