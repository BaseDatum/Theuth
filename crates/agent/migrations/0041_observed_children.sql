-- 141 ACP SUBAGENT VISIBILITY: watch-only children a harness runs inside
-- itself. Rows have kind = 'observed', parent = the ACP child (or an observed
-- parent when nested), NO agent (nothing here drives them), and
-- acp_session = the harness's own id for the child.
ALTER TABLE sessions ADD COLUMN observed_source TEXT NULL;        -- rfd | opencode_ext | opencode_task | omp_task
ALTER TABLE sessions ADD COLUMN observed_state TEXT NULL;         -- pending | running | requires_action | idle | completed | failed | cancelled | unknown
ALTER TABLE sessions ADD COLUMN observed_stop TEXT NULL;          -- stop reason / error text
ALTER TABLE sessions ADD COLUMN observed_cancel INTEGER NOT NULL DEFAULT 0;   -- the harness accepts session/cancel for this child
ALTER TABLE sessions ADD COLUMN observed_description TEXT NULL;
ALTER TABLE sessions ADD COLUMN observed_tool_call TEXT NULL;     -- the spawning tool call id (roster card), when known
ALTER TABLE sessions ADD COLUMN observed_detail TEXT NULL;        -- JSON: source-specific live fields (141 §6)
ALTER TABLE sessions ADD COLUMN observed_cursor INTEGER NULL;     -- omp: byte offset read so far in the child's transcript
ALTER TABLE sessions ADD COLUMN observed_cost_rule TEXT NULL;     -- counted | inside_parent | display_only | unreported (141 §7.6); NULL = the source's default
-- descendants_incl and the observed lookups filter on parent; nothing indexed it before.
CREATE INDEX IF NOT EXISTS sessions_parent ON sessions(parent);
-- Identity of a watch-only child: one row per (parent, source, external id).
CREATE UNIQUE INDEX IF NOT EXISTS sessions_observed_identity
    ON sessions(parent, observed_source, acp_session)
    WHERE observed_source IS NOT NULL;
