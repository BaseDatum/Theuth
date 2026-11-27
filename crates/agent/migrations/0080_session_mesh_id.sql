-- Shape A multi-mesh mobile sessions: optional mesh identifier tagging an
-- on-device session (NULL for untagged / desktop sessions). Inherited from the
-- root like `project` and `notebook`.
ALTER TABLE sessions ADD COLUMN mesh_id TEXT;
