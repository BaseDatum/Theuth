-- Design doc 51 §3 as-built: per-project icon and colour, shown left of the
-- project name in every client. Rows exist only for styled projects; the
-- label is the `sessions.project` value. `icon` is a lucide icon name and
-- `color` a palette key (both allow-listed in `project.rs`), never hex.
CREATE TABLE projects (
    name TEXT PRIMARY KEY,
    icon TEXT NULL,
    color TEXT NULL,
    updated_at TEXT NOT NULL
);
