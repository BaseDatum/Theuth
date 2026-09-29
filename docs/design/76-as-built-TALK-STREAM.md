# 76 as built — TALK-STREAM (speak sentences as they stream)

Branch `fix/talk-sentence-stream` (worktree `theuth-talk-stream`), based on
`main` @ db4fa2c3 (the brief named 31d0d710; the worktree was cut at the later
merge db4fa2c3, and nothing in `crates/talk` or `crates/voice` differs between
them for this work). Follow-up A of the first-audio latency recon
(`facts/theuth-talk-first-audio-latency-cause-2026-09-29`): a one-paragraph
reply was heard only at end of turn because `speak_committed` released a
block only when it closed.

Started by an omp worker that died in the 2026-09-29 provider outage; its
uncommitted diff (the `speak_open` skeleton, `is_prose`, the latency logs,
the `!cancelled` delta guard) was reviewed, kept, and finished by a theuth
subagent.

## 1. What was built

### `crates/talk/src/policy.rs`

- `Policy.streamed: usize` — sentences of the open block already spoken.
  `reset_md()` clears `md`, `spoken_to` and `streamed` together (every place
  that used to reset `md`/`spoken_to` by hand calls it).
- `speak_open()` runs after `speak_committed()` on every `Input::Delta`. The
  open block is `md.raw()[md.committed()..]`. When `is_prose(tail)` holds it
  cuts `markdown::spoken(tail)` with `split()` and speaks every complete
  sentence past `streamed`. "Complete" means the raw tail has whitespace after
  terminal punctuation. `spoken()` trims the tail and adds a `.`, so the
  splitter only sees that whitespace when the raw tail really has it; this
  also keeps an abbreviation at the end ("Dr. ") from being treated as a
  sentence end. A sentence that still has `*` `_` a backtick `[` `<` `~` `|`
  in its spoken form (unbalanced markup reads as literal, or the line could
  become a table row) is held, with everything after it, until the close
  renders it.
- `speak_committed()` skips the first `streamed` sentences of the newly
  committed range and resets `streamed`. The committed range always starts at
  the block that was streaming (`spoken_to == committed` when `speak_open`
  runs), and both paths use the same `spoken()` and the same splitter, so the
  streamed sentences are exactly the first `streamed` of the commit.
- `say()` is the one place that makes a ledger entry and pushes
  `Action::Speak` (the index is the ledger length, so it stays monotonic per
  turn).
- `is_prose(tail)`: pulldown-cmark (same options as `markdown::spoken`) gives
  exactly one block and it is a `Paragraph`, and the first line has no `|`.
  A second block in the tail means mdstream is about to commit the first.
  Headings, lists, fences, quotes, HTML and indented code parse as themselves
  and are not streamed. A first line that could still become a list marker
  ("1", "-") has no complete sentence yet, so it waits for its kind as before.
- `Input::Delta` is ignored while `cancelled` (our own `Cancel{keep}` or
  late-addition cancel is in flight). Streaming would otherwise speak the
  cancelled turn's last few deltas; before this change the same could happen
  for a block that committed in those deltas.
- One shared splitter. `sentences(line)` is now `split(line.trim())` plus its
  rest. `split(text) -> (complete, rest)` cuts after `.` `?` `!` followed by
  whitespace, except after an entry of `ABBREVIATIONS` (e.g., i.e., vs., cf.,
  approx., Dr., Mr., Mrs., Ms., Prof., St., case-insensitive, leading `(`
  ignored). A decimal (`3.5`) has no whitespace after its dot and never
  splits. **Known misses** (pinned in a test): an unlisted abbreviation or an
  initial splits ("Ask J." / "Smith."); a listed one that really ends a
  sentence joins it to the next ("Main St. Next."). "etc." is deliberately not
  listed because it ends sentences more often than not. A miss only moves a
  sentence boundary, never a word.

### `crates/talk/src/lib.rs`

- `Live.turn_started: Option<Instant>`, set on the session's
  `AgentEvent::TurnStarted`. INFO under target `talk`, once per turn:
  `talk: first sentence spoken after {ms} ms (turn started → first Speak)` on
  `Action::Speak { index: 0 }`, and `talk: first audio after {ms} ms` on the
  `Saying` of ledger sentence 0. `Saying` is emitted on `SpeechStarted`, which
  the voice task sends when the synthesis stream has opened, just before the
  first frame reaches the ring or the client.
- The talk prompt's "spoken sentence by sentence as you write it" is now true
  for prose, so it is unchanged.

### `crates/talk/Cargo.toml`

- `pulldown-cmark` (workspace) for `is_prose`.

### `docs/design/76-talk.md` §4.8

- The paragraph that said "a long paragraph still waits for its own end" now
  describes sentence streaming, the block kinds that still commit on close,
  the held-markup rule, and the known misses.

