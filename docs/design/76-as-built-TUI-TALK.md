# 76 — as built: TUI-TALK (the `/talk` view in the terminal)

Status: landed 2026-09-28 on `talk/tui` (base `feature/talk` @ 763f8bfb).
Built against the TALK-CONTRACT surface (`docs/design/76-as-built-TALK-CONTRACT.md`);
the engine (`crates/talk`) is still the stub on this branch, so the view was
developed and smoked against a scripted event source (§Verified).

## What was built

All inside `crates/tui/`.

### `src/talk.rs` (new, pure — the `dictation.rs` shape)

| Item | What |
|---|---|
| `Look { Idle, Thinking, Speaking }` + `look(State) -> Look` | the state→orb mapping of 76 §0.12: `Idle`, `Listening`, `Interrupted` → `Idle`; `Thinking` → `Thinking`; `Speaking`, `Holding` → `Speaking` |
| `Talking { id, session, state, level, agent: Option<(SpeechId, String)>, user: Option<String>, degraded, held_at }` | the talk this view started |
| `Talking::apply(TalkEvent, frame) -> Option<EndReason>` | the subtitle reducer (§5.2): `Heard` (partial and final) → `user`; `Submitted` → `user = None`; `Saying` → `agent`; `Interrupted` → `agent = None`; `Level` → `level = max(rms)`; `Degraded` → caption on; `TalkState{Holding}` records the frame so the speaking look freezes; other ids ignored; `TalkFinished` returns the reason |
| `Talking::speech_finished(SpeechId)` | clears `agent` when the `VoiceEvent::SpeechFinished` is for the sentence being shown |
| `Talking::orb_frame(frame)` | `held_at` while `Holding`, else `frame` |
| `radius(width, height, reserve)` | orb radius for the pane: `min((height - reserve - 1) / 2, (width - 1) / 4)` clamped to `1..=MAX_RADIUS` (11 → 23 rows tall at most) |
| `orb(look, frame, level, r) -> Vec<String>` | `2r+1` rows × `4r+1` cells (cells are ~2:1, so x is halved). Idle: a ring breathing between 0.62 and 0.86 r over 4 s (`•` core, `·` fringe). Thinking: a disc of `░▒▓` whose three-armed texture rotates and pulses, `·` edge. Speaking: a `●` core plus three rings running outward every ⅔ s; the level stretches the core and how far the rings reach, so the orb moves with the voice |
| `DEGRADED` | `"no echo cancellation; headphones recommended"` |
| `sim(id, session) -> Vec<(delay_ms, UiEvent)>` | the scripted source: `TalkStarted`, `Degraded`, then three rounds of `Heard` partial → final → `Submitted` → `Thinking` → `Speaking` + `Saying` + `Level` swells → `SpeechFinished` → second sentence → `Holding` → `Interrupted` → `Submitted{interrupted}` → `Thinking` → `Speaking` → `Listening` |

### `src/model.rs`

- `Command::TalkStart(Option<SessionId>)`, `Command::TalkStop(TalkId)`;
  `UiEvent::TalkStarting(TalkId)`, `UiEvent::TalkFailed(String)`;
  `UiEvent::Talk` now handled.
- `Model { talk: Option<Talking>, talk_view: bool, talk_pending: bool }`.
- `/talk` in `COMMANDS` (beside `/dictate`) → `toggle_talk(false)`;
  `toggle_talk(stop)`: no talk → `TalkStart(self.session)` (a `None` session is
  created by the command task first); talk running, view closed → re-enter
  the view; view open, or `stop` (Ctrl+T) → `TalkStop`; reply pending → no-op.
- `apply`: `TalkStarting` → `Talking::new`, view opens; `TalkFailed` → `Error`
  entry; `Talk(ev)` → `apply_talk` (reducer; `TalkFinished` drops the talk,
  closes the view, reports `Error{message}`); `Voice(SpeechFinished{id})` →
  `talk.speech_finished(id)`.
