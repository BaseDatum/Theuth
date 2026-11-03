-- 172: typed channel provenance beside a user row (the `vision` pattern,
-- 0014_vision.sql). JSON `ChannelOrigin` (via, chat, from, title) when Slack,
-- Signal or Telegram delivered the row; NULL for every other message, and for
-- every row stored before 172. The model never sees it.
ALTER TABLE messages ADD COLUMN channel TEXT NULL;
