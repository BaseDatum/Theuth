-- Knowledge harvester per session: a chat's own pick for the knowledge harvest over
-- `agent.knowledge_harvester_model` (then the session's model). A side table like
-- `session_advisors`; engine-local: not in store::capture::ATTACHED, so SESSIONS_WIRE_SCHEMA
-- stays. No row = the default.
CREATE TABLE session_knowledge_harvesters (
  session TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
  model   TEXT NOT NULL,
  effort  TEXT NULL
);
