# 145 — Talk presence: the conversation continues while the work runs

Status: CONTRACT, 2026-10-08 14:45. Written by the orchestrator from Sky's four rulings of 14:20
(`decisions/theuth-talk-borrow-rulings-from-qwen-audio-agent-2026-10-08`) after the qwen-audio-agent recon
(`facts/theuth-borrow-list-from-qwen-audio-agent-2026-10-08`; reports in
`~/.theuth/briefs/2026-10-08/qwen-audio-agent/`). Two omp workers build it off this commit: W1 TALK
(`crates/talk`, the TUI and desktop talk mappings) and W2 AGENT (`crates/agent`, `crates/tools`). Amends 76
§0.9, §4.8, §4.11 and 77 §2.1; builds nothing of 77 B/C.

## 0. Decisions in one screen

1. **Keep 76's architecture.** One model talks and works; local STT/TTS; `talk` is a channel plugin with a pure
   policy. Rejected again: a second "voice-front" model (qwen-audio-agent's split costs ~2.8 s per short request
   in their own paper) and hosted realtime audio.
2. **Interruption ≠ cancellation, once something is Working.** New `State::Working`: a tool call of the turn is
   running and no speech is queued or playing. Speech while `Thinking` still cancels and resubmits (76 §0.9).
   Speech while `Working` is captured like any utterance and, at end of turn, **steered into the running turn**
   (`agent::Steer`, exists) — the work continues, the model reads the remark after the current tool batch. A
   barge-in while `Speaking`/`Holding` keeps 76 §4.5's five cuts unchanged. "Stop", "cancel that" are ordinary
   speech: the model is told to call `cancel`-shaped behaviour itself — no keyword detection in the policy.
3. **Results that land during a talk are spoken when idle.** A harness-made input (`<job_result>`, `<reminder>`,
   a child's or peer's `<message>`, a schedule run, `<job_idle>`) for an **idle** session with a live talk is
   parked while the spoken window is closed (user speaking, speech queued or playing, a turn pending) and
   released when it opens; everything parked is **coalesced into one turn** (items landing within 120 ms of
   each other join it); a 60 s cap releases regardless; a human input releases the parked items into its own
   turn as steers; the talk ending releases them. Sessions without a live talk are untouched.
4. **Promotion after 5 s.** While a talk is live, an inline tool call that has not returned after
   `talk.promote_after_ms` (default 5000) becomes a background job: the call returns at once with a `{job}`
   handle, the body keeps running under the `Jobs` actor, its result arrives later as `<job_result>` (through
   rule 3), and `cancel_job` stops it. Tools that are interactive or already job-shaped are never promoted.
5. **`ToolCx` gains a cancellation token.** The registry cancels the tokens of a cancelled turn's in-flight
   calls and of a cancelled promoted job; this wave makes the promoted-job path honour it (the task is aborted)
   and leaves `shell`/`python`/MCP observing it as follow-ups unless each is ≤ 40 lines.
6. **Approvals are announced, not answered, by voice.** When an `ApprovalRequested` lands on the talk's session,
   the talk says one sentence ("Waiting for your approval to run shell: `git push …`"); the answer still happens
   on the card or by the double. The sentence is harness speech: it never enters the heard prefix (`keep`) or a
   `TrimAssistant`.
7. **Nothing machine-shaped is ever spoken.** Sentences carrying a harness envelope tag are dropped from speech
   (they stay in the transcript); snake_case identifiers are read with spaces. The prompt also forbids it.
8. **Progress is visual.** The orb shows `Working…` from the state (not from session activity). Spoken progress
   only when the model chooses to, at most one sentence per minute per job (prompt rule); no filler is built here
   — 77 B stays documented, with the amendment that a filler may only play into idle silence.
9. **Spoken length.** Aloud: one or two sentences, then "the rest is on screen". Prompt rule, measured by ear.
10. **Boundary invariants are pinned in tests** (§5), the way their deterministic eval suite does it.

## 1. Problem, with evidence

- A spoken request is a blocking turn. The user hears the one pre-tool narration sentence the prompt asks for
  (`crates/talk/src/lib.rs#L411-L412`), then silence; the orb says `Working…` only because the UI reads session
  activity (`crates/desktop/ui/src/lib/talk.ts#L26-L44`).
- Speech while the turn runs cancels it: `late_addition` (`crates/talk/src/policy.rs#L746-L762`) emits
  `Cancel(None)` and resubmits alone — a side remark kills twenty seconds of work. `Delivery::Steer`
  ("inject after the current tool batch", `crates/agent/src/lib.rs#L146-L160`) exists and talk never uses it.
