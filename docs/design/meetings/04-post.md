# Meetings phase 4 — transcript of record, speakers, review, summary, follow-up

Status: phase design v0 · 2026-09-19 · implements 53 §0 decisions 6, 7 and
the after-meeting half of §3.3 (`meeting_read/search/speakers/list`) and
§3.4; fills 53's missing §6. Depends on phases 1 (Opus tracks, manifest,
index) and 2 (session, `live.jsonl`, `voice::stt::Engine` with word times);
its UI lands on phase 3's `/meetings/$id` route. Contract: `README.md` §3.

Goal of the phase: after stop, the user gets a transcript with named
speakers they can correct, a guided summary in the same session, knowledge
harvested, and a session that answers "what did Dana say about X" through
tools rather than from a pasted transcript.

## 0. Decisions local to this phase

1. **Models are files the user places, exactly like Parakeet today.**
   `<data_dir>/models/pyannote/{segmentation-3.0.onnx, wespeaker_en_voxceleb_CAM++.onnx}`
   — the exact files `pyannote-rs` 0.3.4 loads (its `BUILDING.md`), published
   ungated on its GitHub releases (`v0.1.0` assets), so 53 §2.2's "HF-gated,
   mirror on the release bucket" is moot. Override: `meeting.models_dir`.
   53 §2.2 also says "as 43 §0.3 does" — 43's mirror is unbuilt (`43-voice.md` §11,
   `voice::ModelState` has no `Downloading`; the only downloader is
   `update::download`). Missing files → the post pipeline runs *without*
   diarization: every system segment is `Speaker 1`, `PostProgress` skips
   `Diarizing`, the review pane says why. A downloader is one later change
   in one function (`post::models()`), for both crates at once.
2. **Alignment is "word midpoint inside a diarization span".** Words come
   from the re-transcription with `TimestampMode::Words`; spans from
   pyannote segmentation on the system track; a word joins the span that
   contains its midpoint, else the nearest span within 500 ms, else the
   previous word's span. Consecutive words with one speaker form a
   `Segment`. The mic track is one speaker (`Me`) — its words segment on
   sentence gaps > 1 s. Rejected: forced alignment (a third model for a
   correction pass the user makes anyway).
3. **Speaker identity = cosine to the profile's mean embedding; ≥ 0.6
   proposed, never auto-applied.** `Speaker N` labels are proposals with a
   score in the review pane; the user confirms/relabels/enrolls. Enrolling
   from a segment updates the profile's running mean (`n` kept). Rejected:
   auto-apply above a threshold (a wrong name on record is worse than a
   click).
4. **Review edits are one RPC with an op enum, applied and re-saved
   atomically.** `meeting_edit(MeetingId, Vec<EditOp>)` — `Relabel`,
   `Merge`, `Split`, `FixText`, `Enroll`. One method, one writer, one
   `transcript.json` rewrite (`.tmp` + rename). Rejected: five methods.
5. **Segment playback is a WAV over RPC.** `meeting_segment_audio(id, track,
   t0_ms, t1_ms) -> Vec<u8>` (base64 over JSON as `get_attachment`): decode
   the span from the Opus file (`voice::stt::decode`, made `pub`, on the
   Ogg with a seek to the page before `t0`), encode 16-bit PCM WAV. Spans
   are seconds long; the payload is small. Rejected: `asset://` URLs to the
   `.opus` files (WKWebView's Ogg/Opus playback is not reliable; and the
   mix carries both voices — review wants the one track).
6. **Summary is a normal turn; the agent gains `Compact.head`.** `Summarize`
   sends the guided form as a `SendInput` and waits for `TurnFinished`
   (the actor already tracks `AgentEvent`); the assistant's final text is
   `summary.md`; its first `#` line is the title (`RenameSession`). Then
   `Harvest { session }` (exists), then `Compact { session, style:
   Some(Summary), head: Some(summary) }` — a new optional `head` on
   `agent::Compact` (`lib.rs:245-250`) that installs the given text as the
   epoch head and skips the summariser call (`compact.rs::wrap_head` on the
   provided body). One field, no model call, no drift between what the user
   read and what the session remembers. If the agent change is unwanted,
   plain `Compact` is acceptable: the summary is the last message and the
   summariser will keep it.
