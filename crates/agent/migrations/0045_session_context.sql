-- 144 §5.1 (W-g): the context dial per thread. Owner-only, never captured (not in `capture::ATTACHED`).
CREATE TABLE session_context (session TEXT NOT NULL, item TEXT NOT NULL, kind TEXT NOT NULL, level TEXT NOT NULL, hlc INTEGER NOT NULL, PRIMARY KEY (session, item));