- The policy's `Input` set (`policy.rs#L27-L70`) has no notion of a job result, reminder or child message. Such
  inputs start a model turn through `Agents::input` (`supervisor.rs#L1183-L1200`, `InputOrigin::Automatic`,
  `continuation.rs#L45-L50`); if a talk is live the turn is spoken as "a turn typed elsewhere"
  (`policy.rs#L389-L398`) — while the user may be mid-sentence, and three children finishing together speak
  three times.
- `ToolCx { session, call_id, quiet, resources }` (`crates/tools/src/lib.rs#L294-L306`) has no cancellation
  token; a cancelled turn's tool body runs to completion behind a synthetic result (`registry.rs#L1915`). OPEN
  since 76 §12.
- Approvals are answered on the frontend only (76 §4.11, `docs/design/76-talk.md#L658-L665`).
- qwen-audio-agent's rules for exactly these cases — interruption never cancels accepted work
  (`deep-dive.md#L456-L460`), a safe insertion window `userSpeaking || turnPending || audio queued`
  (`announcement-window.mjs#L78-L84`), bounded batching with playback receipts (`announcement-manager.mjs`),
  progress visual-only (`deep-dive.md#L253-L271`) — are the same three rules GPT-Live, Gemini Live and LiveKit
  converged on (`facts/voice-agent-field-evidence-turn-taking-async-results-2026-10-08`).

## 2. Frozen signatures

Every name below is the contract; a worker that needs another shape messages the orchestrator instead of
inventing one. Doc comments are the worker's.

### 2.1 `crates/agent` (W2)

```rust
// crates/agent/src/lib.rs — next to `Steer` / `Delivery`.

/// `ask` → `()` (145 §0.3–0.4). A live talk tells the agent whether its spoken
/// window is open; `None` when the talk ends. Only sessions that have received
/// `Some(..)` park anything or promote tool calls.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SetTalk {
    pub session: SessionId,
    pub live: Option<TalkLive>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TalkLive {
    /// Open: harness-made inputs may start a turn (after `TALK_COALESCE_MS`).
    /// Closed: they are parked.
    pub open: bool,
    /// `talk.promote_after_ms`; `None` never promotes.
    pub promote_after_ms: Option<u64>,
}

/// Items landing within this window join one turn.
pub const TALK_COALESCE_MS: u64 = 120;
/// A closed window releases what it parked after this long regardless.
pub const TALK_PARK_CAP_MS: u64 = 60_000;
```

Behaviour in `Agents::input` / `route` (`supervisor.rs`): an `InputOrigin::Automatic` input for a session whose
`talk` is `Some` and which has **no running turn** (the same busy test `defer` uses) is (a) parked when
`!open`, (b) otherwise held `TALK_COALESCE_MS` and then routed **as one input** together with everything parked
(texts joined by a blank line, each keeping its own envelope; `files` concatenated; one ticket per original
input is admitted and consumed by that turn), (c) released by `SetTalk{open: true}`, by `SetTalk{live: None}`,
by the cap, or by an `External` input (routed first; the parked items follow it as `Delivery::Steer`). A busy
session keeps today's behaviour exactly. The agent passes `promote_after: Some(Duration)` into every `ToolCx`
of a session whose `talk.live` is `Some` with `promote_after_ms: Some`.

```rust
// crates/agent/src/lib.rs — AgentEvent gains one variant (ts(export) like its siblings).
    /// 145 §0.3: harness inputs parked behind a closed spoken window.
    InputsParked { session: SessionId, count: u32 },
```

### 2.2 `crates/tools` (W2)

```rust
// crates/tools/src/lib.rs
pub struct ToolCx {
    pub session: SessionId,
    pub call_id: String,
    pub quiet: bool,
    pub resources: Arc<SessionResources>,
    /// 145 §0.5: cancelled when the turn or the promoted job is cancelled.
    pub cancel: tokio_util::sync::CancellationToken,
    /// 145 §0.4: promote to a job after this long; `None` never.
    pub promote_after: Option<std::time::Duration>,
}

// crates/tools/src/lib.rs — on the `Tool` trait, default `true`.
    /// A call may be promoted to a background job (145 §0.4). `false` for
    /// shell/start/send/wait/screen/jobs/cancel_job (job-shaped), ask_user,
    /// desktop_handoff, secret_request, secret_request_form, approval-shaped
    /// tools, spawn_subagent, send_message (an explicit wait is the caller's).
    fn promotable(&self) -> bool { true }
```

```rust
// crates/tools/src/jobs.rs — `Jobs` gains one message.
/// `ask` → `JobId` (145 §0.4): run a tool call that outlived `promote_after`
/// to completion as job `jN`. Publishes `JobEvent::Started { kind: Promoted, .. }`
/// and `JobEvent::Finished` whose body is the tool's output (`is_error` → exit 1);
/// `CancelJob` cancels `cancel` and aborts the task.
pub struct Adopt {
    pub session: SessionId,
    pub call_id: String,
    pub tool: String,
    pub cancel: CancellationToken,
    pub work: Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send>>,
}
```

