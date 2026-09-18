-- Design doc 50 §2.3, §7: root communication scope, durable peer exchanges,
-- A2A context/task bindings and remote peer identity bindings.

-- Root communication scope (`owner` | `isolated`) and the owner-selected peer
-- alias. Meaningful on roots (parent IS NULL); children inherit dynamically.
-- Existing roots of unknown provenance become `isolated`; forks start there too.
ALTER TABLE sessions ADD COLUMN scope TEXT NOT NULL DEFAULT 'isolated';
ALTER TABLE sessions ADD COLUMN alias TEXT NULL;

-- One accepted peer message. `accepted_seq` is the harness-wide acceptance
-- order; `id` is the stable message id peers see. Dedup key is the
-- authenticated sender plus its request id; `fingerprint` hashes
-- recipient/body/reply_to so a reused request id with changed content is a
-- conflict, not a retry. `history_seq` is the recipient's `messages.seq` link,
-- set exactly once at consumption. `session` is the owning local session
-- (recipient when local, else sender); rows die with it.
CREATE TABLE peer_exchanges (
    accepted_seq      INTEGER PRIMARY KEY AUTOINCREMENT,
    id                TEXT    NOT NULL UNIQUE,
    sender            TEXT    NOT NULL,
    recipient         TEXT    NOT NULL,
    sender_session    TEXT    NULL,
    recipient_session TEXT    NULL,
    session           TEXT    NOT NULL REFERENCES sessions(id),
    request_id        TEXT    NOT NULL,
    fingerprint       TEXT    NOT NULL,
    reply_to          TEXT    NULL,
    request_scope     TEXT    NULL,
    generation        INTEGER NOT NULL,
    body              TEXT    NOT NULL,
    state             TEXT    NOT NULL,
    reply             TEXT    NULL,
    task_id           TEXT    NULL,
    error             TEXT    NULL,
    history_seq       INTEGER NULL,
    created_at        TEXT    NOT NULL,
    updated_at        TEXT    NOT NULL,
    UNIQUE (sender, request_id)
);
CREATE INDEX peer_exchanges_pending ON peer_exchanges (recipient_session, state, accepted_seq);
CREATE INDEX peer_exchanges_sender ON peer_exchanges (sender_session, state);
CREATE INDEX peer_exchanges_scope ON peer_exchanges (request_scope);

-- Remote peer identity: immutable binding UUID separate from the editable
-- alias. Same alias + same endpoint keeps the id; a changed endpoint or a
-- recreated alias mints a new one. Revoked rows stay for accounting.
CREATE TABLE peer_bindings (
    id         TEXT PRIMARY KEY,
    alias      TEXT NOT NULL,
    endpoint   TEXT NOT NULL,
    created_at TEXT NOT NULL,
    revoked_at TEXT NULL
);
CREATE UNIQUE INDEX peer_bindings_live ON peer_bindings (alias) WHERE revoked_at IS NULL;

-- Opaque A2A conversation handle within (principal, export) → one session.
CREATE TABLE a2a_contexts (
    id         TEXT PRIMARY KEY,
    principal  TEXT NOT NULL,
    export     TEXT NOT NULL,
    session    TEXT NOT NULL REFERENCES sessions(id),
    created_at TEXT NOT NULL
);
CREATE INDEX a2a_contexts_scope ON a2a_contexts (principal, export);

-- One inbound A2A request's execution. `snapshot` is the SDK Task serialized
-- as JSON (no parallel wire schema); `status` mirrors its state for atomic
-- terminal checks; `revision` is the optimistic update handle; `request_scope`
-- is the local scope exchanges/turns bind to; `dedup_exchange` is the intake
-- row that created it.
CREATE TABLE a2a_tasks (
    seq              INTEGER PRIMARY KEY AUTOINCREMENT,
    id               TEXT    NOT NULL UNIQUE,
    context_id       TEXT    NOT NULL REFERENCES a2a_contexts(id),
    principal        TEXT    NOT NULL,
    export           TEXT    NOT NULL,
    session          TEXT    NOT NULL REFERENCES sessions(id),
    request_scope    TEXT    NOT NULL UNIQUE,
    status           TEXT    NOT NULL,
    revision         INTEGER NOT NULL DEFAULT 0,
    cancel_requested INTEGER NOT NULL DEFAULT 0,
    snapshot         TEXT    NOT NULL,
    error            TEXT    NULL,
    dedup_exchange   TEXT    NULL REFERENCES peer_exchanges(id),
    created_at       TEXT    NOT NULL,
    updated_at       TEXT    NOT NULL
);
CREATE INDEX a2a_tasks_scope ON a2a_tasks (principal, export, context_id, seq);
CREATE INDEX a2a_tasks_session ON a2a_tasks (session, status, seq);

-- Native work owned by a task scope (turns, delegated children, jobs); a task
-- completes only when every row is settled.
CREATE TABLE a2a_task_work (
    task_id TEXT    NOT NULL REFERENCES a2a_tasks(id),
    key     TEXT    NOT NULL,
    settled INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (task_id, key)
);
CREATE INDEX a2a_task_work_key ON a2a_task_work (key);
