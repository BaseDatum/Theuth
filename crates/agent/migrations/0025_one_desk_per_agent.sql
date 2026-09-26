-- STALE-WAKE-DESKS (2026-09-26): one desk per agent per engine. A restart used
-- to open a fresh session for every agent, so `desk = <agent>` rows piled up
-- and the roster, the wake claims and the sidebar each pointed at a different
-- one. The roster now reuses the marked row (`agent::DeskSession`); this makes
-- the invariant hold for existing data and the schema.
--
-- Dedupe first: per (desk, owner) the most recently updated row stays the desk
-- (an unowned row, `owner = ''`, counts as this engine's: `Store::open`
-- backfills it right after the migrations); every older duplicate keeps its
-- transcript but loses the mark, is archived and unpinned, and is titled
-- `<agent> · desk (superseded <date>)` so it is still readable from the
-- archived chat list.
UPDATE sessions
SET title    = desk || ' · desk (superseded ' || strftime('%Y-%m-%d', 'now') || ')',
    desk     = NULL,
    pinned   = 0,
    archived = 1
WHERE desk IS NOT NULL
  AND EXISTS (
    SELECT 1 FROM sessions o
    WHERE o.desk = sessions.desk
      AND (o.owner = sessions.owner OR o.owner = '' OR sessions.owner = '')
      AND o.id <> sessions.id
      AND (o.updated_at > sessions.updated_at
           OR (o.updated_at = sessions.updated_at AND o.id > sessions.id))
  );

-- Per engine (`owner`), not global: session rows replicate between engines
-- (68), and each hub opens its own desk for an agent when the lease moves.
CREATE UNIQUE INDEX sessions_desk_owner ON sessions (desk, owner) WHERE desk IS NOT NULL;
