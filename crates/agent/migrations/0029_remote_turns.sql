-- 75 §7: one row per finished ACP turn whose `session/prompt` response carried
-- `usage` (`unstable_end_turn_token_usage`). Columns in `model_calls` terms:
-- `input_tokens` = fresh + cache read + cache write, `output_tokens` = output
-- + thought. Otherwise as the harness sent them — omp a per-turn delta,
-- opencode the last request only (upstream #1860); `harness` names the sender
-- so the semantics can be corrected later. `model` is `sessions.harness_model`
-- at turn end. Money stays in `remote_meters`; these rows carry none, so
-- nothing double-counts.
CREATE TABLE remote_turns (
  owner_id TEXT NOT NULL REFERENCES cost_owners(id) ON DELETE CASCADE,
  turn_id TEXT NOT NULL,
  harness TEXT NOT NULL,
  model TEXT NULL,
  input_tokens INTEGER NOT NULL CHECK(input_tokens >= 0),
  output_tokens INTEGER NOT NULL CHECK(output_tokens >= 0),
  cache_read_tokens INTEGER NOT NULL CHECK(cache_read_tokens >= 0),
  cache_write_tokens INTEGER NOT NULL CHECK(cache_write_tokens >= 0),
  at TEXT NOT NULL,
  PRIMARY KEY (owner_id, turn_id)
);