7. **`meeting_search` is FTS5 in `meetings.sqlite`, not the unified `search`
   (49).** 53 names the tool; joining 49's `search` is a later doc.

## 1. Files

```
crates/meeting/src/post/mod.rs     run(meeting) task: stages, progress, failure handling
crates/meeting/src/post/transcribe.rs   Opus → PCM 16 kHz → Engine words (180 s pieces, 45 §0.8)
crates/meeting/src/post/diarize.rs      pyannote-rs segmentation + embeddings + clustering
crates/meeting/src/post/align.rs        words + spans → Vec<Segment> (pure; unit-tested)
crates/meeting/src/speakers.rs          profiles: load/save/match/enroll (pure math + files; unit-tested)
crates/meeting/src/transcript.rs        Transcript, Segment, Word, EditOp, apply (pure; unit-tested)
crates/meeting/src/summary.rs           SUMMARY_FORM const, Summarize flow
crates/meeting/src/tools.rs             + MeetingRead, MeetingSearch, MeetingSpeakers, MeetingList
crates/meeting/src/store.rs             + segments FTS table, index_transcript, search
crates/meeting/migrations/0002_segments.sql
crates/agent/src/{lib.rs,supervisor.rs,loop_.rs,compact.rs}   Compact.head (§0.6)
crates/rpc/src/api.rs, export.rs                              §5
crates/desktop/ui/src/components/meeting/{Review,Summary}.tsx  §6
```

`meeting/Cargo.toml` adds `pyannote-rs = "0.3"` (pin), `hound` (WAV) or a
20-line writer, `sqlx` already present.

## 2. Post pipeline (`post/mod.rs`)

Triggered by `StopMeeting` after the encoder joins: state `Post(Transcribing)`;
a tokio task per finished meeting (only one can finish at a time, but a
restart must resume: at plugin start, any manifest with `state: Post(_)` is
re-queued — the pipeline is idempotent from the Opus files).

1. **Transcribing** (0–60 %): one `Engine` (phase 2's; the two live engines
   were dropped at stop), each track decoded to 16 kHz, 180 s pieces with
   1 s overlap, `transcribe(piece, words: true)`; word times shifted by the
   piece offset; overlap words deduplicated by time. Two `Vec<Word>` (mic,
   system). Progress per piece.
