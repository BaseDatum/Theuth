-- Design doc 109 §8.2 (REPORT): the counts a resumed turn's step settled
-- with, so the banner survives a desktop that connects after the engine
-- resumed. One row per turn, upserted — a turn resumed twice keeps the
-- latest. Written in `loop_::report()` right before the live `ResumeReport`
-- event; read by `Loaded.resume`. Times are RFC 3339 text like `turn_steps`.
-- Rides `capture::ATTACHED` with the rest of the journal (§3.5).
CREATE TABLE resume_reports (
    session_id           TEXT    NOT NULL REFERENCES sessions(id),
    turn_id              TEXT    NOT NULL,
    kind                 TEXT    NOT NULL,              -- interrupted | suspended (ResumeKind)
    interrupted_at       TEXT    NOT NULL,              -- the plan's newest journal row time
    at                   TEXT    NOT NULL,              -- when the resumed step settled
    replayed             TEXT    NOT NULL DEFAULT '[]', -- JSON [[call_id, name], …]
    interrupted          TEXT    NOT NULL DEFAULT '[]', -- JSON [[call_id, name], …]
    lost_jobs            TEXT    NOT NULL DEFAULT '[]', -- JSON [job_id, …]
    children_restored    INTEGER NOT NULL DEFAULT 0,
    children_handed_off  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (session_id, turn_id)
);
