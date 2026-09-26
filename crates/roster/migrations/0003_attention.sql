-- 70 HUD: the attention inbox is derived (tasks, agents, approvals, peers);
-- only the user's dismiss / snooze per item id is stored, engine-local.

CREATE TABLE attention_state (
    id               TEXT PRIMARY KEY,   -- attention::AttentionItem.id
    dismissed_ms     INTEGER,
    snoozed_until_ms INTEGER
);
