# Meetings phase 4 — transcript of record, speakers, review, summary, follow-up

Status: phase design v0 · 2026-09-19 · implements 53 §0 decisions 6, 7 and
the after-meeting half of §3.3 (`meeting_read/search/speakers/list`) and
§3.4; fills 53's missing §6. Depends on phases 1 (Opus tracks, manifest,
index) and 2 (session, `live.jsonl`); `voice::stt::Engine` (TDT with word
times) is **built in this phase** — 02 §0.1 deferred it here as the only
consumer. Its UI lands on phase 3's `/meetings/$id` route
(`ui/src/routes/meetings.$id.tsx` → `components/meeting/ControlCenter.tsx`).
Contract: `README.md` §3. Facts re-verified 2026-09-20 against the tree and
the `pyannote-rs` sources (upstream `main` = 0.3.4; not in the local registry).

> **Superseded in part (2026-09-24):** the diarization stage no longer runs
> pyannote segmentation-3.0 + agglomerative clustering. It runs NVIDIA's
> Nemotron 3 Diarization (Streaming Sortformer, ONNX) with one wespeaker
> embedding per speaker for the profile match; the models live under
> `<data_dir>/models/{nemotron-diarization,pyannote}/` and `meeting.models_dir`
> now names that root. Decision 1's file list, §1's `diarize.rs` line, §2's
> stage-2 text and deviations 1 and 3 below describe the replaced code; what
> runs is in `04-post-nemotron-as-built.md`. Decision 2 (alignment), 3
> (profiles) and deviation 2 (wespeaker tiling) stand.

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
   t0_ms, t1_ms) -> String` (base64, exactly as `get_attachment`,
   `crates/rpc/src/api.rs:2594-2618`): decode the track's Opus file with
   `voice::stt::decode` (today `pub(crate)`, `stt.rs:99`; the `voice` module
   `stt` is private — expose `Engine`, `decode`, `resample` through `pub use`
   in `voice/src/lib.rs`, keeping `parakeet_rs`/`symphonia` types inside).
   `decode` returns the Opus branch at **48 kHz** (libopus, `stt.rs:118-129`),
   whole file; slice `[t0, t1)` at 48 kHz, downmix nothing (mono), write
   16-bit PCM WAV with `hound` (3.5.1 already in the lock). No Ogg seek:
   a whole-file decode of a 3 h track is ~2 s and the review pane calls this
   on a click — `ponytail:` add page seeking if that ever registers. Spans
   are seconds long; the payload is small. Rejected: `asset://` URLs to the
   `.opus` files (WKWebView's Ogg/Opus playback is not reliable; and the
   mix carries both voices — review wants the one track).
6. **Summary is a normal turn; the agent gains `Compact.head`.** `Summarize`
   sends the guided form as a `SendInput` and waits for `TurnFinished`
   (the actor already handles `AgentEvent`, `lib.rs:1529-1568` — but only
   while `current` is `Some`; the summary wait needs its own pending slot on
   `Meeting`); the assistant's final text is `summary.md`; its first `#`
   line is the title (`RenameSession`). Then `agent::Harvest { session }`
   (`lib.rs:355-358`, `Err("turn in flight")` if raced — retry once after
   the next `TurnFinished`), then `Compact { session, style: Some(Summary),
   head: Some(summary) }` — a new optional `head` on `agent::Compact`
   (`lib.rs:245-250`, `Copy` today → becomes `Clone`) plumbed through
   `supervisor.rs:2030` → `CompactNow` (`loop_.rs:305/1510`) → `compact()`
   (`loop_.rs:3756`): with `head`, skip the summariser model call
   (~3778-3860) and pass the text as `body` to the existing `wrap_head`
   at ~3951 — the harness parts (roster, todos, files, …) are still
   assembled as usual. One field, no model call, no drift between what the
   user read and what the session remembers. If the agent change is
   unwanted, plain `Compact` is acceptable: the summary is the last message
   and the summariser will keep it.
7. **`meeting_search` is FTS5 in `meetings.sqlite`, not the unified `search`
   (49).** 53 names the tool; joining 49's `search` is a later doc.

## 1. Files

