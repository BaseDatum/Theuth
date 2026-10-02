# Meetings — phase plan for `53-meetings.md`

Status: plan v0 · 2026-09-19 · splits `../53-meetings.md` (design v0) into four
buildable phases plus one integration pass. Where a phase doc and 53 disagree,
53's **decisions (§0)** win and the phase doc says why it deviates; where 53 is
silent (see §2 below), the phase doc is the design.

## 1. Phases

| # | doc | deliverable | ends with |
|---|---|---|---|
| 1 | `01-capture.md` | crate `meeting`: plugin, config, actor + state machine, store (dir, manifest, `meetings.sqlite`), capture (VPIO mic + `flexaudio` tap), three live Opus tracks, lifecycle RPC, `MeetingEvent::{Started,State,Level,Deleted}`, the meeting's session created | three valid `.opus` files from a real call, pause gap in the manifest, delete removes everything |
| 2 | `02-live.md` | `voice::stt::Engine` extracted; two VAD+STT tasks; `live.jsonl`; batcher; session inputs/steers; `meeting_topic`/`meeting_brief` tools + prompt block; topic/brief bookkeeping; agent additions (per-session compaction override, `model` on `CreateSessionOptions`, read-only approvals switch) | a real model calls `meeting_topic` from a played-back recording, headless over RPC |
| 3 | `03-island.md` | desktop: island `NSPanel` window, capture exclusion + eye toggle, PTT + eye hotkeys, notch placement, brief stack UI, start form + tray entry, control center route, settings tab, bundle (`LSMinimumSystemVersion` 15, `NSAudioCaptureUsageDescription`) | a brief appears over full-screen Zoom while the meeting app keeps focus; Zoom's share does not show it |
| 4 | `04-post.md` | post pipeline: re-transcribe with word times, `pyannote-rs` diarization, speaker profiles, review RPC + UI, guided summary, title, harvest, compaction with summary head, `meeting_read/search/speakers/list` tools, FTS | stop → review → summarize → "what did John say about X?" answered from the follow-up chat via `meeting_search` |
| 5 | `05-brief-cache.md` | brief cache: `briefs.sqlite` (truth + FTS5), actor-side alias scan that shows cached briefs before any model turn (utterances, requests, start context), `meeting_topic` by cached id + alias learning, `brief_search/put/delete/list` tools in every session, prompt block for prefill/pruning, `/briefs` page, `BriefEvent` | a subject cached in meeting 1 is on the island < 1 s after it is spoken in meeting 2, before the model's turn starts; a chat session prefills briefs for tomorrow's meeting |
| 6 | `06-live-toolset.md` | while recording only: the meeting session runs with an allowlisted, direct-call toolset (web search included) and no python runtime (`agent::SetToolset`, cleared at stop); config `meeting.live_tools/live_prompts`; reviewed tool table | the first batch turn calls `brief_search`/`knowledge_search`/`search_web` directly with no fumble; after stop the same session is an ordinary one (python back, every tool) |
| 7 | §4 below | integration pass | 53 §12-equivalent end-to-end dogfood on a live meeting |

Dependency order: 1 → 2 → 4 → 5 → 6; 3 depends on 1's RPC and 2's events but can be
built against a fake `MeetingEvent` feed (`03-island.md` §7) once the contract
in §3 is frozen. Run 1, 2, 4, 5, 6 in sequence; 3 may overlap 2.

## 2. What 53 does not contain (as of 2026-09-19)

`53-meetings.md` ends at §4.7. Its header and §0 reference §5 (storage
layout), §6 (post pipeline), §7 (island), §10 (phone), §12 (checks), §13
(amendments) — none exist in the file. The phase docs fill those from §0's
decisions:

- storage layout → `01-capture.md` §5 (decision 13)
- post pipeline, diarization, review, summary → `04-post.md` (decisions 6, 7)
- island window, eye toggle, PTT, positioning → `03-island.md` (decisions 8–11)
- checks → each phase's last section; integration → §4 here
- amendments to 00/25/43/45 → §5 here

When 53 gains those sections, reconcile here first; the phase docs cite 53
by decision number, not section number, for exactly this reason.

## 3. Shared contract (frozen before any phase starts)

Everything below is declared in `crates/meeting/src/lib.rs` in phase 1, even
the parts phase 2/4 populate. Later phases add variants and fields; they do
not rename or retype what is here.