`registry.rs::dispatch`: `select!` the call against `promote_after`; on timeout and `promotable()`, `Adopt` and
return `Ok` with the JSON `{"job": "<id>", "tool": "<name>", "promoted_after_ms": N, "note": "the result arrives
later as <job_result>; do not wait or poll"}`. The existing `<job_result>` wrapper (`supervisor.rs::job_result`)
names `tool="<name>"` instead of `command=` for a promoted job. A cancelled turn cancels the `cancel` token of
every in-flight call of that turn before writing the synthetic result (existing path kept).

### 2.3 `crates/talk` (W1)

```rust
// crates/talk/src/lib.rs
pub enum State { Idle, Listening, Thinking, Working, Speaking, Holding, Interrupted }
//                                           ^ new: a tool call runs, nothing queued or playing (serde "working")

pub struct TalkConfig { /* … existing … */
    /// 145 §0.4: inline tool calls promote to a job after this long; 0 = never.
    pub promote_after_ms: u64,   // default 5000
}

pub enum TalkEvent { /* … existing … */
    /// 145 §0.6: a harness sentence the talk spoke (approval announce).
    Announced { id: TalkId, text: String },
}
```

```rust
// crates/talk/src/policy.rs
pub enum Input { /* … existing … */
    /// Agent `ToolCallStarted` / `ToolCallFinished`: outstanding calls of the turn.
    Tools(u32),
    /// `ApprovalRequested` on the talk's session: a one-line, pre-redacted summary.
    Approval(String),
}
pub enum Action { /* … existing … */
    /// `Agents::Steer { delivery: Steer }` with the utterance (145 §0.2).
    Steer(String),
    /// `Agents::SetTalk { live: Some(TalkLive { open, .. }) }` on change (145 §0.3).
    Window(bool),
    /// Publish `Announced`.
    Announced(String),
}
pub struct Sentence { /* … existing … */
    /// Harness speech (145 §0.6): never part of the heard prefix or a Trim.
    pub harness: bool,
}
```

Window predicate, evaluated after every `step`: `open = state == Listening && candidate.is_none() && gate closed
&& !ledger.busy()`; `Action::Window` is emitted only when the value changes, and `Window(false)` once at talk
start, `SetTalk{live: None}` by the actor at talk end. Working is entered from `Thinking` when `Tools(n>0)`
arrives and no sentence is queued or playing, left for `Thinking` on `Tools(0)`, for `Speaking` on
`SpeechStarted`; `maybe_listen` treats `Working` like `Thinking`. An utterance that commits while `Working`
emits `Steer(text)` and stays `Working` (no `Cancel`, no `Submit`). `late_addition` runs only while `Thinking`.
Envelope guard in `say`/`speak_*`: drop a sentence matching `</?(job_result|job_idle|reminder|message|until|
schedule|knowledge|surface_error)\b`; replace `_` inside `[a-z0-9]+(_[a-z0-9]+)+` with a space.

### 2.4 Frontends (W1)

`crates/desktop/ui/src/lib/talk.ts`: `orbState('working') === 'thinking'`; `phaseLine` returns `Working…` for
`working` from the state (keep the activity fallback for `thinking`); `orbHint('working')` = `Tap the orb to
stop`; `Announced` → the agent subtitle line. The generated binding stub `State.ts` is hand-edited in the worktree
(gitignored; never committed). `crates/tui/src/talk.rs::look`: `Working → Look::Thinking`; every exhaustive
`match` on `State` in `crates/tui` compiles.

## 3. File ownership

| Worker | Owns | Must not touch |
|---|---|---|
| W1 TALK | `crates/talk/**`, `crates/tui/src/talk.rs` (+ the `State` matches in `crates/tui/src/{ui,model}.rs`), `crates/desktop/ui/src/lib/talk.ts`, `talk.test.ts`, the gitignored binding stubs under `crates/desktop/ui/src/rpc/bindings/` | `crates/agent/**`, `crates/tools/**`, `crates/rpc/**` |
| W2 AGENT | `crates/agent/**` (`lib.rs`, `supervisor.rs`, `loop_.rs`, `durability/jobs.rs`), `crates/tools/**` (`lib.rs`, `registry.rs`, `jobs.rs`, the `promotable` overrides in the listed tool files) | `crates/talk/**`, `crates/tui/**`, `crates/desktop/**`, `crates/rpc/**` |
| Orchestrator | `docs/design/145-*.md`, bindings regeneration (`just ts-desktop`) in the integration tree, `crates/rpc` if a signature leaks there | — |

