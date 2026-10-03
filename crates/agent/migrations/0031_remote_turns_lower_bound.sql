-- 06 §8.4: whether a `remote_turns` row's tokens are the harness's last
-- request only (opencode before 2.0, upstream #1860) rather than the turn's
-- sum. Decided per child from `initialize.agentInfo` at record time; rows
-- written before this column all came from `opencode acp` 1.x or omp.
ALTER TABLE remote_turns ADD COLUMN lower_bound INTEGER NOT NULL DEFAULT 0;
UPDATE remote_turns SET lower_bound = 1 WHERE harness = 'opencode';