```rust
// crates/meeting/src/lib.rs — all Clone + Serialize + Deserialize + JsonSchema,
// #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub const MEETING_ACTOR: &str = "meeting";
pub const DIR: &str = "meetings";                       // <data_dir>/meetings/

pub struct MeetingId(pub uuid::Uuid);                   // v7
pub struct TopicId(pub uuid::Uuid);
pub struct BriefId(pub uuid::Uuid);
pub enum Track { Mic, System }

pub enum MeetingState { Idle, Starting, Recording, Paused, Stopping,
    Post(PostStage), Review, Summarizing, Done, Failed }
pub enum PostStage { Transcribing, Diarizing, Aligning }
pub enum Degradation { MicNoAec, SttDown(Track), ModelDown, DiskLow, NearMaxLength,
    PermissionDenied(Permission) }
pub enum Permission { Microphone, AudioCapture }

pub struct ProcessInfo { pub pid: u32, pub name: String, pub bundle_id: Option<String>,
    pub active: bool }

#[serde(tag = "type")]
pub enum MeetingEvent {                                  // one stream; the island and control
    Started { meeting: MeetingId, session: SessionId, process: ProcessInfo, at: String /* RFC 3339 */ },
    State { meeting: MeetingId, state: MeetingState, degraded: Vec<Degradation> },
    Level { meeting: MeetingId, mic_rms: f32, system_rms: f32 },        // 10 Hz, not persisted
    Utterance { meeting: MeetingId, track: Track, t0_ms: u64, t1_ms: u64, text: String, request: bool,
                #[serde(default = "yes")] final: bool,     // false = rolling partial of an open utterance (02 §2)
                #[serde(default)] speaker: Option<String> }, // 07 §4: live label `s1`..`s8` of a scored system line; a second final event carries it
    TopicAdded { meeting: MeetingId, topic: Topic },
    TopicRemoved { meeting: MeetingId, topic: TopicId, why: TopicGone },
    Brief { meeting: MeetingId, brief: Brief },
    Listening { meeting: MeetingId, held: bool },
    ApprovalWaiting { meeting: MeetingId, waiting: bool },
    PostProgress { meeting: MeetingId, stage: PostStage, pct: u8 },
    ReviewReady { meeting: MeetingId },
    Summarized { meeting: MeetingId, title: String },
    Deleted { meeting: MeetingId },
}
pub enum TopicGone { Viewed, Dismissed, Expired }
pub struct Topic { pub id: TopicId, pub label: String, pub at_ms: u64, pub brief: Option<BriefId> }
pub struct Brief { pub id: BriefId, pub topic: Option<TopicId>, pub title: String,
    pub bullets: Vec<String>, pub source: BriefSource, pub at_ms: u64, pub parent: Option<BriefId>,
    #[serde(default)] pub cached: Option<BriefId> }          // phase 5: the cache row this instance came from
pub enum BriefSource { Auto, Tap, Request, Deeper, Typed, Cache }   // Cache: phase 5

// phase 5 (05 §2, §8): CachedBrief { id, title, bullets, aliases, tags, source, created_at, updated_at,
//   uses, last_used_at }, CachedBriefPut, and a second event stream
//   #[serde(tag = "type")] enum BriefEvent { Changed { id: BriefId, removed: bool } }  // no `meeting`: the cache has none
```

Deviations from 53 §3.2, on purpose: every event carries `meeting` (the
bridge filters on top-level ids; `Level`/`Listening` gained it);
`ApprovalWaiting` is an event, not a `Degradation` (it flips per tool call);
`PermissionDenied` is a `Degradation` so a denied TCC prompt has one home;
`MeetingState::Failed` exists because `Starting` can fail. Times are
milliseconds on the recording clock (pauses excluded, 53 §3.2).

RPC method names are `meeting_*`; each phase doc lists its own. TS bindings:
`meeting/ts` appended to `crates/rpc/Cargo.toml` `features.ts`, each wire
type added to `crates/rpc/src/export.rs`, `MeetingEvent` added to
`crates/desktop/ui/src/rpc/events.ts` `WireEvent` — phase 1 does the first
two for the phase-1 types, later phases append.

Config section (whole feature; a phase only reads the keys it implements):

```yaml
meeting:
  enabled: true
  max_minutes: 180          # hard stop; warning (NearMaxLength) at max_minutes - 10
  context_tokens: 24000     # phase 2: per-session compaction trigger
  batch_secs: 25            # phase 2
  batch_utterances: 6       # phase 2
  live_model_dir: null      # phase 2: defaults to <data_dir>/models/nemotron-streaming-en — the directory dictation (54) uses; one Nemotron streaming checkpoint for both
  topic_ttl_secs: 600       # phase 2
  max_topics: 6             # phase 2
  models_dir: null          # phase 4: diarization models root; default <data_dir>/models (nemotron-diarization/, pyannote/)
  live_diarization: true    # 07: label the other side's live lines by speaker; off → `them:` (the post pass diarizes either way); next meeting start
  live_hint_limit: 2        # 06 §1.5 / 80: lines per `<knowledge>` note while recording; 0 = none
```

