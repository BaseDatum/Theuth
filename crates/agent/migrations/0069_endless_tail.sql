-- 201 §4.2 / §12.1: the per-session tail override, beside `endless_view_bytes`.
-- NULL mode = inherit (the agent's `tail_mode:`, else `agent.endless.tail_mode`).
-- `low`/`high` are in the mode's unit: bytes for `fixed`, shares for `scaled`.
-- `sessions` is replicated by the session log: SESSIONS_WIRE_SCHEMA becomes 69.
ALTER TABLE sessions ADD COLUMN endless_tail_mode TEXT NULL;
ALTER TABLE sessions ADD COLUMN endless_tail_low REAL NULL;
ALTER TABLE sessions ADD COLUMN endless_tail_high REAL NULL;
