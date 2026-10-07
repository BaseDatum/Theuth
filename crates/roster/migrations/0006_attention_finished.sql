-- 130 E3: a root / desk session's last clean turn is an attention item
-- (`finished:<session>:<turn>`), the receipt the companion's Done zone shows.
-- One row per session: the next turn's replaces it. Engine-local like
-- `attention_shows`; dismiss / snooze live in `attention_state`.

CREATE TABLE attention_finished (
    session     TEXT PRIMARY KEY,
    turn        TEXT NOT NULL,
    at_ms       INTEGER NOT NULL,
    summary     TEXT,                -- ≤ 160 chars, NULL when the turn had no text and no title
    next_steps  TEXT NOT NULL        -- JSON array of ≤ 2 strings, each ≤ 80 chars
);
