# 76 as-built — TALK-FIX-1 (2026-09-29)

Follow-ups from Sky's first real use of design 76 (brief
`/tmp/talk76/omp-TALK-FIX-1.md` + orchestrator item 4). Branch
`fix/talk-sensitivity-card-icon` on `main` @ 4d11125a.

## 1. Sensitivity — noise never commits a barge-in (`crates/talk`, `crates/voice`)

Root cause (`policy.rs decide()`): `None => None` — a held candidate still
open with no text at the `decide_ms` (900 ms) deadline was the §4.4 "treated
as real" fallback. Steady noise (an aquarium) keeps TEN-VAD open past
`barge_in_min_ms` and never yields a transcript, so the fallback cut the reply.

| Change | Where |
|---|---|
| No word ⇒ never a cut-in. `decide()`: `None => Resume(Noise)` for both the closed-empty and the open-at-deadline case; the candidate utterance is dismissed as before (`discard`), state → `Speaking`. `ResumeReason::Noise` added (`lib.rs`; ts binding regenerated). | `policy.rs decide`, `lib.rs ResumeReason` |
| Word rule. `Policy::classify(text) -> Option<ResumeReason>` replaces `is_backchannel`: all tokens listed → `Backchannel`; fuzzy prefix of the ledger → `Echo`; no non-backchannel token of ≥ 2 chars after punctuation strip (`"I"`, `"a"`, `"."`, `"yeah a"`) → `Noise`; else real. A wordless *partial* does not settle a hold (`text()` computes `wordless` before storing `c.text`); the next partial or the deadline does. | `policy.rs classify/text` |
| Hysteresis. `talk.barge_in_vad: f32` (default 0.7). `Policy::set()` emits `Action::VadThreshold(f32)` whenever the state's threshold changes: `Speaking`/`Holding` → `barge_in_vad`, everything else → `voice::vad::Gate::THRESHOLD` (0.5). The actor stores it in `Live.vad_threshold: Arc<AtomicU32>` (f32 bits); the blocking audio `Task` reads it per 20 ms chunk and applies `Utterer::set_vad_threshold` when it differs from `applied`, and resets to `THRESHOLD` before the lease goes back. | `policy.rs set/threshold`, `lib.rs run/Task` |
| `voice::vad::Gate`: `threshold: f32` field, manual `Default` (0.5), `pub const THRESHOLD`, `set_threshold()`, `reset()` keeps the threshold, `push()` compares against it. `Gate::new()` and every dictation/meeting path are byte-identical (their tests unchanged and green). `Utterer::set_vad_threshold` passthrough. | `voice/src/vad.rs`, `voice/src/live.rs` |
| Re-arm rule: a candidate is created only in `onset()`, i.e. on a gate *open edge*; after a Noise/Backchannel/Echo resume the gate has to close and reopen before another hold. Documented at the resume site; proven by the new test (below). If the gate never closes, the speech continues unpaused. | `policy.rs decide` |
| Info logs under target `talk` (the default TUI/daemon log is `info`): `holding since_speech_ms=…`, `resumed reason=… text="…"`, `interrupted heard=… dropped=… text="…"`. `Action::Resume { reason, text }` and `Action::Interrupted { heard, dropped, text }` carry the candidate text (`Policy.cut_text` bridges `decide` → `commit`). Per-action `debug!` lines kept; the task logs `talk: vad threshold` at debug when it switches. | `lib.rs run` |
| Docs: 76 §4.4 rewritten (deadline = noise, threshold, word rule, re-arm, log lines), §4.9 `Resumed` reason, §4.10 `decide_ms`/`barge_in_vad`, §5.2 card; `DEFAULT_CONFIG` in `theuth/src/main.rs` gains `barge_in_vad` and corrects the `decide_ms` comment. | |

Tests (`policy.rs`): `classifier_backchannel_echo_real` (now over `classify`,
incl. the Noise cases), `open_and_wordless_at_the_deadline_is_noise_and_needs_a_new_onset`
(Onset → Hold pauses → wordless partial does not settle → Decide fires
`Resume{Noise}` with no `Commit`/`Cancel`/`Trim`/`StopSpeech` → further text
and stale timers do nothing while the gate stays open → Close + Onset holds
again → a real word commits, `Interrupted{text:"no"}`),
`vad_threshold_follows_the_state` (no action Listening→Thinking; `0.7` on
Thinking→Speaking; none across Holding; back to `THRESHOLD` on the commit),
`backchannel_resumes_and_its_final_is_not_text` (closed-empty → `Noise`).
`voice/src/vad.rs`: `threshold_moves_onset_and_close_and_survives_reset`.
`lib.rs`: `config_defaults` checks `barge_in_vad == 0.7`.

