-- 139 §10: two columns, no new table. Both run before the capture session
-- exists (store.rs: "DDL on an attached table would leave the session
-- unusable"), like 0033/0035 did.

-- 139 D14: which engine spawned the process; the reaper signals only its own.
-- Backfilled to the row's session owner at migration time; '' on a lone
-- engine matches `me = ''` the way `here` does.
ALTER TABLE jobs ADD COLUMN engine TEXT NOT NULL DEFAULT '';
UPDATE jobs SET engine = (SELECT owner FROM sessions s WHERE s.id = jobs.session_id);

-- 139 §6.3: why the turn was resumed, beside 109's kind.
-- crash | shutdown | update | transfer | takeover; '' = pre-139 row.
ALTER TABLE resume_reports ADD COLUMN cause TEXT NOT NULL DEFAULT '';

-- 139 §5.3 / §7.2: `pty_screens` joins `capture::ATTACHED` from this release
-- on (0033's "Engine-local" comment is superseded here, not edited — sqlx
-- checksums applied migrations). No DDL: the table's shape is unchanged.
