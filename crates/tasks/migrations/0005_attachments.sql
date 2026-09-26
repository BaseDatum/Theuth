-- 70 TASK-ATTACHMENTS: references attached to the task (design 66 refs:
-- `[{kind: note|knowledge|artifact|session|meeting, id, title, meta}]`), the
-- same things a chat message can carry. Replicated with the row (the replica
-- triggers are rebuilt from table_info at attach). `external_json` stays what
-- it is — links belong in the brief.
ALTER TABLE tasks ADD COLUMN attachments_json TEXT NOT NULL DEFAULT '[]';
