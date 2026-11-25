-- Per-chat always-ask tools (219 §3.2, `set_always_ask`): a JSON array of tool names and
-- `mcp:<server>` keys that always raise a human-answered approval card in this chat and its
-- subagents, NULL when none. A session-log column (SESSIONS_WIRE_SCHEMA 77), so a moved chat
-- keeps its list; the supervisor restores it when the chat's actor starts.
ALTER TABLE sessions ADD COLUMN always_ask TEXT NULL;
