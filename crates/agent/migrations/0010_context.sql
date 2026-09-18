-- Design doc 48: explicit removal from context.
-- `context_revision` is the mutation handle for context edits: bumped on
-- every append, epoch, exclusion edit, undo/redo and committed cut.
ALTER TABLE sessions ADD COLUMN context_revision INTEGER NOT NULL DEFAULT 0;
-- Exclusion masks (JSON `{"whole":bool,"binaries":[{"part":n,"image":n|null}]}`)
-- beside the untouched original rows; NULL means nothing excluded.
ALTER TABLE messages ADD COLUMN excluded TEXT NULL;
ALTER TABLE epochs ADD COLUMN excluded TEXT NULL;
