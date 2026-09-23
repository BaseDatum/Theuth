-- Design doc 14 as-built (2026-09-23): pending `remind_me_in` timers, so a
-- restart re-arms them (overdue ones fire at once as late). Inserted when
-- set, deleted when fired or cancelled; times are unix seconds. Ids are
-- `r<n>` per session, hence the composite key.
CREATE TABLE reminders (
    id TEXT NOT NULL,
    session TEXT NOT NULL,
    message TEXT NOT NULL,
    set_at INTEGER NOT NULL,
    fires_at INTEGER NOT NULL,
    seconds INTEGER NOT NULL,
    PRIMARY KEY (session, id)
);
