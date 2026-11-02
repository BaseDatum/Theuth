-- 160 R2: `cost_calls` pages for a filter scan the matching rows newest first
-- and stop at the page size, instead of walking the whole time index for a
-- rare provider / model / kind (openai: 1.6 s → < 1 ms on Sky's ledger). The
-- page picks one of these from the cost cube's counts (`CallDriver`).
CREATE INDEX model_calls_provider_created ON model_calls(provider, created_at);
CREATE INDEX model_calls_model_created ON model_calls(requested_model, created_at);
CREATE INDEX model_calls_kind_created ON model_calls(kind, created_at);
-- Basis filters: rows with no money (unpriced, plus the repriced ones) and rows
-- with a provider-returned price (none yet on Sky's ledger).
CREATE INDEX model_calls_unpriced_created ON model_calls(created_at)
  WHERE returned_nanos IS NULL AND estimated_nanos IS NULL;
CREATE INDEX model_calls_known_created ON model_calls(created_at)
  WHERE returned_nanos IS NOT NULL;