```
crates/voice/src/stt.rs            + pub struct Engine (02 §0.1's text: load(dir), transcribe(pcm16k, words) -> Piece); Transcriber.model becomes Option<Arc<Engine>>
crates/meeting/src/post/mod.rs     run(meeting) task: stages, progress, failure handling; resume at plugin start
crates/meeting/src/post/transcribe.rs   Opus → PCM 48 kHz (voice::stt::decode) → 16 kHz (voice::stt::resample) → Engine words (180 s pieces, 45 §0.8)
crates/meeting/src/post/diarize.rs      Nemotron 3 Diarization streaming loop → turns; one wespeaker embedding per speaker (04-post-nemotron-as-built)
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

`meeting/Cargo.toml` adds `pyannote-rs = { version = "=0.3.4", default-features = false,
features = ["load-dynamic"] }` and `hound = "3.5"`; `sqlx` already present.
`pyannote-rs` declares `ort = { version = "2.0.0-rc.10", features = ["ndarray"] }`
with ort's defaults on — cargo unifies it onto the workspace `=2.0.0-rc.13`,
and `ort-sys` skips download/linking whenever `load-dynamic` is enabled
anywhere (`ort-sys build/main.rs:28`); `wavekat-vad` (54) already pulls
`ort` the same way and the workspace builds, so this is not the blocker it
looks like. Its `create_session` does not call `ort::init`, so
`voice::stt::ort_lib` must run first (open the `Engine` before any
pyannote session — same order rule as 54's VAD). `knf-rs/sys` builds
kaldi-native-fbank with `cmake` (already required by `ptts`).

## 2. Post pipeline (`post/mod.rs`)

Triggered by `StopMeeting` at the point that today sets `Done`
(`crates/meeting/src/lib.rs:1369`, after tools are unregistered and the
manifest has `ended_at`/`duration_ms`): state `Post(Transcribing)`, saved,
`PostProgress{pct: 0}`; a tokio task per finished meeting (only one can
finish at a time, but a restart must resume: in `MeetingPlugin::start`
(`lib.rs:1831-1884`) any manifest with `state: Post(_)` is re-queued — the
pipeline is idempotent from the Opus files). The task runs off-mailbox and
reports back with `tell`s (as the STT tasks do); `current` is `None` after
stop, so post state lives in a `Meeting.post: Option<PostJob>` slot, not on
`Recording`.

1. **Transcribing** (0–60 %): one `voice::stt::Engine` (new here; the two
   live Nemotron sessions were dropped at stop), each track decoded
   (`decode` → 48 kHz, `resample` → 16 kHz), 180 s pieces with
   1 s overlap, `transcribe(piece, words: true)`; word times shifted by the
   piece offset; overlap words deduplicated by time. Two `Vec<Word>` (mic,
   system). Progress per piece.
2. **Diarizing** (60–90 %; skipped when models are missing, §0.1): system
   track only (decision 6). Real 0.3.4 API (`src/lib.rs` exports
   `get_segments, Segment, EmbeddingExtractor, EmbeddingManager, read_wav`):
   `pyannote_rs::get_segments(&pcm16k_i16, 16_000, &segmentation_model)
   -> Result<impl Iterator<Item = Result<Segment { start: f64, end: f64
   /* seconds */, samples: Vec<i16> }>>>` (speech/non-speech spans over 10 s
   windows; input is `i16`, so convert once); `EmbeddingExtractor::new
   (&embedding_model)` then `compute(&mut self, &segment.samples) ->
   Result<impl Iterator<Item = f32>>` → one embedding per span (spans < 1 s
   reuse the nearest longer one's label); greedy cosine clustering in our
   own ~15 lines (new cluster when best cosine < 0.75; `EmbeddingManager`'s
   `search_speaker(Vec<f32>, threshold)` does the same but caps speakers
   and hides its map — not used) → `Speaker 1..n` by first appearance; per
   cluster: mean embedding, match against `speakers/*.json` (§4) →
   `proposed: Option<(profile_id, score)>`.
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
`meeting_segment_audio(MeetingId, Track, u64, u64) → String /* base64 WAV */`;
`meeting_speakers() → Vec<Profile>` (embedding omitted on the wire);
`meeting_speaker_delete(Uuid)`; `meeting_summarize(MeetingId) → ()` (state
`Summarizing`; `Summarized{title}` when done); `meeting_post_retry(MeetingId)`;
`meeting_summary(MeetingId) → String` (the markdown). `Status`/`Manifest`
already carry `state`. `export.rs`: `Transcript, Segment, Word, EditOp,
SpeakerLabel, Profile`.

`DeleteMeeting` (phase 1) additionally deletes `segments` rows. Profiles are
never deleted with a meeting.

Tools (`Risk::ReadOnly`, **not** `DIRECT` — they return data, 08 §0.4;
registered once at plugin start inside the same `lookup_when::<ToolRegistry>`
block that registers the prompt, `lib.rs:1831-1884` — they read the store):

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
`agent`'s `LoadSession { session, limit: Some(8), .. }` → last
`HistoryEntry::Assistant`) → `summary.md` + `transcript.summarized_at`
(the manifest carries only `state`) → `RenameSession { title }` → `agent::Harvest { session }` (10 §5;
`"knowledge is off"` is logged, not fatal) → `Compact { session, style:
Some(Summary), head: Some(summary) }` (§0.6) → `Done`, `Summarized{title}`.
The first line of the follow-up conversation is therefore the summary; the
prompt block (phase 2) already tells the model to use the tools for the rest.

## 7. UI (`/meetings/$id`, phase 3's route)

- `state: Post(_)` → progress bar with stage names (`PostProgress`). The
  meeting store (`ui/src/store/meeting.ts`) does not yet handle
  `PostProgress`/`ReviewReady` — add them; and since `hydrateMeeting`
  (03 §9 deviation 10) rebuilds only the *live* meeting from
  `meeting_status`, the `/meetings/$id` page hydrates `Post/Review/
  Summarizing/Done` from `meeting_get` per id. Pause/stop live in
  `MeetingBar` (53 §4.8, formerly `MeetingPill`, 03 deviation 9); the phase-4 buttons go in `ControlCenter.tsx`.
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

Implemented 2026-09-20. `crates/meeting` +≈2 100 lines incl. tests
(`post/{mod,transcribe,diarize,align}.rs`, `transcript.rs`, `speakers.rs`,
`summary.rs`, `migrations/0002_segments.sql`, store/tools/actor additions);
`crates/voice/src/stt.rs` `Engine`/`Piece`/`Word` (+≈150); agent
`Compact.head` (+≈40, five callers updated); rpc 8 methods + 9 wire types;
desktop UI `components/meeting/{Review,Summary}.tsx`, store/rpc/api/settings
additions. Verified headless over JSON-RPC on `/tmp/theuth-meet` (the phase-2/3
scratch dir; `ORT_DYLIB_PATH` at the TDT checkpoint's dylib,
`THEUTH_SECRETS__KEY_STORE=file`, `models/` → `~/.theuth/models` with
`pyannote/{segmentation-3.0.onnx, wespeaker_en_voxceleb_CAM++.onnx}`),
macOS 15, `anthropic/claude-haiku-4-5`, the phase-2 knowledge bundle, a debug
build. The recordings used were phase-2/3 meetings of the two-voice `say`
clip played through speakers with the external mic live (so `mic.opus`
carries the far end, 01 §9 check 5).

### Exercised

1. `cargo test -p meeting` 28/28 (+14: `align::segments` — midpoint, nearest
   within 500 ms, previous-word fallback incl. no-span/no-previous, mic gaps at
   exactly 1 000 vs 1 100 ms, cross-track order and ids; `transcript::apply`
   — relabel/rename/fix/reviewed with the `lines()` filters, merge then
   split, ten rejects (non-adjacent, single, cross-track, unknown label,
   unknown segment, split at 0/len, empty text/name, enroll without an
   embedding), enroll links the label and folds a same-name profile;
   `speakers` threshold/running mean/reload/delete; `diarize::cluster`
   agglomerative order; `transcribe::starts` coverage; `summary::title_of`;
   FTS round trip — speaker and text indexed, hyphenated ticket as text, scope,
   re-index after an edit finds the new text only, delete removes rows, a
   rebuilt index picks `transcript.json` back up). `cargo test -p voice`
   13/13 incl. `engine_returns_words_with_monotonic_times` (the `say` smoke
   sentence → 8 words, monotonic non-overlapping times inside the clip;
   `words: false` → text only). `cargo test -p agent`: the vision contract
   test extended — `compact_now(Some(Summary), Some(head))` installs the given
   text as the epoch head with zero model requests. `cargo check --workspace
   --all-targets`, `cargo clippy -p meeting -p voice -p rpc -p theuth
   --all-targets --no-deps -D warnings`, `cargo check -p rpc --features ts`,
   `just ts-desktop` clean; `bun run check` clean, vitest 34 files / 185.
2. `meeting_post_retry` on the 150 s recording: `State{Post(Transcribing)}`,
   `PostProgress` 0/30/60 (one 180 s piece per track), `Diarizing` 60→90 per
   span, `Aligning` 90/100, `State{Review}`, `ReviewReady` — 23.9 s wall
   (debug build; the TDT load is ~0.8 s of it). Transcript: `me`, `s1`, `s2`
   and 9 segments alternating exactly as the clip does; every system segment
   boundary sits at a speaker change, off by at most one word (`slips.`,
   `object.` land on the neighbour). Word times monotonic. A second clip
   rendered for this phase with *known* turns (`say -v Samantha`/`Daniel`,
   10 turns, 62 s, 700 ms gaps) came back as 10 segments, all correctly
   labelled, through the same functions (a throwaway example, deleted).
3. Models moved away → `PostProgress` shows `Transcribing`, `Aligning` only;
   `diarized: false`, `note` names the directory and both files, one `s1`.
4. `meeting_edit`: `Rename s1 → Dana`, `Enroll s2 "John"` → one profile file;
   re-running the pipeline on the same recording proposed `~John 0.60`, after a
   second enroll (running mean, `samples: 3`) `0.88`. `Relabel`, `Merge`,
   `Split`, `FixText`, `Reviewed` each reflected in the returned transcript; the
   four rejects return the actor's text and leave the file untouched.
   `meeting_search("DR sync")` (through the tool, from an unrelated session)
   finds the fixed segment; `"Doctor Sync"` → `no matches`.
5. `meeting_segment_audio` for a 3 s span → RIFF, 48 kHz, mono, 16-bit,
   3.000 s (`ffprobe`); fed back through the `Engine` it reads *"Okay, let's
   get started. First item is"* — the right words. Empty span → `Err`.
6. `meeting_summarize` → `State{Summarizing}`, the model's turn (`python`
   cell, see not-exercised 2), `Summarized{title: "DR Cutover Readiness —
   October 3rd Blockers"}`, `summary.md` with Topics/Decisions/Action Items/
   an improvised Incident Context, `TitleChanged`, `Harvested` (the bundle
   gained `tickets/sre-2386` and `decisions/october-3-cutover-date`;
   `knowledge_search("SRE-2386")` ranks them first), `Compacted{epoch: 1,
   style: summary}`, `load_session` shows the compacted head = the summary
   text verbatim, `State{Done}`.
7. From an unrelated session, through the python runtime: `meeting_list`,
   `meeting_search`, `meeting_read{meeting, speaker: "Dana", to_ms}` all
   answered as specified. See not-exercised 2 for the meeting's own session.
8. `SIGKILL` 5 s into `Post(Transcribing)` on the 237 s meeting: manifest on
   disk says `Post(Transcribing)`; restart → the pipeline is re-queued at
   plugin start and reaches `Review` 29 s after boot.
9. `meeting_delete` on an indexed meeting: its `segments` rows gone
   (`sqlite3` on the index), directory, index row and session gone, `Deleted`
   published, the John profile kept.

### Not exercised

1. Check 10 (the review UI in the `.app`) and a fresh `meeting_stop` → `Post`
   hand-over with the user speaking: pending the user's run on a Developer
   ID build. The `.app` packaged on this machine (`scripts/package-macos.sh`,
   ad-hoc identity) starts a hardened-runtime sidecar whose library
   validation refuses the ad-hoc-signed `Contents/Frameworks/libonnxruntime.dylib`
   (`dlopen failed`; `python3 -c "ctypes.CDLL(...)"` loads the same file), so
   live STT (`SttDown` on both tracks) and the post pipeline
   (`Transcribing: load …libonnxruntime.dylib`) both fail in that bundle — a
   signing matter, not a code path: with a team identity the dylib and the
   sidecar share it and validation passes. The hook is the same `start_post`
   the retry path uses.
2. Check 7 *in the meeting's own session*: the model's `python` cell got
   `unresolved-reference: Name meeting_search used when not defined` (and the
   summary turn likewise could not call `meeting_read`; it summarised from the
   live batches still in its context — the summary reads "Based on the live
   transcript…"). Cause, found in `monty-proto` 0.0.23 `worker.rs
   handle_load`: restoring a runtime dump adopts the dump's
   `committed_stubs`, so `agent::repl::refresh_stubs` (which re-`Configure`s
   then re-`Load`s) cannot make a tool registered *after* a session's first
   python cell visible to that session's type checker. Every session on the
   scratch dir predates this build, hence every meeting session there is
   affected; a meeting recorded with this build has the post tools registered
   before its session exists and is not. An `agent`/08 issue, not fixed here;
   noted for phase 5. Fresh sessions call all four tools fine (item 7).
3. The failure path (`Failed` with `failure: "<stage>: …"`), a
   `Summarizing` manifest at restart (→ `Review` + failure note), the harvest
   "turn in flight" retry, `Enroll { speaker: "me" }`, `meeting.models_dir`
   set, a mic track from a user with headphones (the `me` segments here are
   one 93 s row: the far-end residual keeps the mic words gap-free).
   `Summarize` while the session's last batch turn is still running: the form
   is steered into that turn (agent `human_input`), so its `TurnFinished` is
   the summary's — unless the turn ends before the injection, in which case
   the wait settles early on that turn's text (or "no text") and the meeting
   returns to `Review` with `failure: "summary: …"` for a retry. Not staged.
4. Live time vs file time: `live.jsonl` stamps the clip's first utterance at
   55.8 s while the file's first sample above −34 dBFS is at 57.9 s and the
   first word at 57.6 s — the live clock (02 §10 deviation 7) runs ~2 s ahead
   of the Ogg timeline on this recording. The transcript of record is on file
   time, as §2 specifies; the live stamps are a phase-2 matter for phase 5.

### Deviations from the design above

1. **No `pyannote-rs` dependency.** 0.3.4 does not compile against the
   workspace `ort =2.0.0-rc.13` (29 errors: its `?` on `ort::Error<SessionBuilder>`
   needs `Send + Sync`, and its `ndarray 0.16` arrays do not feed rc.13's
   `ndarray 0.17` tensors). `post/diarize.rs` runs the two ONNX files on `ort`
   directly with `knf-rs = "0.3"` for the fbank (its `segment.rs` windowing:
   10 s windows, 270-sample frames from 721; `(shape, Vec)` tensors, no
   `ndarray` in our code). Workspace deps: `knf-rs`, `hound`; `meeting`
   gains `ort`, `base64`. The `ort` init order rule stands: `Engine::load`
   first.
2. **The wespeaker export needs its input tiled to a multiple of 200 frames.**
   Measured: a 3 s cut of one voice scored cosine 0.00 against her 4 s cut
   (norm 90 vs 23) while 2/4/6/8/10 s cuts scored 0.93–0.98; the same 3 s
   tiled to 400 frames scores 0.97. `Embedder::compute` repeats the frames up
   to `ceil(T/200)·200`. Without this, clusters followed span *length*, not
   voice (7 speakers for 2).
3. **Spans split on a powerset class change**, not only speech/non-speech
   (`pyannote-rs` collapses the 7 classes to a bool), so a span carries one
   voice; **clustering is agglomerative on centroid cosine** (closest pair
   first) rather than greedy in order — greedy glued a boundary span to
   whichever cluster came first; **`CLUSTER = 0.5`**, not 0.75: on the tapped
   recording the shorter spans of one voice score 0.57–0.85 to each other
   (0.75 split it into four), different voices 0.06–0.24 on both clips.
4. **A silence > 1 s also closes a system segment** (§0.2 grouped only by
   speaker); a 40 s monologue is otherwise one row.
5. **Types.** `Proposed { profile, name, score }` instead of `(Uuid, f32)`
   (the pane shows the name); `ProfileId` is an `id_type!` like `MeetingId`;
   `Transcript.note: Option<String>` carries the diarization-skipped reason;
   the wire name is `MeetingTranscript` (`Transcript.ts` is the voice note's
   reply, as `Status` was in 02). `EditOp` is the seven-variant enum of §3
   (incl. `Rename`, `Reviewed`), tagged `op`.
6. **Embeddings live in `embeddings.json` beside the transcript** (label →
   mean), written by the pipeline; `Enroll` reads it, so `apply` stays pure
   (`apply(t, ops, &embeddings, &mut profiles)`). The `me` embedding is
   computed in the diarize stage from ≤ 30 s of the mic word regions, not on
   demand. `Enroll` into an existing name is the running mean and also links
   the label and clears `proposed`; *accept* in the UI is `Enroll` with the
   proposed name (§7 said `Rename`+`Relabel`, which would not update the
   profile).
7. **Post state on disk after stop:** `StopMeeting` saves `Stopping` with
   `ended_at`/`duration_ms` and hands the manifest to the pipeline task, which
   saves `Post(_)`/`Review`/`Failed` and publishes its own events; resume at
   start also picks up `Stopping` + `ended_at` (a crash between the two
   saves). A `Summarizing` manifest at start goes back to `Review` with
   `failure: "summary: the daemon restarted …"`. A summary failure returns
   the meeting to `Review` (not `Failed`) with `failure: "summary: …"` so the
   button works again. `DeleteMeeting` aborts a running pipeline for that id.
8. **Harvest retry** is one retry after 3 s, not "after the next
   `TurnFinished`" (the finishing task runs off the actor). `manifest.title`
   is set from the summary's title when the user gave none (the list shows
   it). `SUMMARY_FORM` ends with "Reply with the markdown only."
9. **`Compact.head` also skips the pre-compaction harvest** (§0.6 said only
   the summariser): the meeting flow harvests explicitly one call earlier and
   the test asserts zero model calls.
10. **Post tools hold `Arc<Store>`**, not the actor: `meeting_read` resolves
    the session's meeting with `Store::by_session`, reads `live.jsonl`
    directly while the manifest says `Starting..Stopping`. `meeting_search`
    hits are `meeting "title" [mm:ss] Name: snippet`. `voice` gains
    `TDT_MODEL_SUBDIR` (the transcriber default and the pipeline's `tdt`
    path share it); `Transcriber.model` is `Arc<Mutex<Option<Engine>>>`.
11. **UI** (03's files, by the UI pass): `Review({ id, failure })`;
    `meeting_segment_audio` params are `number` (JSON has no bigint); the edit
    action replaces the cached transcript from the reply instead of
    refetching; *Re-run*/*Delete* on the Done pane confirm first; native
    checkboxes for multi-select; the entity on *Enroll…* is a text field (no
    knowledge picker component).
12. **One session link per state**: the header's *Session* button shows only
    while the meeting is live; once `Done` the summary pane's *Open chat* is
    the link (user's call after the first `.app` run — two buttons to the
    same place).
13. **Per-utterance transcription, not 180 s pieces** (§2 stage 1). The
    user's first real runs (speak, play a video, speak) fused the two `me`
    statements into one row and then lost system utterances. Measured on the
    34 s recording: the TDT given the whole 52 s system file (long −114 dBFS
    silences between utterances) returned 18 words — the second utterance
    replaced by a hallucinated tail — and stretched a word's end across the
    silence (`because[19.9–29.5]`), so §0.2's ">1 s silence" split never saw
    a gap. `transcribe::speech` now runs the live path's TEN-VAD + `Gate`
    over the track (onset backdated as `voice::live` does), `disjoint` trims
    the pre-roll overlaps, and each utterance (≤ 15 s by the gate's cap) is
    transcribed alone with 200 ms of context, word times clamped to it. On
    that recording both system utterances and the full mic sentences come
    back. Also, `align::group` closes a mic row where a system row starts
    (`cuts`), so the far-end residual the AEC leaves on the mic (transcribed
    as short `me` rows: "Principle.", "Okay.") cannot glue the user's own
    statements together. ponytail: those residual rows are not yet dropped;
    revisit after a run with deviation 14 — less ducking changes what the
    AEC leaves.
14. **VPIO ducking set to the minimum** (`capture/mic.rs`,
    `kAUVoiceIOProperty_OtherAudioDuckingConfiguration`, macOS 14+). The unit's
    default ducking made the call inaudible to the user, and the process tap
    records the ducked signal: `system.opus` speech sat at −55…−60 dBFS, at
    which the TDT hears nothing (the first system sentence of the 34 s
    recording is missing for that reason alone). Not verified by ear yet.
