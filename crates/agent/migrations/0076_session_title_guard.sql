-- Title guards per session (212 §11): `agent_titled` = the agent retitled itself with
-- `set_status`, `user_titled` = the user renamed the session; either stops the title model
-- for good. A side table like `session_knowledge_harvesters`; engine-local: not in
-- store::capture::ATTACHED, so SESSIONS_WIRE_SCHEMA stays. No row = both 0.
CREATE TABLE session_title_guard (
  session      TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
  agent_titled INTEGER NOT NULL DEFAULT 0,
  user_titled  INTEGER NOT NULL DEFAULT 0
);
