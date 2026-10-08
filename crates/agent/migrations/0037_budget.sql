-- 136 R-22: the session's money cap (`agent::Budget` JSON: nanos, currency,
-- since) so a durability-resumed turn is still metered after a restart.
-- NULL = no cap.
ALTER TABLE sessions ADD COLUMN budget TEXT NULL;

-- 136 R-21: `LedgerSince` / `budget_spent` window these tables by time.
CREATE INDEX IF NOT EXISTS messages_at ON messages(at);
CREATE INDEX IF NOT EXISTS model_calls_created ON model_calls(owner_id, created_at);