Unrelated but blocking the gate: `agent/src/supervisor.rs` — clippy 0.1.96
`type_complexity` (deny-all workspace lints) on `Agents::session`'s `acp`
tuple; a `type AcpSpawn` alias, no behaviour change.

## 2. Spoken-turn card (desktop + TUI)

Desktop: `[spoken …]` is parsed by `parseSpoken()` in `lib/injected.ts`
(beside `parseBroadcast`; `classify()` unchanged — marker inputs stay kind
`user`, the look is chosen at render, which is the existing mechanism) and
rendered by `MessageUser.tsx` exactly the way `[voice note, N s]` is: the
transcript as the bubble body, a quiet caption above it —
`<AudioLines size-3/> spoken · 5 s`, plus ` · cut in` for the interrupted
variant. No separate `SpokenCard` in `HarnessCards.tsx`: the voice-note
caption already lives in `MessageUser` after the attach `parse()`, so the
spoken turn keeps edit/fork/stamp/attachments. `Header.tsx` and
`SessionItem.tsx` strip the marker from titles as they do the note marker.
Tests: `injected.test.ts` `spoken marker` (both variants, voice-note and
plain text → null, `classify` → null); voice-note tests unchanged.

TUI: `attach::spoken(text) -> Option<(secs, interrupted, body)>` and
`attach::spoken_tag()` (regex-free); `ui.rs Entry::User` draws the tag line
(`spoken 5 s` / `spoken 5 s · cut in`, `SAND`) between `◆ you` and the body;
`model.rs` `Entry::height` counts the row and `turns()` (the `/edit n` list)
strips the marker. Tests: `attach::tests::spoken_marker`,
`ui::tests::spoken_user_card_shows_tag_not_marker` (rendered frame contains
the tag and body, not `[spoken`). Left as is: `lib.rs`'s plain-text history
dump prints the raw marker (a textual export).

## 3. Icon

Talk = lucide `AudioLines` everywhere: `AppSidebar.tsx` rail entry,
`Composer.tsx` Talk button, the spoken caption. `Orbit` (DESKTOP-TALK's
choice) removed from both imports; `routes/talk.$session.tsx` has no Talk
icon (title text only). `MicButton` keeps `Mic`; `MeetingBar` keeps its own
`AudioLines`. 76 §0.12 now says so explicitly; `76-as-built-DESKTOP-TALK.md`
corrected (L97, L104) and its `[spoken …]` open item resolved.

## 4. Prompt — a voice in the room (`crates/talk/src/lib.rs PROMPT`)

Sky said "say hello to my friend" and the model reached for `message_user`
and an agent lookup. The `## Live talk` block now opens with: the reply is
synthesised to speech and played on the user's speaker the moment it is
written, so the user and anyone with them hears it aloud; when asked to say
something to a person present, just say it; do not send messages, DM, or
look up agents or contacts for people the user mentions unless explicitly
asked to send a message elsewhere — a voice in the room, not a messenger.
The rest of the block is unchanged. Test
`prompt_says_it_is_heard_aloud_and_not_a_messenger` (`"aloud"`,
`"Do not send messages"`).

## Gates

- `cargo test -p talk -p voice -p tui`: all green (result lines 25 / 16 /
  121 / 1 passed, 0 failed); re-run after rustfmt of the touched files:
  talk + voice libs green; `tui` `spoken` tests green.
- `cargo clippy -p talk -p voice -p tui --all-targets`: zero warnings (after the `AcpSpawn` alias).
- `cargo check --workspace --tests`: clean (only the pre-existing `block v0.1.6` future-incompat note).
- `just ts-desktop`: bindings regenerated (`ResumeReason.ts` gains `"noise"`).
- UI (`crates/desktop/ui`, `bun install --frozen-lockfile` here and in
  `packages/apps`): `bunx tsc --noEmit` clean; `bunx vite build` ok
  (13 547 modules); `CI=1 bunx vitest run`: 116 files, 654 tests passed.

## Live check — not evidence; skipped on Sky's instruction

