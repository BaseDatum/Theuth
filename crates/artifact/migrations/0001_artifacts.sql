CREATE TABLE artifacts (
  call_id TEXT PRIMARY KEY, id TEXT NOT NULL, session TEXT NOT NULL,
  name TEXT NOT NULL, title TEXT NOT NULL, kind TEXT NOT NULL, mime TEXT NOT NULL, bytes INTEGER NOT NULL,
  version INTEGER NOT NULL, source TEXT, preview TEXT, pages INTEGER, origin TEXT, created_at INTEGER NOT NULL
);
CREATE INDEX artifacts_session ON artifacts(session, created_at);
