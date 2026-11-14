-- Typed peer provenance beside a user row (the `channel` pattern, 0049_channel.sql):
-- JSON `PeerOrigin` (id, from, reply_to, scope, contact_mode) written in the transaction
-- that consumed the exchange into history, so it rides session sync with the row. The
-- receiving engine still reads its own `peer_exchanges` record first; every other mesh
-- node renders the row from this. NULL for every other message. The model never sees it.
-- A replicated column: SESSIONS_WIRE_SCHEMA = 65.
ALTER TABLE messages ADD COLUMN peer TEXT NULL;
