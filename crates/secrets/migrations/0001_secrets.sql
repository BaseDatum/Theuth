-- The vault as replicated rows (67 §5.1, §5.4). `vault` is append-only per
-- (name, version); `vault_current` is the LWW pointer. `id` = `<name>#<version>`
-- because replication needs a single-column primary key. `meta` is local only.
CREATE TABLE vault (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    version INTEGER NOT NULL,
    ciphertext BLOB NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    source TEXT NOT NULL DEFAULT '',
    updated_ms INTEGER NOT NULL,
    hlc TEXT,
    UNIQUE(name, version)
);

CREATE TABLE vault_current (
    name TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    hlc TEXT
);

CREATE TABLE meta (
    k TEXT PRIMARY KEY,
    v BLOB NOT NULL
);