## 2. Not built: TTS pipelining (step 3)

Skipped on purpose. The brief allows that when the change would not be clean.
In `crates/voice` the queue is not in `speak.rs`: `Voice::advance` in `lib.rs`
starts the next `Queued` utterance only when `current` is empty, i.e. on
`Progress::Finished`. On the device sink, `speak::synthesize` writes into
`Player::write`, which blocks while the ring is full. So synthesis of
sentence N ends about one ring (`player_buffer_ms`) before N stops playing,
and only then does synthesis of N+1 start. The gap between sentences is
N+1's time to first chunk (~200 ms, 43 §0.5) minus the audio still in the
ring. Closing that gap takes a prefetch slot in the `Voice` actor: a second
`spawn_blocking` for `queue[0]` that synthesises into a bounded buffer and
hands it over when it becomes `current`. That slot has to be dropped on
`Stop{None}`, on a replacing `Speak`, and on flush. It lives in `lib.rs`
(outside this brief's allow-list) and touches the barge-in stop path.
**Next step:** a `Prefetch { id, rx, cancel }` beside `current`, filled by
`advance` when the queue is non-empty and the model is loaded. On `Finished`
it drains `rx` into the player instead of calling `synth.stream`. Clearing
the queue cancels it. Test with the fake `Synthesizer`: three queued speaks,
where the second's stream opens before the first's `Finished`, and
`Stop{None}` publishes `Stopped` for the prefetched id.

## 3. Verification

- `cargo test -p talk`: 20 passed. That is the 16 existing tests (three
  changed for sentence streaming, see below) plus 4 new ones:
  - `a_paragraph_is_spoken_sentence_by_sentence_as_it_streams` replaces
    `a_paragraph_is_spoken_when_it_closes_not_at_turn_end`. "First one. Sti" →
    Speak 0; "ll first. La" → Speak 1; "st" → nothing; `TurnFinished` → Speak 2
    "Last.". The ledger holds each sentence once. A paragraph that commits
    mid-delta speaks only its unspoken rest.
  - (a) `three_char_deltas_release_each_sentence_at_its_boundary`: each
    sentence is released in the delta that carries the whitespace after it,
    and not one delta earlier.
  - (b) `abbreviations_and_decimals_do_not_split`: "Dr. Smith measured 3.5 s,
    e.g. on the second run." comes out as one sentence. The known misses are
    pinned.
  - (c) `blocks_that_are_not_prose_are_not_streamed`: ordered and bullet
    lists, a heading, a quote and a `|` table stream nothing and are spoken at
    the close. A paragraph that a list interrupts streams, and the list waits.
    Half-arrived `**bold.` is held until it balances.
  - (d) `barge_in_mid_stream_keeps_heard_sentences_and_drops_the_rest`: with
    two sentences streamed and the player on the second, `commit` gives
    `Cancel(Some("One."))`, `Interrupted { heard: 1, dropped: 1 }` and
    sentence 1 `Dropped`. The cancelled turn's later deltas speak nothing, and
    its `TurnFinished` is silent.
  - Changed for the new timing: the `speaking()` helper and
    `vad_threshold_follows_the_state` collect `Speak` from the delta step too,
    since complete sentences now stream there.
- Other gates: see §5.

## 4. Latency evidence

The headless release smoke (brief step 4) **was not run**. The worktree had
no release target. A cold `cargo build -p theuth --bin t --release` at
machine load average ~40 (17:45, several parallel waves) would take well over
the 30-minute budget. The unit evidence stands in for it. Before this change,
a one-paragraph reply emitted its first `Speak` at `TurnFinished` (live log
11:05: `State(Thinking)` at :22.6, `Speak{0,1,2}` within 1 ms at :26.5, so
first sentence 3.9 s after the turn began). After it, the first `Speak` comes
in the delta that completes the first sentence (tests (a) and the paragraph
test). That is one sentence of model output after the first token, not the
whole reply. The two INFO lines above will give the real numbers on the next
live talk: `grep "talk: first" ~/.theuth/logs/theuth.log.<date>`.

## 5. Gate results

Run 2026-09-29 17:40–17:53 in this worktree, machine load average about 40.

- `cargo test -p talk`: 20 passed, 0 failed.
- `cargo test -p voice`: 25 passed, 0 failed. `crates/voice` was not changed.
- `cargo clippy -p talk -p voice --all-targets`: no warnings.
- `cargo check --workspace --tests`: clean. The only output was cargo's
  future-incompatibility note for the dependency `block v0.1.6`, which was
  already there before this change. For `crates/nodes` to compile, the
  worktree needs the gitignored `bindings/methods.json`. It was copied from
  the main checkout.
- The release build and headless smoke (brief step 4) were not run; see §4.
