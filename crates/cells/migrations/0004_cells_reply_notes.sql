-- 171F B9: the note a chat reply was opened as ("Open as note" reopens it instead of a copy).
CREATE TABLE IF NOT EXISTS reply_notes(
  session    TEXT NOT NULL,
  entry      TEXT NOT NULL,
  doc        TEXT NOT NULL,
  created_ms INTEGER NOT NULL,
  PRIMARY KEY(session, entry)
);
