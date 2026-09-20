-- Transcript segments for `meeting_search` (04 §5): re-indexed whole per meeting
-- on every edit; rebuilt from `transcript.json` files with the rest of the index.
CREATE VIRTUAL TABLE segments USING fts5(
  meeting UNINDEXED,
  seg UNINDEXED,
  t0 UNINDEXED,
  t1 UNINDEXED,
  speaker,
  text
);
