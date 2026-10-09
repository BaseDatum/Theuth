-- 146 D11 (RESTART-CLARITY): when a respawned ACP child comes back on a fresh
-- harness session (`session/load` failed), the id it could not load is kept so
-- the RESUME brief can name it. Written by `Store::set_acp_session` whenever
-- the recorded id changes.
ALTER TABLE sessions ADD COLUMN acp_previous TEXT NULL;