## 4. Integration pass (phase 7, done together)

Preconditions: phases 1–6 each have their Status section written with
"exercised / not exercised".

1. `cargo check --workspace --all-targets`, `cargo clippy --workspace
   --all-targets -- -D warnings`, `cargo test --workspace` (with
   `--test-threads=1` if the sqlite teardown hang from 00 §12 recurs),
   `cargo check -p rpc --features ts`, `just ts-desktop`, `bun run check`,
   vitest.
2. Cross-phase seams to re-read together, in this order:
   - actor state machine (01 §3) vs. what phase 2's batcher and phase 4's
     post task assume about `Paused`/`Stopping` (no utterances after stop;
     post starts only from `Stopping`).
   - `briefs.jsonl` writer (02) vs. control-center brief log reader (03).
   - `live.jsonl` (02) vs. `meeting_read` while recording (04).
   - `RegisterTool`/`UnregisterTool` of the two live tools at start/stop
     (02) vs. the four post tools registered at plugin start (04): the
     registry is read per turn, so a session mid-turn at stop sees the
     live tools vanish — confirm the tool-not-found error text is sane.
   - island cursor (`meeting_cursor`, 03) vs. control-center mirror (02
     stores it, 03 renders it).
   - `hidden_from_capture` (03) applies to both windows on startup, not
     only on toggle.
   - cache hot path (05 §3) vs. `Topics` rules (02): a cached topic obeys
     `max_topics`, TTL, dismiss and the `live:`/`dismissed:` dedupe exactly
     as an `Auto` one; `meeting_topic { brief }` for a label already live is
     refused like any duplicate.
   - cached `Brief` instances in `briefs.jsonl` (05) vs. the brief log (03)
     and `meeting_briefs` (02): `cached` is `Some`, `source` is `Cache`.
3. Live dogfood, one real call (Zoom or Meet in a browser), the `.app`
   built with `just`-equivalent bundle so TCC prompts are the real ones:
   1. tray → Start meeting… → picker shows the meeting app active; consent
      required; start.
   2. speak and let the other side speak: `Level` meters move
      independently; with speakers (no headphones) the mic track has no
      audible far end (AEC).
   3. someone names a ticket / system in the knowledge bundle: a topic
      button appears within ~30 s; tap → brief; button gone. Say it again
      in a second meeting: the button is there < 1 s after the words.
   4. hold PTT, ask a question, release → brief with `Request` source
      jumps into view; the `Listening` cue showed while held.
   5. step back one brief; let a new auto brief arrive → `↑ 1 new` badge;
      arrow up shows it.
   6. eye hotkey: the other side confirms the island (and main window)
      vanish/appear from the shared screen. Record the result in 53's
      decision 8 honesty note.
   7. pause 30 s, resume; stop → `PostProgress` to `ReviewReady`; review
      shows `Me` and `Speaker 1..n`; enroll one speaker; relabel; play a
      segment; fix one word.
   8. Summarize → title set, `summary.md` written, `knowledge_harvest`
      ran (new concepts in the bundle), session compacted, follow-up
      question answered via `meeting_search`.
   9. `meeting_delete` → directory, index row, session gone.
   10. `meeting.enabled: false` → no tools, RPC says disabled, island
       never created.
4. Write 53's §12 (checks) and §11-equivalent Status from the notes;
   append the §5 amendments below to the named docs in the same commit.

## 5. Amendments to other docs (applied at integration, not per phase)

- `00-architecture.md` §9: `meetings/` and `models/pyannote/` under the data
  dir. §10: crate `meeting` — "plugin (53): meeting recorder, live briefs,
  post-meeting transcript". §13 returned-as table: `meetings` → `meeting`
  plugin (53). Dependency chain line: `meeting` sits with `voice` (depends on
  `agent`, `tools`, `voice`, `runtime`; `rpc` names its types).
- `25-desktop.md` §4.5/§7: second window `island`; `set_content_protected`
  on both windows; global shortcuts PTT and eye; `LSMinimumSystemVersion`
  15.0 (was 12.0).
- `43-voice.md` §9 / `45-voice-notes.md` §9: `voice::stt::Engine` is the
  shared handle; `Transcriber` wraps one.
- `07-context.md` §8.1: per-session `SetCompaction` override (token
  threshold + hint), used only by meetings.
- `README.md` (repo): config bullet `meeting:`, crate list line.
