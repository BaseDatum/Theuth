-- Design 223: a project is a first-class record. The `projects` row (until now icon and colour
-- only, 0018) also carries a description, a status (`active` | `archived`), when it was created
-- and string → string metadata (a JSON object). A row with `created_ms` NULL and nothing but a
-- style is a pre-223 style row. `projects` rides the record replica (per-column LWW), not the
-- session log, so SESSIONS_WIRE_SCHEMA stays (as 0068 did for `contacts.full_access`).
ALTER TABLE projects ADD COLUMN description TEXT NULL;
ALTER TABLE projects ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE projects ADD COLUMN created_ms INTEGER NULL;
ALTER TABLE projects ADD COLUMN metadata TEXT NULL;
