-- UI-FIXES-3 (2026-09-26): the roster agent whose desk this session is
-- (67 §6.2). Set by the roster when it opens a desk; a durable mark, so the
-- chat lists drop desk sessions and the forwarder routes them without the
-- old `pinned && title starts with agent:` heuristic. NULL for every other session.
ALTER TABLE sessions ADD COLUMN desk TEXT;
