-- Per-chat tool hides (`set_mcp_hidden`): a JSON array of registered tool names the chat's
-- owner hid for it, NULL when none. A session-log column (SESSIONS_WIRE_SCHEMA 61), so a
-- moved chat keeps its hides; the supervisor restores them when the chat's actor starts.
ALTER TABLE sessions ADD COLUMN hidden_tools TEXT NULL;
