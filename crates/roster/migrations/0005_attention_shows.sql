-- 104 A1: every artifact an agent showed since this build is an attention
-- item (`show:<call_id>`). The actor records the `ArtifactShown` event here,
-- engine-local like `attention_state`; a re-show of the same call_id replaces
-- the row (newer version wins). Dismiss / snooze live in `attention_state`.

CREATE TABLE attention_shows (
    call_id  TEXT PRIMARY KEY,   -- artifact::Artifact.call_id
    session  TEXT NOT NULL,
    title    TEXT NOT NULL,
    name     TEXT NOT NULL,
    kind     TEXT NOT NULL,      -- artifact::Kind::as_str
    mime     TEXT NOT NULL,
    bytes    INTEGER NOT NULL,
    version  INTEGER NOT NULL,
    at_ms    INTEGER NOT NULL
);
