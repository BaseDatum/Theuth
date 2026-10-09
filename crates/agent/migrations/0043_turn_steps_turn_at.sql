-- 146 D5 (RESTART-CLARITY): the boot query's per-turn `MAX(at)` was a
-- correlated scan per `turn_steps` row (~1.1 s over 25k rows at boot, 4×
-- per pass). The window rewrite in `store/durability.rs` (`NEWEST`) makes it
-- one pass; this index makes every remaining per-turn `at` lookup
-- (`open_turn`, `journal_prune`'s `fin`, `resume_report`) an index seek.
CREATE INDEX IF NOT EXISTS turn_steps_turn_at ON turn_steps(session_id, turn_id, at);
