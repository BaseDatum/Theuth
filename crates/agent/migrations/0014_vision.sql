-- Design doc 52 §6: typed vision-consultation metadata beside the note row.
-- JSON `VisionNote` (source references + fingerprints, question/vision model
-- ids, per-source outcome); NULL for every ordinary message. Question and
-- answer text live once in `body`. Only this column marks a row as a vision
-- note: quoted prose never does, and such rows never open a turn.
ALTER TABLE messages ADD COLUMN vision TEXT NULL;
