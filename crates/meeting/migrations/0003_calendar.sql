-- 158 §7.4 / §9: calendar sources and events, project memory, the pre-meeting brief cache,
-- and the meeting's project on the index. Derived data like the rest of this file: a wipe
-- plus a fetch/push (or a manifest rebuild) restores it. W2 and W3 write the queries.
ALTER TABLE meetings ADD COLUMN project TEXT;
CREATE TABLE cal_sources (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,                       -- 'eventkit' | 'ics'
  name TEXT NOT NULL,
  secret TEXT,                              -- vault name for ics; NULL for eventkit
  enabled INTEGER NOT NULL DEFAULT 1,
  etag TEXT,
  last_modified TEXT,
  last_fetch TEXT,
  last_error TEXT,
  state TEXT
);
CREATE TABLE cal_events (
  id TEXT PRIMARY KEY,                      -- source:uid:recurrence_id
  source TEXT NOT NULL REFERENCES cal_sources(id) ON DELETE CASCADE,
  uid TEXT NOT NULL,
  recurrence_id TEXT,
  sequence INTEGER NOT NULL DEFAULT 0,
  title TEXT NOT NULL,
  start TEXT NOT NULL,                      -- RFC 3339 UTC
  "end" TEXT NOT NULL,                      -- RFC 3339 UTC
  all_day INTEGER NOT NULL,
  tz TEXT,
  location TEXT,
  description TEXT,
  join_url TEXT,
  join_host TEXT,
  organizer_name TEXT,
  organizer_email TEXT,
  attendees TEXT NOT NULL,                  -- json [{name,email,status,role,is_me}]
  cancelled INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL
);
CREATE INDEX cal_events_window ON cal_events(start, "end");
-- §9.3: `uid:<uid>` / `title:<norm>` → project; §11.1: `tpl:uid:<uid>` / `tpl:title:<norm>` → template slug.
CREATE TABLE cal_project_memory (
  key TEXT PRIMARY KEY,
  project TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
-- §9.4: one generation per event instance; invalidated on sequence/description change or a new Done meeting.
CREATE TABLE cal_prebrief (
  event_id TEXT PRIMARY KEY,
  generated_at TEXT NOT NULL,
  bullets TEXT NOT NULL,                    -- json [string]
  brief TEXT,                               -- briefs.sqlite BriefId
  sources TEXT NOT NULL                     -- json [meeting id]
);