- `input_focused()` is false and `animating()` is true while `talk_view`.
- `status()` gains `  ·  ● talk: <state> (/talk)` while a talk runs with the
  view closed (the brief's indicator).

### `src/ui.rs`

- Keys: in the view (and no approval prompt on top) Esc → `talk_view = false`,
  Ctrl+T → `toggle_talk(true)`, everything else swallowed. Ctrl+T outside the
  view (editor idle, no menu/secret) → `toggle_talk(true)`: starts a talk
  when none runs, stops the running one otherwise.
- `talk_pane(&Talking, frame, width, height)`: replaces the transcript in the
  content area. Column, centred: the orb rows (`Text`, `NoWrap`), then the
  agent line and the user line — each a `height: 2`, `overflow: Hidden`,
  `justify_content: FlexEnd` box around a wrapped, centred `Text`, so a long
  line clips at its head and the tail stays — then the one-row caption.
  Colours are the palette's only: `OCHRE` (accent) speaking, `SAND` (muted)
  idle and the user line and the caption, the terminal's own foreground
  (no colour) thinking and the agent line.
- Header row shows `● talk  (Esc leaves · Ctrl+T stops)`; the editor box drops
  its border and `min_height` and hides the `TextInput` (still mounted) while
  the view is open.

### `src/lib.rs`

- `pub mod talk` (the extern crate is `::talk::` in this file).
- `Command::TalkStart(session)` → `start_talk`: with `None`,
  `create_session(CreateSessionOptions::default())` and `UiEvent::SessionOpened`
  first, then `api.talk_start(session, Sink::Device, None)` →
  `TalkStarting(id)` | `TalkFailed(reason)`. `Command::TalkStop(id)` →
  `api.talk_stop(id)`.
- `THEUTH_TALK_SIM` (any value): `start_talk` spawns `run_talk_sim` feeding
  `talk::sim` into the UI channel at its own pace instead of calling the
  engine; `TalkStop` answers with a synthetic `TalkFinished{Stopped}`.
- `print_event`: `TalkStarting` silent, `TalkFailed` → `talk: <m>`.

### Tests

- `talk::tests::six_states_three_looks` — the mapping.
- `talk::tests::subtitle_reducer` — partial → final → cleared on `Submitted`;
  agent line cleared by its own `SpeechFinished` only and by `Interrupted`;
  stranger ids ignored; level = max; `Holding` freezes `orb_frame`.
- `talk::tests::orb_frames_are_rectangular_and_alive` — every look at
  `r = 1..=11` is a `2r+1 × 4r+1` block with ink; speaking grows with the
  level; idle and thinking change between frames.
- `talk::tests::radius_fits_the_pane` — `80×24 → 8`, `120×40 → 11`, tiny → 1.
- `model::tests::talk_lifecycle` — `/talk` with no session → `TalkStart(None)`,
  second press waits, `TalkStarting` opens the view (editor unfocused,
  animating), status indicator only while the view is closed, `/talk`
  re-enters, Ctrl+T stops, a stranger's `TalkFinished` is ignored, ours
  closes the view and an `Error` reason lands as an entry, `TalkFailed`
  clears the pending flag.
- `ui::tests::talk_pane_is_centred_at_both_sizes` — `Listening`, `Thinking`,
  `Speaking`, `Holding` at 80×24 and 120×40 render into a string without
  panicking, fit the height, every orb row's centre is within ±1 cell of
  `width / 2`, and both subtitles and the caption are present.

## Verified

Toolchain per the brief; all in this worktree.

- `cargo check -p tui --tests` → `Finished dev profile … in 2m 07s`, no warnings.
- `cargo test -p tui` → `test result: ok. 116 passed; 0 failed` (lib, incl. the six
  above), `ok. 1 passed` (doc/integration), `ok. 0 passed`.
- `cargo clippy -p tui --all-targets` → `Finished dev profile …`, no warnings.
- `cargo fmt -p tui -- --check` → clean.
- **Scripted captures** (the render test at 80×24 and 120×40; a temporary
  `println!` in the test, removed afterwards). 80×24, `Listening` (idle ring,
  `SAND`), `Thinking` (foreground texture), `Speaking` at level 0.8 (`OCHRE`),
  `Holding` (speaking look frozen at its first frame):

```
                                 ···•••••••••···
                              ··••••·········••••··
                            ··•••··           ··•••··
                           ·•••·                 ·•••·
                          ·••··                   ··••·
                         ·••··                     ··••·
                         ·••·                       ·••·
                         ·••·                       ·••·
                         ·••·                       ·••·
                         ·••··                     ··••·
                          ·••··                   ··••·
                           ·•••·                 ·•••·
                            ··•••··           ··•••··
                              ··••••·········••••··
                                 ···•••••••••···



            Berlin is mild and cloudy today, around fourteen degrees.


                               what about tomorrow
                  no echo cancellation; headphones recommended
```

```
                                    ·········
                                ···    ░░▒▒▓▓▓···
                              ··      ░░░▒▒▓▓▓▓▓▓··
                            ··░   ░░░▒▒▒▒░░░▒▒▓▓▓▓▓··
                           ··▒▒░░░▒▒▓▓▓▓▒░   ░░▒▒▒▒░··
                          ··▓▓▒▒▒░▒▒▓▓▓▓▒    ░░░░░░░ ··
                          ··▓▓▓▒░░ ░░▒▒▒▒░░░▒▒▒▒▒░░  ··
                          ·▓▓▓▓▒░     ░▒ ░▒▓▓▓▓▓▒░    ·
                          ··▓▓▓▒░░░░░▒▒▒░░░░▒▓▓▒▒░   ··
                          ··▓▒▒▒▒▒▒▒▓▓▓▓░    ░░▒░░░  ··
                           ··▒░░░░▒▒▓▓▓▒░░   ░░▒▒▒░░··
                            ··░    ░░▒▒▒░░░░▒▒▓▓▓▓▒··
                              ··     ░░░▒▒▒▓▓▓▓▓▓··
                                ···   ░░▒▒▓▓▓▓···
                                    ·········
```

```
                                     ·······
                                 ·····     ·····
                               ···             ···
                             ···    ●●●●●●●●●    ···
                             ··   ●●●  ●●●  ●●●   ··
                            ··   ●●  ●●●●●●●  ●●   ··
                            ··   ●● ●●●●●●●●● ●●   ··
                            ··   ●●  ●●●●●●●  ●●   ··
                             ··   ●●●  ●●●  ●●●   ··
                             ···    ●●●●●●●●●    ···
                               ···             ···
                                 ·····     ·····
                                     ·······
```

```
                                    ·········
                                 ····       ····
                               ···             ···
                              ···   ●●●●●●●●●   ···
                              ··   ●●●●●●●●●●●   ··
                              ··   ●●●●●●●●●●●   ··
                              ··   ●●●●●●●●●●●   ··
                              ···   ●●●●●●●●●   ···
                               ···             ···
                                 ····       ····
                                    ·········
```

  At 120×40 the orb is the 23-row `MAX_RADIUS` block, same shapes (in the
  test output, not repeated here).

- **Live smoke with the scripted source** (no TALK-CORE engine on
  `feature/talk` at finish — `git diff feature/talk talk/core -- crates/talk`
  is empty, `crates/talk` is the stub). Fresh data dir per the brief plus
  `THEUTH_SECRETS__KEY_STORE=file` (see Not verified), then
  `THEUTH_TALK_SIM=1 THEUTH_DATA_DIR=/tmp/talk-TUI-TALK-data THEUTH_RPC__LISTEN=127.0.0.1:7793 ./target/debug/t`
  (`cargo build -p theuth` → `Finished dev profile …`) under a 100×30 pty
  driven by a `pyte` screen: typed `/talk` ⏎ with no session, waited, pressed
  Esc, typed `/talk` ⏎, pressed Ctrl+T. Screens seen (trimmed):

```
                                              ·········
                                          ·····       ·····
                                        ···               ···
                                       ···   ●●●●●●●●●●●   ···
                                      ···   ●●   ●●●   ●●   ···
                                      ··   ●●  ●●●●●●●  ●●   ··
                                      ··   ●● ●●●●●●●●● ●●   ··
                                      ··   ●●  ●●●●●●●  ●●   ··
                                      ···   ●●   ●●●   ●●   ···
                                       ···   ●●●●●●●●●●●   ···
                                        ···               ···
                                          ·····       ·····
                                              ·········

                      Berlin is mild and cloudy today, around fourteen degrees.

                            no echo cancellation; headphones recommended

 ● talk  (Esc leaves · Ctrl+T stops)
 a415b18a  idle
```

  (speaking, session `a415b18a` created by `/talk`), the idle ring with the
  user partial `what's the weather` / `actually what about` under it while
  listening, the thinking texture between `Submitted` and `Speaking`, and after
  Esc the ordinary session view with the status row
  `a415b18a  idle  ·  ● talk: speaking (/talk)`; `/talk` again reopened the
  view (orb still animating); Ctrl+T left it and the status row read
  `a415b18a  idle`. Every state of the script rendered; no panic, no error
  entry. Also confirmed by that run: a talk started with no session creates
  one first (`SessionOpened` then `TalkStarting`).
- `NO_COLOR`: iocraft honours it globally (`iocraft/src/color.rs:133-149`);
  the three looks differ by glyph (ring / `░▒▓` texture / `●` core and rings),
  so the state reads without colour — the captures above are colourless text.
  The TUI has one palette (dark, `ui.rs:49-51`); there is no light theme to check.

## Not verified

- Against the real engine: `TalkState`/`Saying`/`Heard` timing, `Level` from a
  microphone, the `SpeechFinished` clear on the device sink (76 says
  `SpeechFinished{Done}` may precede the end of playback — the contract still
  says "cleared on that id's `SpeechFinished`", implemented as written).
- A first live run of a rebuilt binary against a data dir whose vault key is in
  the macOS keychain blocks in `secrets::key::keychain` (Keychain access prompt
  for the re-signed binary, `sample` showed the stack). Not talk's; the smoke
  used a fresh dir and `THEUTH_SECRETS__KEY_STORE=file`.
- The approval-prompt-on-top path in the view (keys fall through to the prompt)
  is by code reading only.

## Deviations

- **`/talk` semantics** (brief §Goal 1 says both "`/talk` again … stops" and
  "re-enter with `/talk`"): `/talk` = start (no talk) / re-enter (talk on,
  view closed) / stop and leave (view open); Ctrl+T = stop from anywhere,
  start when none. So a talk left with Esc is re-entered by `/talk`, stopped
  by `/talk` in the view or Ctrl+T.
- **No session → `/talk` creates one** (the desktop's Talk entry does the same,
  76 §0.12). The brief said "on the current session"; with the TUI's lazy
  session there is often none.
- **Orb frames are computed per tick, not precomputed**: ≤ 23×45 cells, a few
  thousand float ops at 60 Hz; marked `ponytail:` in `orb`. The frozen
  `Holding` look is the frame recorded at the transition, not a special frame.
- **`--talk-sim` is an env var** (`THEUTH_TALK_SIM`) read in `crates/tui`, not a
  hidden clap flag: the flag would live in `crates/theuth/src/main.rs`, outside
  the allow-list.
- **Composer while in the view**: hidden (border, min-height and `TextInput`
  off) rather than shown read-only. `display: None` on the bordered view is
  not an option — iocraft draws the border of a zero-width view and panics
  with `attempt to subtract with overflow` (`iocraft/src/components/view.rs:283`);
  found in the first live run, fixed by dropping the border instead. Comment
  in `ui.rs` says so.
- **Esc in the view leaves it; it does not cancel the agent's turn** (76 §5.2
  "Esc during Speaking is the agent's cancel"): the brief's TUI rule wins.
- Subtitle tail-clamp is layout (`FlexEnd` in a two-row hidden-overflow box,
  the transcript's own trick), not a text function; no `wrap`/clamp helper added.

## Outside the allow-list

None. (`crates/tui/**` and this doc only.)

## Follow-ups

- Smoke against TALK-CORE when it lands: `/talk`, speak, barge in; check that
  `SpeechFinished` on the device sink clears the agent line at the right moment
  and that `Level` keeps the speaking rings moving.
- If `Holding` should keep a hint of motion (76: "the player is paused, not
  stopped"), pulse the core without advancing the rings — one line in
  `speaking`.
- `Model::submit("/talk")` while `awaiting_session` (a message just sent to a
  brand-new session) would create a second session; wait for `SessionCreated`
  first if that ever bites.