2. **Diarizing** (60–90 %; skipped when models are missing, §0.1): system
   track only (decision 6). `pyannote_rs::segment(&pcm16k, 16_000,
   &segmentation_model)` → `Vec<{start, end}>`; `EmbeddingExtractor::new
   (&embedding_model)` → one embedding per span (spans < 1 s reuse the
   nearest longer one's label); greedy cosine clustering
   (`EmbeddingManager`-style: new cluster when best cosine < 0.75) →
   `Speaker 1..n` by first appearance; per cluster: mean embedding, match
   against `speakers/*.json` (§4) → `proposed: Option<(profile_id, score)>`.
   API names are 0.3.4's as read from the crate; verify against the source
   at implementation, not the README.
3. **Aligning** (90–100 %): `align::segments(mic_words, system_words,
   spans)` → `Vec<Segment>` sorted by `t0_ms` across both tracks;
   `transcript.json` written; `segments` FTS rows inserted; manifest
   `state: Review`; `ReviewReady` published.

Failure in any stage → manifest `state: Failed`, `failure: <stage: err>`,
files intact; `meeting_post_retry(MeetingId)` re-runs from stage 1.

Times: all `*_ms` on the recording clock. Pauses are gaps in the Ogg
timeline too (phase 1 stopped the encoder), so file time == recording time
and no gap arithmetic is needed here.

## 3. Transcript of record (`transcript.rs`)

```rust
pub struct Transcript {
  pub meeting: MeetingId,
  pub speakers: Vec<SpeakerLabel>,        // { id: String /* "me" | "s1".. */, name: String,
                                          //   profile: Option<Uuid>, proposed: Option<(Uuid, f32)> }
  pub segments: Vec<Segment>,             // sorted by t0_ms
  pub reviewed_at: Option<String>, pub summarized_at: Option<String>,
  pub diarized: bool,
}
pub struct Segment { pub id: u32, pub track: Track, pub speaker: String /* SpeakerLabel.id */,
  pub t0_ms: u64, pub t1_ms: u64, pub text: String, pub words: Vec<Word> }
pub struct Word { pub text: String, pub t0_ms: u64, pub t1_ms: u64 }

pub enum EditOp {
  Relabel { segments: Vec<u32>, speaker: String },           // existing label id
  Rename  { speaker: String, name: String },                 // "Speaker 2" → "John"
  Merge   { segments: Vec<u32> },                            // adjacent, same speaker after merge
  Split   { segment: u32, at_word: u32 },                    // word index
  FixText { segment: u32, text: String },                    // words keep times; text replaced
  Enroll  { speaker: String, name: String, entity: Option<String> },  // → profile (new or update)
  Reviewed,                                                  // sets reviewed_at
}
pub fn apply(t: &mut Transcript, ops: &[EditOp], speakers: &mut Profiles) -> Result<(), String>;
```

`meeting_read` renders `[mm:ss] Name: text` from this (or from `live.jsonl`
with `me`/`them` while recording), 400 lines per call, `from_ms/to_ms/
speaker` filters.

## 4. Speaker profiles (`speakers.rs`)

`<data_dir>/meetings/speakers/<uuid>.json`:

```rust
pub struct Profile { pub id: Uuid, pub name: String, pub entity: Option<String> /* knowledge id */,
  pub embedding: Vec<f32> /* mean */, pub samples: u32, pub created_at: String, pub updated_at: String }
```

`Profiles::load(dir)`, `match(&emb) -> Option<(Uuid, f32)>` (best cosine ≥
0.6), `enroll(name, entity, emb)` (new file or running mean into the named
profile), `delete(id)` = remove file. `Me` is never auto-enrolled (decision
6); `Enroll { speaker: "me", .. }` works like any other label (mic-track
embedding is computed on demand for that op only).

## 5. Actor, RPC, tools

Messages/RPC (all `ask` → `Result<_, String>`): `meeting_transcript(MeetingId)
→ Transcript`; `meeting_edit(MeetingId, Vec<EditOp>) → Transcript`;
`meeting_segment_audio(MeetingId, Track, u64, u64) → Vec<u8>`;
`meeting_speakers() → Vec<Profile>` (embedding omitted on the wire);
`meeting_speaker_delete(Uuid)`; `meeting_summarize(MeetingId) → ()` (state
`Summarizing`; `Summarized{title}` when done); `meeting_post_retry(MeetingId)`;
`meeting_summary(MeetingId) → String` (the markdown). `Status`/`Manifest`
already carry `state`. `export.rs`: `Transcript, Segment, Word, EditOp,
SpeakerLabel, Profile`.

`DeleteMeeting` (phase 1) additionally deletes `segments` rows. Profiles are
never deleted with a meeting.

Tools (`Risk::ReadOnly`, registered at plugin start — they read the store):

| tool | args | returns |
|---|---|---|
| `meeting_read` | `{ meeting?: MeetingId, from_ms?, to_ms?, speaker? }` | `[mm:ss] Name: text` lines, ≤ 400; `meeting` defaults to the meeting owning the calling session (`ToolCx.session` → index lookup), else the most recent |
| `meeting_search` | `{ query, meeting? }` | FTS5 hits: `meeting, title, [mm:ss] Name: snippet`, ≤ 40 |
| `meeting_speakers` | `{ meeting? }` | with `meeting`: labels with talk time (sum of segment durations); without: enrolled profiles (name, entity, samples) |
| `meeting_list` | `{ limit? }` | date, title, duration, session id, state |

`meetings.sqlite` gains
`CREATE VIRTUAL TABLE segments USING fts5(meeting UNINDEXED, seg UNINDEXED,
t0 UNINDEXED, t1 UNINDEXED, speaker, text)`; re-indexed on every
`meeting_edit` (delete by `meeting`, insert all — a meeting is hundreds of
rows) and rebuilt from `transcript.json` files when the db is rebuilt.

## 6. Summary (`summary.rs`)

`SUMMARY_FORM` (static; sent as one user input to the meeting session after
`Reviewed`):

> `[meeting summary]` The meeting has ended and the transcript was reviewed.
> Use `meeting_read` (all of it, in pieces) and `meeting_speakers`, then write
> the summary as markdown: a `#` title line (≤ 8 words), then **Topics**
> (what was discussed, in order, one line each), **Decisions** (with who),
> **Action items** (owner — item — due if said), **Open questions**, **Numbers
> and names said aloud**. Improvise sections if the meeting calls for it;
> keep it under 400 words.

Flow: `Summarizing` → `SendInput` → wait `TurnFinished` for the session →
read the last assistant message (`load_session`'s history; the actor uses
`agent`'s in-process message for it) → `summary.md` + manifest
`summarized_at` → `RenameSession { title }` → `Harvest { session }` (10 §5;
`"knowledge is off"` is logged, not fatal) → `Compact { session, style:
Some(Summary), head: Some(summary) }` (§0.6) → `Done`, `Summarized{title}`.
The first line of the follow-up conversation is therefore the summary; the
prompt block (phase 2) already tells the model to use the tools for the rest.

## 7. UI (`/meetings/$id`, phase 3's route)

- `state: Post(_)` → progress bar with stage names (`PostProgress`).
- `Review` → the review pane: speaker legend (label, name, proposed
  profile with score → *accept* = `Rename`+`Relabel` to the profile name,
  *enroll* dialog with name + optional knowledge entity picker), segment
  list (time, speaker chip, text; click time → play via
  `meeting_segment_audio` in an `<audio>` from a Blob; multi-select →
  relabel/merge; a word click → split; text click → inline edit → `FixText`),
  *Done reviewing* → `Reviewed` op then the **Summarize** button
  (`meeting_summarize`).
- `Summarizing` → spinner + a link to the live session.
- `Done` → summary markdown (`meeting_summary`), title, *Open chat*
  (`/c/$session`), the transcript below (read-only view of the same pane),
  *Re-run post pipeline* (`meeting_post_retry`) and *Delete*.
- Settings → Meetings tab (phase 3) gains the enrolled speakers list with
  delete.

## 8. Not in this phase

A model downloader (§0.1). Live diarization (decision 6). Export of
embeddings or transcripts (53 not-in-v1). Joining 49's unified search.

## 9. Check

1. `cargo test -p meeting`: `align::segments` (midpoint, nearest-within-500 ms,
   fallback-to-previous, mic sentence gaps, cross-track ordering);
   `transcript::apply` for every `EditOp` incl. rejects (non-adjacent
   merge, unknown label, split at 0/len); `speakers::match`/`enroll`
   (running mean, threshold); FTS index round trip on an in-memory db.
2. Headless: stop the phase-1/2 smoke meeting (two voices: mic + a played
   two-person clip) → `PostProgress` through the three stages →
   `ReviewReady`; `meeting_transcript` has `me` plus `Speaker 1`/`Speaker 2`
   segments whose texts match the clip; word times monotonic; a segment
   boundary falls at a speaker change in the clip within ~0.5 s.
3. Remove the pyannote models → re-run (`meeting_post_retry`): `diarized:
   false`, one `Speaker 1`, review pane explains.
4. `meeting_edit`: `Rename s1 → "Dana"`, `Enroll s2 "John"` → a profile
   file; stop and post-process a second meeting with the same clip →
   `proposed: (john-id, ≥ 0.6)` on the matching label; `Relabel`, `Merge`,
   `Split`, `FixText`, `Reviewed` each reflected in the returned
   `Transcript` and in `segments` (FTS finds the fixed text, not the old).
5. `meeting_segment_audio` for a 3 s span → a WAV that plays the right
   words.
6. `meeting_summarize` → `Summarized{title}`; `summary.md` has the
   sections; session title changed; `knowledge_harvest` added concepts
   (`knowledge_search` finds the ticket); the session has a new epoch whose
   head is the summary text (`load_session` shows it).
7. Follow-up in `/c/$session`: "what did Dana say about the image tags?" →
   the model calls `meeting_search`/`meeting_read`, answers with a time.
   From an unrelated session: `meeting_list` then `meeting_read{meeting}`
   works.
8. Restart the daemon during `Post(Transcribing)` → the pipeline resumes at
   start and reaches `ReviewReady`.
9. `meeting_delete` → `segments` rows gone, profiles kept.
10. Review UI: play, relabel via multi-select, split on a word, accept a
    proposal, enroll, Done → Summarize → summary rendered; speakers list in
    settings shows John; delete him → gone.

## 10. Status

Not started.
