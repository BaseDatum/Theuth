-- 126 §4.3: who settled a question — `human` | `double` (`tools::Answerer::as_str`);
-- NULL while pending or on a timeout. `answer` keeps its bare Decision / answers
-- JSON so 109's `HonourAnswer` replay is unchanged.
ALTER TABLE questions ADD COLUMN answered_by TEXT NULL;
