CREATE TABLE app_invocations (
  request_id TEXT PRIMARY KEY, session TEXT NOT NULL, record_call_id TEXT NOT NULL,
  statement_id TEXT NOT NULL, action_id TEXT, tool TEXT NOT NULL, args_hash TEXT NOT NULL,
  status TEXT NOT NULL, result TEXT, started_at INTEGER NOT NULL, finished_at INTEGER
);
CREATE INDEX app_invocations_session ON app_invocations(session);
