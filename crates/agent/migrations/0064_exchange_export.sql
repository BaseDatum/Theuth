-- Contact return route: the contact's export an outbound message addressed (`""`: the
-- contact's first), kept when it left while the contact's card was unknown, so the
-- conversation the contact mints for it is recorded (or adopted) on that export.
-- `peer_exchanges` is local (not on the session log): SESSIONS_WIRE_SCHEMA stays.
ALTER TABLE peer_exchanges ADD COLUMN export TEXT NULL;
