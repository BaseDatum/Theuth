-- 99 WRITE-CAP-LINE: the cross-task write cap counts writes within a rolling
-- window (`tasks.cross_task_write_window_secs`, default 24 h) instead of for
-- the session's life, so a long-lived desk is not locked out forever by old
-- writes. `at_ms` is when the (session, task) row was first charged; existing
-- rows get 0 and age out on the first charge under a window.
ALTER TABLE write_budget ADD COLUMN at_ms INTEGER NOT NULL DEFAULT 0;