Two runs were made before the instruction arrived
(`THEUTH_DATA_DIR=/tmp/talk-fix1-data`, `/tmp/talk76/tui_noise.py`: TUI
`/talk`, `say` through the built-in speaker, then 40 s of 4 Hz-modulated
pink noise through the built-in speaker via `ffmpeg -f audiotoolbox`). The
laptop is in clamshell mode — built-in speaker muffled, built-in mic covered;
the BRIO heard the room — so the numbers say nothing about thresholds and are
recorded only to show the log shape. Default log (`info`, no `RUST_LOG`):

```
INFO talk: holding since_speech_ms=8615
INFO talk: resumed reason=Noise text=""
… ×12 during the noise, no `interrupted` …
INFO talk: interrupted heard=1 dropped=8 text="balance"   ← 27 s after the noise ended; source unknown (room)
```

The proof for item 1 is the policy unit tests above.

### How to run the live test (for Sky / the orchestrator)

Setup: a laptop with a real speaker and mic — or headphones + the aquarium.
Debug build is fine (`cargo build -p theuth`, then `./target/debug/t`);
default logging is enough (`info`). Log file: `~/.theuth/logs/theuth.log.<date>`
(the TUI's daemon; the desktop app's engine log is the same file under its
data dir). `grep 'INFO talk' ~/.theuth/logs/theuth.log.$(date +%F)` while
running, or `tail -f` it.

1. Start the TUI, `/talk` (or the desktop Talk entry). Wait for
   `talk: listening` in the status line.
2. Ask for something long: *"Tell me a story about a lighthouse keeper in
   eight short sentences."* — the reply should run 30–60 s.
3. **Noise case.** Say nothing; let the aquarium (or any steady non-speech
   noise: a fan, running water, rustling paper near the mic) run through the
   reply. Expected in the log, possibly several times, the speech pausing
   ≤ 0.5 s each time and never stopping:
   ```
   INFO talk: holding since_speech_ms=…
   INFO talk: resumed reason=Noise text=""
   ```
   No `interrupted` line; the transcript keeps the whole reply. If the noise
   never even holds, the 0.7 threshold filtered it — also a pass.
4. **Real word.** During the next reply say *"Stop."* or *"Wait, no."*
   Expected:
   ```
   INFO talk: holding since_speech_ms=…
   INFO talk: interrupted heard=N dropped=M text="Stop"
   ```
   The speech stops, the transcript keeps only the N heard sentences, the
   next user row is `[spoken, … s, interrupted] Stop …` rendered with the
   `cut in` caption.
5. **Backchannel.** During a reply say *"mhm"* or *"okay"*: `holding` →
   `resumed reason=Backchannel text="Mhm."`, the speech resumes.

Reading a false barge-in, if one still happens: the `interrupted … text="…"`
line names the word the STT produced. (a) It is a word from the room that
nobody said → STT hallucination on noise; raise `talk.barge_in_vad`
(0.7 → 0.8) in `config.yaml` so the gate does not open on it, or add the word
to `talk.backchannels` if it is a filler. (b) It is a word from the agent's
own sentence (compare with the `Saying` text — `RUST_LOG=talk=debug` shows
`Saying {…}`) → echo not cancelled and not caught by the prefix rule (§4.4
step 3 only matches a *prefix* of the recent sentences); headphones, or
`talk.echo_ratio` down to 0.7, or the §12 AEC follow-up. (c) `resumed
reason=Noise` lines every ~1 s with the speech stuttering → the noise sits
right at the threshold: raise `barge_in_vad`, or `barge_in_min_ms` 400 → 600.
Nothing in the transcript is lost in (c); only in (a)/(b).

## Files

`crates/talk/src/{lib.rs,policy.rs}`, `crates/voice/src/{vad.rs,live.rs}`,
`crates/theuth/src/main.rs`, `crates/agent/src/supervisor.rs`,
`crates/tui/src/{attach.rs,ui.rs,model.rs}`,
`crates/desktop/ui/src/lib/{injected.ts,injected.test.ts}`,
`crates/desktop/ui/src/components/chat/{MessageUser.tsx,Composer.tsx,Header.tsx}`,
`crates/desktop/ui/src/components/sidebar/{AppSidebar.tsx,SessionItem.tsx}`,
`bindings/ResumeReason.ts` (+ the desktop copy; regenerated by `just ts-desktop`, gitignored),
`docs/design/{76-talk.md,76-as-built-DESKTOP-TALK.md,76-as-built-FIX-1.md}`.