W1 depends on `SetTalk` only (`Steer` exists today). W2's **first commit, within ~20 minutes**, adds the §2.1
types with a no-op handler and tells the orchestrator `PIN 145-AGENT <sha>`; the orchestrator merges that commit
into W1's worktree and says so. Until then W1 works on the pure policy and its tests. The 144 NOTEBOOKS wave edits
`crates/agent/src/lib.rs` (prompt assembly) and `store.rs` concurrently; whoever lands second folds and keeps
both.

## 4. Merge rules

1. Both workers branch from this commit (`feat/145-int`). Each owns a worktree and a `CARGO_TARGET_DIR` under
   `~/Develop/theuth-145-target/<name>`; one build at a time per worker; crate-level gates only, through
   `BUILD_LABEL=145-<W> ~/.theuth/bin/build-lock`, `CARGO_BUILD_JOBS=4 nice -n 10`.
2. W2 lands first (W1 depends on `SetTalk`). The orchestrator merges W2 into `feat/145-int`, then W1, regenerates
   bindings (`just ts-desktop`), runs tsc/vite/vitest for `crates/desktop/ui`, `cargo test -p talk -p agent -p
   tools`, `cargo clippy -p talk -p agent -p tools`, then one `cargo check --workspace --all-targets` behind
   `BUILD_SLOTS=1` and the `/tmp/theuth-build.lock` mkdir lock, after the 144 session's.
3. Fold `origin/main` before landing; land with `merge --no-ff -F msgfile`; push origin and basedatum.
4. A worker's `DONE <W> <sha>` message is ≤ 30 lines: files, tests added/passing, deviations from §2.

## 5. Proof list

Policy tests (`crates/talk/src/policy.rs`, pure, no model):
- P1 `Thinking` + `Tools(1)` → `Working`; `Tools(0)` → `Thinking`; `SpeechStarted` while `Working` → `Speaking`.
- P2 utterance committed while `Working` → exactly one `Steer(text)`, no `Cancel`, no `Submit`, state unchanged.
- P3 utterance committed while `Thinking` → unchanged 76 §0.9 behaviour (`Cancel(None)` then `Submit`).
- P4 barge-in while `Speaking` with `Tools(1)` → the five cuts of 76 §4.5, unchanged.
- P5 `Window`: closed on `Onset`, open again after a backchannel resume, closed from `Submit` until `maybe_listen`,
  emitted only on change.
- P6 `Approval("shell: git push")` → one `Speak` whose sentence has `harness: true`, one `Announced`; a barge-in
  afterwards computes `keep` without it.
- P7 envelope guard: `"<job_result id=\"j1\">done</job_result>"` is not spoken; `"I ran search_web"` is spoken as
  `"I ran search web"`.
- P8 no sentence is re-spoken after `Interrupted`; late `Delta`s after our `Cancel` stay unspoken (existing, kept).

Agent/tools tests:
- A1 `SetTalk{open:false}` + two `Automatic` inputs on an idle session → nothing routed, `InputsParked{count:2}`;
  `SetTalk{open:true}` → one turn whose user text holds both envelopes in arrival order.
- A2 two `Automatic` inputs 50 ms apart on an open window → one turn; 500 ms apart → two.
- A3 cap: parked input released after `TALK_PARK_CAP_MS` with the window still closed.
- A4 `External` input on a session with parked items → the human turn starts, parked items arrive as steers.
- A5 a session without `SetTalk` behaves byte-for-byte as before (existing supervisor tests stay green).
- A6 `SetTalk{live: None}` releases parked items and switches promotion off.
- T1 a tool future that sleeps 10 s with `promote_after = 5 s` → returns the `{job}` JSON before 6 s, `JobEvent::
  Started{Promoted}` then `Finished` with the tool's output; `<job_result tool="…">` wrapper test.
- T2 `promotable() == false` tool is awaited to completion regardless of `promote_after`.
- T3 `CancelJob` on a promoted job cancels its token and the future never completes; `Finished` says cancelled.
- T4 cancelling a turn cancels the tokens of its in-flight calls (observable through a test tool).

By hand after landing (orchestrator, release `t` + desktop rebuild): say "run the test suite" then a side remark
while `Working…` shows — the remark is answered after the batch, the work finishes; a child finishing while you
speak is spoken only after you stop; three children finishing together produce one sentence.

## 6. Out of scope

77 B/C fillers (documented; amendment: idle-silence gating), STT tiers, spoken yes/no for approvals, wake word /
sleep, vision, multi-device claims, restart recovery of parked inputs (lost on restart; bounded by the 60 s cap —
`<job_result exit="lost">` still arrives for promoted jobs through 109), `shell`/`python`/MCP honouring the cancel
token beyond the promoted path, speaking children's `task_note` progress on a schedule, a turn-generation
counter (the ledger's `SpeechId`s and `cancelled` flag already drop stale events).
