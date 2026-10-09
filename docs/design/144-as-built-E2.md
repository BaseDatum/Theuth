# 144 as built — E2 (ENGINE-ASK-OVERVIEWS-AUDIO)

Worker 144-E2, branch `feat/144-e2`, cut from `feat/144-w2-base` 6ab1905f. Ask across notebooks, generated
overviews, the audio overview (design 144 §9, §10, §13; wave-2 contract §2).

## What landed (file:line)

### `SessionKind::Ask`
- `crates/agent/src/lib.rs` `SessionKind::Ask` (+ doc): a normal durable root without a notebook binding whose first
  turn the ask run writes. `crates/agent/src/store/advisor.rs` `kind_str`/`kind_of` ↔ `"ask"` (the only exhaustive
  `match` on `SessionKind` in the workspace — every other use is `==`/`!=`; grep before the first gate). The live
  loop treats `Ask` like `Chat` (advisor/consult hooks test `== Chat`/`== Advisor` only). `crates/agent/src/store.rs`
  `Store::set_kind(id, kind)` beside `set_title`.

### Ask (`crates/agent/src/ask.rs`)
- Types (contract §2, ts-rs exported): `AskEvent` (`#[serde(tag = "type", rename_all = "snake_case")]`: `Plan`,
  `Branch`, `Partial`, `Answer`, `Done`), `AskBranch`, `AskHit`, `NotebookAsk { session, event }` (bus event,
  `#[serde(tag = "type")]` → wire `{"type":"NotebookAsk","session":…,"event":{"type":"plan",…}}`), `AskStarted`,
  `AskSummary`. Notebook ids are `String` in the agent crate (no `document` edge); the RPC layer takes `DocId`.
- `AskConfig` reads `notebooks.{ask_model, ask_max_branches (5), ask_hits_per_branch (10), context_budget_tokens}`
  with the `Budget::from_runtime` pattern.
- `run(driver, scope, titles, question, cfg)`: `plan` (one planning call, `parse_plan` validates — branches ⊆ scope,
  non-empty query, ≤ max, 1-based `n`; one retry on bad JSON, else `single_branch` = the question over the whole scope)
  → `AskEvent::Plan` → the branches concurrently (`join_all`): `search` (nb_search scoped to the branch's notebooks,
  `ask_hits_per_branch`) → `Branch { n, hits }` → one model call over the passages cut to `context_budget_tokens /
  branches` (one retry; a branch with no passages answers without a call) → `Partial { n, answer }` → synthesis under
  `SYNTH_SYSTEM` ("use only hrefs present in the partials; every factual sentence carries a chip") → `filter_chips`
  drops any chip whose document id is not among the branches' hits → `Answer { markdown }` → `driver.commit` → `Done`.
