-- 153 §6.5: dictation history, corrections and suggestion state.
CREATE TABLE dictations (
  id TEXT PRIMARY KEY,            -- DictationId (uuid v7)
  started_ms INTEGER NOT NULL, finished_ms INTEGER,
  surface TEXT NOT NULL,          -- composer | tui | companion | phone | any_app
  destination TEXT NOT NULL,      -- agent_prompt | document | message_reply | any_app:<bundle or exe>
  app TEXT,                       -- any-app: bundle id / exe; else NULL
  raw TEXT NOT NULL, shaped TEXT NOT NULL, polished TEXT,   -- finals joined; polished NULL unless §9 ran
  delivered TEXT,                 -- what actually went out (polished ?? shaped), after end-command edits
  press_ms INTEGER, first_partial_ms INTEGER, first_final_ms INTEGER, final_ms INTEGER,  -- §8.7 (relative to press)
  reason TEXT NOT NULL,           -- done | stopped | error:<message>
  audio_bytes INTEGER NOT NULL DEFAULT 0, audio_path TEXT,  -- <data_dir>/voice/history/<id>.pcm16, NULL when evicted
  -- A3 addition: cumulative byte end offset of each utterance in the audio file, comma-separated
  -- ("3200,9600" = utterance 0 is bytes [0,3200), utterance 1 is [3200,9600)).
  utterance_ends TEXT NOT NULL DEFAULT ''
);
CREATE TABLE corrections (
  id INTEGER PRIMARY KEY, dictation_id TEXT NOT NULL REFERENCES dictations(id) ON DELETE CASCADE,
  utterance INTEGER NOT NULL, heard TEXT NOT NULL, typed TEXT NOT NULL, at_ms INTEGER NOT NULL
);
CREATE TABLE suggestion_state (          -- cooldowns and nevers, keyed by the pair
  heard_lc TEXT NOT NULL, typed TEXT NOT NULL, state TEXT NOT NULL,   -- cooldown | never | added
  until_ms INTEGER, PRIMARY KEY (heard_lc, typed)
);
CREATE INDEX dictations_started ON dictations(started_ms);
