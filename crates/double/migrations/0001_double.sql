-- 126 §5: `<data_dir>/double/double.sqlite`. Engine-local, never replicated
-- (training bundles ship explicitly, §8.4). Every string leaf is scrubbed with
-- `tools::secrets::redact_value` before it is bound. Times are RFC 3339 text.

-- One row per discovered class key (§3); created at L0 by the first card.
CREATE TABLE classes (
    key             TEXT    PRIMARY KEY,          -- kind:shape:requester
    kind            TEXT    NOT NULL,             -- approval | ask | attention | packet | draft
    rung            INTEGER NOT NULL DEFAULT 0,   -- L0..L3
    cap             INTEGER NOT NULL,             -- max rung for this class (§9 caps)
    mutating_opt_in INTEGER NOT NULL DEFAULT 0,
    n               INTEGER NOT NULL DEFAULT 0,   -- examples seen
    stats           TEXT    NOT NULL DEFAULT '{}',-- JSON: answer histogram, median_answer_ms, last_30d {n, acc, ece, overrides}
    updated_at      TEXT    NOT NULL
);

-- One card seen (§5): the scrubbed 4 KB state and its pooled embedding.
CREATE TABLE examples (
    id          TEXT PRIMARY KEY,                 -- uuid v7
    class       TEXT NOT NULL REFERENCES classes(key),
    question_id TEXT NOT NULL,                    -- ApprovalId / AskId / packet key / draft id / attention item id
    session_id  TEXT,
    requester   TEXT NOT NULL,                    -- root | subagent | desk:<agent> | routine:<name>
    card_text   TEXT NOT NULL,                    -- ≤ 4 KB, scrubbed
    options     TEXT NOT NULL,                    -- JSON labels in card order
    embedding   BLOB,                             -- f16 little-endian × hidden (2 KB at 1024)
    asked_at    TEXT NOT NULL
);
CREATE UNIQUE INDEX examples_question ON examples (question_id);
CREATE INDEX examples_class_asked ON examples (class, asked_at);

-- Free supervision (§6): one row per (example, source).
CREATE TABLE labels (
    example_id TEXT NOT NULL REFERENCES examples(id) ON DELETE CASCADE,
    source     TEXT NOT NULL,                     -- direct | hindsight_rewind | hindsight_correction | hindsight_interrupt | override | timeout | excluded
    choice     TEXT NOT NULL,
    weight     REAL NOT NULL,
    at         TEXT NOT NULL,
    PRIMARY KEY (example_id, source)
);

-- The prequential ledger (§5, §7.4): written before the owner sees the card;
-- `baseline` gets a row beside the active version for every example.
CREATE TABLE predictions (
    example_id   TEXT NOT NULL REFERENCES examples(id) ON DELETE CASCADE,
    version      TEXT NOT NULL,
    choice       TEXT NOT NULL,
    probs        TEXT NOT NULL,                   -- JSON [[label, p], …] incl. "none"
    p_top        REAL NOT NULL,
    rung_at_time INTEGER NOT NULL,
    acted        INTEGER NOT NULL DEFAULT 0,
    grace_until  TEXT,
    outcome      TEXT NOT NULL DEFAULT 'unlabelled', -- correct | wrong | unlabelled
    at           TEXT NOT NULL,
    PRIMARY KEY (example_id, version)
);
CREATE INDEX predictions_version_at ON predictions (version, at);

-- Trained artefact sets (§8.5); exactly one `active = 1`; `baseline` is zero-shot.
CREATE TABLE versions (
    id           TEXT PRIMARY KEY,
    tier         TEXT NOT NULL,                   -- a | b | c | baseline
    base_hash    TEXT NOT NULL,
    adapter_path TEXT,
    head_path    TEXT,
    knn_path     TEXT,
    metrics      TEXT NOT NULL DEFAULT '{}',      -- JSON
    data_range   TEXT NOT NULL,
    trained_at   TEXT NOT NULL,
    venue        TEXT NOT NULL,                   -- local | node:<name>
    duration_s   INTEGER NOT NULL DEFAULT 0,
    active       INTEGER NOT NULL DEFAULT 0
);

-- Training runs and their state-machine transitions (§8.1).
CREATE TABLE runs (
    id         TEXT PRIMARY KEY,
    tier       TEXT NOT NULL,
    venue      TEXT NOT NULL,
    state      TEXT NOT NULL,                     -- eligible | waiting | acquiring | running | paused | evaluating | promoted | rejected | cleaned
    started_at TEXT NOT NULL,
    ended_at   TEXT,
    result     TEXT,                              -- e.g. `rejected: golden`
    log_path   TEXT
);

-- The forgetting detector (§7.4): never trained on, re-scored by every candidate.
CREATE TABLE golden (
    example_id TEXT PRIMARY KEY REFERENCES examples(id) ON DELETE CASCADE
);

-- The zero-shot baseline every curve is a gap over (§0 item 3).
INSERT INTO versions (id, tier, base_hash, metrics, data_range, trained_at, venue, duration_s, active)
    VALUES ('baseline', 'baseline', '', '{}', '', '1970-01-01T00:00:00+00:00', 'local', 0, 0);