- `Live` driver: model = `SideCall` on the ask session (`ModelRole::Named(ask_model)` for the plan when configured,
  else `Session`; `CallKind::Round`, 180 s); search = registry `nb_search` + one `doc_read(format=markdown, blocks)` per
  document (`source:`/`notebook:` prefixes by hit kind), split at `<!-- id -->`; emit = `rt.publish(NotebookAsk)`;
  commit = `UnloadSession` (68 §4.3's pattern) → `Store::append(user question)` + `append(assistant answer)` → the
  next input reloads the frame with the turn and `kind = ask` → `save_answer_as_note(opener seq, nb)` when `save_into`.
- Entry points: `notebook_ask(agents, scope, question, save_into)` = `CreateSession` (durable, unbound) →
  `RenameSession` (the question's first 12 words, `fallback_title`) + `PinTitle` → `StartAsk` (sets `kind`, records
  the run in `<data_dir>/notebooks/asks.json`, spawns the run off the mailbox); `notebook_ask_list(limit)` =
  the records newest first (`done` flipped when the run ends). Python tool `notebook_ask(question, notebooks,
  save_into?)` (ReadOnly) registered in `supervisor.rs` beside `session_read`.

### Overviews (`crates/agent/src/overviews.rs`)
- Types (contract §2, ts-rs): `OverviewKind { Briefing, Faq, Timeline, MindMap, Audio }` (`file()` →
  `briefing.md | faq.md | timeline.md | mindmap.md | overview-script.md`, `title()` → the pinned session title),
  `OverviewStatus { Running, Ready, Failed }`, `Overview`, `OverviewRun`.
- Records: `<data_dir>/notebooks/overviews.json` = `Vec<Record { notebook, kind, session, shown: Vec<Shown { name,
  call_id, version, created_ms, note }> }>` — the `(notebook, kind) → session` mapping and every version; live run
  states in a process-wide `RUNS` map. `list(records, runs, notebook)` is pure: per kind the running/failed row
  (version = latest + 1, `artifact: None`) then every shown version newest first (same version: name order, so the
  audio script precedes the mp3).
- `instruction(kind)`: the skill-shaped text (every claim a chip; FAQ = one `Accordion` fence; Timeline = one
  `Timeline` fence `scale: time`; Mind map = one `Tree` fence, `Graph` when cross-links matter; Audio = outline then a
  two-host `**Ada:**`/`**Ben:**` script, 1 000–1 300 words) ending in `show(content, name="<file>")`.
- The run: `notebook_overview_run` finds the kind's session in the records or creates one (`CreateSession { notebook }`,
  renamed `Overview · <kind>` and pinned, `BindOverview`), then `StartOverview` (refused while running) spawns:
  `SendInput { delivery: NextTurn }` of the instruction → the `Watch` actor (one per process, subscribed to
  `AgentEvent`) streams the session's events → `follow_turn` collects `ToolCallStarted{name:"show"}` /
  `ToolCallFinished{!is_error}` pairs (version from the receipt's `(v<N>`, content from the args) until
  `TurnFinished` (20 min cap) → the expected `show` (else the last assistant text is `show`n by the engine as the
  file) → `notebook_add(notebook, artifact, call_id)` → record. Audio continues in `overview_audio::render`.
- Python tools `notebook_overview_run(notebook, kind)` (Mutating) and `notebook_overviews(notebook)` (ReadOnly).

### Audio overview (`crates/agent/src/overview_audio.rs`, `crates/voice/src/{clip,lib}.rs`)
- `parse_script` cuts the script into `(speaker, text)` turns (`**Ada:** …` paragraphs; continuation paragraphs join);
  `assign_voices(speakers, notebooks.audio_voices, available, fallback)` alternates two or more usable voices by
  speaker, else one voice reads every host with `note = "one voice available — both hosts read by <voice>"`.
- `render(rt, session, script)`: `voice::Status` (a one-word `Synthesize` first when the checkpoint has not loaded, so
  `voices` is filled) → `voice::SynthesizeDialogue { turns }` → the mp3 written under `<data_dir>/notebooks/tmp/`
  and `show`n as `overview.mp3` (title `Audio overview · N min`) → `(call_id, version, note)`.
- `crates/voice/src/lib.rs`: new `SynthesizeDialogue { turns: Vec<DialogueTurn { voice: Option<String>, text }> }`
  (reply like `Synthesize`), `DIALOGUE_GAP_MS = 350`; `pending_clips` and `clip()` carry `(voice, spoken)` turns, so
  `Synthesize` is the one-turn case on the configured voice (behaviour unchanged). `crates/voice/src/clip.rs`
  `dialogue(synth, turns)` = one `Synthesizer::stream(spoken, voice)` per turn, PCM joined with `GAP_MS` of zeros,
  `render` encodes once with the existing `mp3()` (LAME, 24 kHz mono, 48 kbit/s).
- **Voice switch verified** (brief item 3): `ptts::synth::SynthOf::stream_with_rng` (checkout
  `~/.cargo/git/checkouts/xn-ptts-*/5d50b52/ptts/src/synth.rs:474-484`) resolves `opts.voice` by
  `self.voices.get(name)` in the `BTreeMap<String, Voice>` filled once by `add_voice_file` at `model::load`
  (`crates/voice/src/model.rs:97-103`); nothing reloads — a per-call voice is a map lookup.
- `crates/agent/Cargo.toml` gains `voice` (cycle-free: `cargo tree -p voice` has no `agent`).

### RPC (`crates/rpc/src/{api,bridge,export}.rs`)
- `notebook_ask(scope: Vec<DocId>, question, save_into: Option<DocId>) -> AskStarted`, `notebook_ask_list(limit) ->
  Vec<AskSummary>`, `notebook_overview_run(notebook, kind) -> OverviewRun`, `notebook_overviews(notebook) ->
  Vec<Overview>` — all `owner(ext)` like the wave-1 notebook methods; bodies delegate to `agent::ask` /
  `agent::overviews`.
- `bridge.rs`: `NotebookAsk` in `forward!` and `rt.subscribe`, delivered on `subscribe_events` exactly like
  `SourceEvent` (its top-level `session` lets a session-filtered subscription see it).
- `export.rs`: `AskEvent, AskBranch, AskHit, NotebookAsk, AskStarted, AskSummary, OverviewKind, OverviewStatus,
  Overview, OverviewRun` exported.

## Proof

## Gates

## Deviations
- **Branches are concurrent in-process side calls, not `spawn_subagent` children** (the brief's allowed fallback).
  A theuth child's result reaches its parent only as a `<message>` input to the parent's *model loop*; there is no
  awaitable result, no allow-list-only toolset (`SetCap` is a risk ceiling, `Toolset` is per-session UI state), and
  the ask session must not have a model turn of its own while the run writes its answer. The in-process branch keeps
  the allow-list discipline exactly: a branch's model sees only the passages `nb_search` returned for its notebooks,
  the synthesis sees only the partials and their hrefs, and `filter_chips` strips any chip outside the hits.
- **The answer turn is written to the store after `UnloadSession`**, not through the live actor (no API appends an
  assistant turn to a running `Session`): the pattern of 68 §4.3 (`sessionsync::owner`). Consequence: the
  `save_answer_as_note` title side call has no live actor and falls back to the answer's first words (W3's documented
  fallback); the next user input respawns the actor from the store.
- **Side calls run as `CallKind::Round`** (plan, branches, synthesis): adding an `Ask` kind means editing
  `runtime::side_call` (not owned); the cost rows read as rounds of the ask session.
- **Records are JSON files** (`<data_dir>/notebooks/{asks,overviews}.json`), not a table: the contract reserves 0045 for
  E1 and forbids another agent migration this wave; the notebook document has no free-form meta. `AskSummary.scope`
  and `question` live there (the session row holds only the 12-word title).
- **Overview turns, including the audio outline + script, run as one real turn in the bound session** rather than
  side calls: a side call carries no `<notebook>` block, so the dial's context (E1's block, wave 1's until it lands)
  reaches the model only through a real turn. The engine captures the `show` from the turn's tool events; if the
  model wrote the document in its reply instead, the engine `show`s it under the right name.
- **One voice shipped.** This Mac has exactly one speaker embedding (`~/.theuth/models/pocket-tts/voices/
  peter_yearsley.safetensors`), no `ptts` binary and no public-domain speech sample on disk (the only audio fixture is
  `crates/document/tests/fixtures/ingest/tone-3s.wav`); re-embedding the existing voice is ruled out by the brief.
  `notebooks.audio_voices` defaults to `["peter_yearsley"]`, both hosts read in it and the mp3 row carries
  `note = "one voice available — both hosts read by peter_yearsley"`.
- `crates/agent/src/notebooks.rs:400` `invoke` became `pub(crate)` (E1's file; one keyword) so `ask`/`overviews` reuse
  the registry call instead of a copy.
- `notebook_overview_run` is `Mutating` as a python tool (it files items and creates a session); `notebook_ask` and
  `notebook_overviews` are `ReadOnly` per §13.2.
- `notebook_add` / `doc_create` / `show` from the engine go through the registry as the session (W3's deviation):
  with `tools.policy.mutating = ask` the `notebook_add` of an overview artifact raises an approval card in the overview
  chat.

## Hand-offs
- **U2 (TS stubs, exact shapes):**
  ```ts
  export type AskEvent =
    | { type: 'plan'; branches: AskBranch[] }
    | { type: 'branch'; n: number; hits: AskHit[] }
    | { type: 'partial'; n: number; answer: string }
    | { type: 'answer'; markdown: string }
    | { type: 'done'; ok: boolean; error: string | null }
  export type AskBranch = { n: number; notebooks: string[]; query: string; instructions: string }
  export type AskHit = { id: string; block: string | null; title: string; score: number }
  export type NotebookAsk = { type: 'NotebookAsk'; session: SessionId; event: AskEvent }   // on subscribe_events
  export type AskStarted = { session: SessionId }
  export type AskSummary = { session: SessionId; question: string; scope: string[]; created_ms: number; done: boolean }
  export type OverviewKind = 'briefing' | 'faq' | 'timeline' | 'mind_map' | 'audio'
  export type OverviewStatus = 'running' | 'ready' | 'failed'
  export type Overview = { kind: OverviewKind; artifact: string | null; name: string; version: number; created_ms: number;
    session: SessionId; status: OverviewStatus; note: string | null; error: string | null }
  export type OverviewRun = { session: SessionId }
  // Api: notebook_ask(scope: DocId[], question: string, save_into: DocId | null): Promise<AskStarted>
  //      notebook_ask_list(limit: number | null): Promise<AskSummary[]>
  //      notebook_overview_run(notebook: DocId, kind: OverviewKind): Promise<OverviewRun>
  //      notebook_overviews(notebook: DocId): Promise<Overview[]>
  ```
  `AskBranch.notebooks`/`AskSummary.scope` are plain `string` ids (not `DocId`) because the agent crate exports them.
  The ask session's transcript shows the turn only after `done` (the UI should reload the session then); until then
  the `NotebookAsk` events are the only view. An `Overview` row with `status: 'running'`/`'failed'` has
  `artifact: null` and the next version number.
- **A second voice embedding needs** (brief item 3): a ≥ 10 s (exactly 10 s is what `TTSConfig::v202601`'s
  `audio_prompt_max_duration`/min accept — `add_voice_from_pcm` bails on shorter) mono clip of a public-domain speaker,
  encoded with `ptts`'s `examples/create_voice.rs` (`cargo run --example create_voice -- --config <TTSConfig json | hf
  id> --weights ~/.theuth/models/pocket-tts/tts_b6369a24.safetensors --input <wav> --output alba.safetensors`; needs the
  Mimi encoder weights the checkpoint ships) and dropped as `~/.theuth/models/pocket-tts/voices/<name>.safetensors`
  before the voice model loads (`model::locate` scans `voices/*.safetensors` once). Then `notebooks.audio_voices:
  [peter_yearsley, <name>]` — nothing else changes; `assign_voices` alternates and the note disappears.
- Closer: regenerate bindings (`mbx test -p rpc --features ts --lib export_bindings` + copy) for the ten new types and
  four methods; `methods.json` grows by `notebook_ask`, `notebook_ask_list`, `notebook_overview_run`,
  `notebook_overviews`.
- E1: `notebooks.rs:400` `invoke` is `pub(crate)` on this branch (merge keeps it).
