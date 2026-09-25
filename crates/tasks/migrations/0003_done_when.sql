-- 70 DESK-PROTOCOL: the acceptance criteria a `done` is judged against first
-- (then the user's evident intention, then follow-up tasks filed). Replicated
-- with the row.
ALTER TABLE tasks ADD COLUMN done_when TEXT;
