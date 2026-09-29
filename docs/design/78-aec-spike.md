# 78 — Software echo cancellation on Windows and Linux (spike report)

Status: research spike, 2026-09-29. Docs only; no code changed. Follow-up to
76 §0.5 and §10 step 9 ("Software AEC spike (decides §0.5's slot)").
Every external fact below has a source. Claims marked **(unverified)** were not
checked against source or a build during this spike.

## 0. Answer

**Use `sonora`** ([crates.io](https://crates.io/crates/sonora),
[repo](https://github.com/dignifiedquire/sonora)): a pure-Rust port of WebRTC's
AudioProcessing module (AEC3 + NS + AGC2), BSD-3-Clause. Tap the far end in
`voice::out::Ring::fill`, which is the speaker callback. Run the canceller in
`voice::mic` as a new `Aec::Software`, at 48 kHz on 10 ms frames, before
VAD/STT. Keep `tonarino/webrtc-audio-processing`, the C++ WebRTC build, as the
A/B reference in the PoC and as the fallback.

Why this one and not the older, better-known binding: tonarino's crate **does
not build on Windows MSVC in any release**. The fix is an open, unmerged PR
([#102](https://github.com/tonarino/webrtc-audio-processing/pull/102), opened
2026-08-08). On Linux its `bundled` feature also needs meson, ninja and a C++
compiler on every build host. `sonora` is the same WebRTC algorithm. It has no
C/C++ toolchain step, runs Windows x86_64 build and test in CI, uses the API
shape our seams already have (f32, deinterleaved, 10 ms), and exposes ERLE and
delay statistics, which the PoC can use directly. Its risk is youth: the port
was made in 2026, the README says it was AI-assisted, and it has one main
maintainer. The PoC below checks it against the C++ build before anything
lands.

## 1. The seams in theuth today (read from the tree)

| What | Where | Format / rate | Notes |
|---|---|---|---|
| Near end (mic) | `crates/voice/src/mic.rs` — `Capture::open_with(Aec)`; the thread `run()` downmixes and sends `Chunk = Result<Vec<f32>,String>` over `std::sync::mpsc` | mono f32, **48 kHz** (`RATE`), **960-sample / 20 ms chunks** (`CHUNK`) | macOS `Aec::Vpio` wraps VoiceProcessingIO; elsewhere `Aec::None`. `flexaudio-mic` 0.2 **throws away `cpal::InputCallbackInfo`** (`flexaudio-mic-0.2.0/src/lib.rs:415`), so the capture timestamp is lost before our code sees it. |
| Consumer of near end | `crates/talk/src/lib.rs` `Task::run` → `Task::chunk` (L1119–1180) → `voice::live::Utterer::chunk` | 48 kHz in; `Utterer` resamples to 16 kHz (rubato `FftFixedIn`), then TEN-VAD gate + Nemotron | Mic opened at `StartTalk` (L762–803): `AecMode::Prefer → Aec::Vpio`, else `Aec::None`. |
| Far end (speaker) | `crates/voice/src/out.rs` — `Player::write` (24 kHz → device rate via rubato, producer side) → `Ring::push`; **`Ring::fill(out, channels, latency)` is the cpal output callback** (L174–203) | TTS writes mono f32 at **24 kHz**; the ring and the callback run at the **device rate** (24 kHz if the device accepts it, else its default, typically 48 kHz); mono copied to every channel | `fill` writes the exact samples the device gets, zeros included while paused or starved. That makes it the right tap: after flush/pause, at device rate. |
| Playback latency known | `Ring::fill` stores `ts.playback − ts.callback` per callback; `position_dev()` = read + silence − latency; default 40 ms | samples at device rate | Per the module doc (cpal 0.16 source): CoreAudio/WASAPI report **one callback buffer** (an estimate, not the device's latency); ALSA reports `snd_pcm_delay`. The player **does** report position (`Player::position() → Played{id, seq, samples}`). |
| Player lifetime | `crates/voice/src/lib.rs:621–631` `Voice::player()` opens it on first `Speak` and keeps the `Arc<Player>` | — | Opened by the `voice` actor, not by `talk`. The mic can be open before any speaker exists, so the canceller must accept "no reference yet". |
| Degradation reporting | `talk/src/lib.rs:868–875` publishes `TalkEvent::Degraded{MicNoAec}` when `sink == Device && aec == Aec::None`; the meeting has `Degradation::MicNoAec` + `Support::Partial("no echo cancellation; headphones recommended")` (`meeting/src/capture/system.rs:113`) | — | UI strings: `desktop/ui/src/components/meeting/Island.tsx:39`, `tui/src/talk.rs:31`. |
| Echo protection today | `talk/src/policy.rs:452` uses `barge_in_vad` (0.7) while `Speaking`/`Holding`; text-domain echo rejection (Levenshtein ≥ `echo_ratio` 0.8 against the last ~2 s of the ledger) | — | Stays as the backstop whatever AEC we add. |
| Audio crates already in `Cargo.lock` | cpal 0.16.0, flexaudio 0.2.0 (+ `-mic`, `-os-windows`, `-os-linux`), rubato 0.16.2 and 3.0.0, pipewire 0.10.1, alsa 0.9.1, `windows` 0.51–0.62, **rtrb 0.3.5, ringbuf 0.4.8** (lock-free SPSC for the reference tap), cc, cmake, pkg-config, bindgen | — | No AEC crate yet. Neither cpal 0.16 nor flexaudio 0.2 sets a WASAPI audio category: `grep AudioCategory\|Communications` over both sources finds nothing. |

## 2. Candidates

Popularity was read from the crates.io and GitHub APIs on 2026-09-29.
"Recent" = crates.io's 90-day download count.

| # | Candidate | What it is | Popularity / upkeep | License | Platforms & build friction | API fit | Delay handling |
|---|---|---|---|---|---|---|---|
| 1 | **`sonora` 0.2.0** | Pure-Rust port of WebRTC APM (M145): AEC3, NS, AGC2, HPF; SIMD (SSE2/AVX2/NEON) with scalar fallback ([README](https://github.com/dignifiedquire/sonora)) | 394 k downloads (391 k recent, almost all on 0.2.0 from 2026-07-29), 85★, pushed 2026-09-29; 0.1.0 released 2026-02-11. **Only 3 reverse deps** on crates.io (moq-audio, mediaway-sw, sonora-ffi), so read the download count as a weak signal (unexplained). | BSD-3-Clause | CI: Linux x86_64, macOS arm64, **Windows x86_64 build + test**, Android, iOS. **No C/C++ step.** MSRV 1.91 (repo is on `stable`, edition 2024). Default deps: its own sub-crates + `tracing`; cpal/hound only behind `examples`. | `AudioProcessing::builder().config(Config{echo_canceller: Some(..)}).capture_config(StreamConfig::new(48000,1)).render_config(..)`; `process_render_f32(&[&[f32]], &mut ..)` / `process_capture_f32(..)` on **10 ms deinterleaved f32**; `set_stream_delay_ms` (clamped 0–500); `statistics()` → ERLE, ERL, `delay_median_ms`, `delay_standard_deviation_ms`, divergent-filter fraction, residual-echo likelihood ([docs](https://docs.rs/sonora/latest/sonora/struct.AudioProcessing.html), [stats](https://docs.rs/sonora/latest/sonora/stats/struct.AudioProcessingStats.html)) | AEC3's own matched-filter delay estimator (`sonora-aec3` `Delay`: 5 correlator filters, down-sampling 4, default delay 5 blocks before convergence, optional external estimate: [docs](https://docs.rs/sonora-aec3/latest/sonora_aec3/config/struct.Delay.html)). The README claims the full WebRTC C++ test suite (2400+ tests) passes against the port **(unverified)**. Its benchmark: 13.3 µs per 10 ms frame at 48 kHz mono, full pipeline, M4 Max. |
| 2 | **`webrtc-audio-processing` 2.1.0** (tonarino) | Rust wrapper of PulseAudio's repackaging of the C++ WebRTC APM ([README](https://github.com/tonarino/webrtc-audio-processing)) | 135 k downloads (71 k recent), 330★, pushed 2026-07-16, releases since 2019 (2.0.0 2026-02-10, 2.1.0 2026-05-13). Used in tonari's product. | BSD-3-Clause (upstream WebRTC) | Default: **dynamic link to the system library** (`libwebrtc-audio-processing-dev`). Ubuntu 24.04's package is the 1.x series, not the 2.x this crate wraps **(unverified)**. `bundled` feature builds the C++ with **clang/gcc + pkg-config + meson + ninja**. **Windows MSVC fails today**: GCC flags passed to `cl`, `/std:c++20` needed, COFF symbol prefixing breaks (PR [#102](https://github.com/tonarino/webrtc-audio-processing/pull/102), open, unmerged; maintainers "don't really have a development Windows machine", [#34](https://github.com/tonarino/webrtc-audio-processing/issues/34)). docs.rs fails to build every 2.x release. | `Processor` with `process_capture_frame` / `process_render_frame` on 10 ms frames, config structs in `webrtc-audio-processing-config`. Same concepts as sonora. | Same AEC3 (C++). |
| 3 | `aec3` 0.4.0 (RubyBit/aec3-rs) | A second pure-Rust AEC3 port, plus a typed processing-graph runtime ([README](https://github.com/RubyBit/aec3-rs)) | 39 k downloads (23 k recent), 60★, pushed 2026-09-17; 1 reverse dep | MIT OR BSD-3-Clause (crates.io; GitHub says NOASSERTION) | Pure Rust | The graph API is "a work in progress and the API is expected to evolve"; `aec3::audio_processing` is also usable without the graph. | AEC3 |
| 4 | `libwebrtc` 0.3.50 (LiveKit rust-sdks) | Safe bindings to the full Google libwebrtc; `native::apm::AudioProcessingModule` ([docs](https://docs.rs/libwebrtc/latest/libwebrtc/native/apm/struct.AudioProcessingModule.html)) | 3.0 M downloads (1.2 M recent), 492★ (monorepo), very active | Apache-2.0 + BSD (WebRTC) | Links a **prebuilt libwebrtc** fetched at build time: the whole WebRTC stack for one module **(size unverified)**. | `new(echo, gain, hpf, ns: bool)`; `process_stream` / `process_reverse_stream` on **`&mut [i16]` only**; `set_stream_delay_ms`. No config and no stats. | AEC3 |
| 5 | `aec-rs` 1.0.0 (thewh1teagle) | speexdsp's MDF echo canceller + preprocessor, C built with `cc` ([README](https://github.com/thewh1teagle/aec-rs)) | 11 k downloads (2.6 k recent), 101★, last release 2024-12-08, last push 2025-04 | MIT (speexdsp BSD) | Windows x86/ARM64, Linux, macOS, etc. (claimed); low friction | Simple frame API | **No delay estimator**: the reference must already be aligned within the filter tail. Older NLMS-style canceller with a weaker residual suppressor than AEC3 **(quality comparison from general knowledge, unverified here)**. |
| 6 | `speexdsp` 0.1.2 / `speexdsp-sys` (rust-av) | Raw bindings to system speexdsp | 4.8 k downloads, 26★, last release 2022 | MIT | Needs the system lib (pkg-config); awkward on Windows | Low-level | As 5 |

Not echo cancellers, so out of scope: `nnnoiseless` (RNNoise denoise only)
and `audiopus`.

### OS-provided routes

**Windows: communications-category capture + the endpoint's AEC effect.** A
capture stream in the Communications category gets the AEC effect (APO) only
if the endpoint's driver provides one. `IAcousticEchoCancellationControl` lets
the app pick the render endpoint used as the reference. It needs Windows build
22621, and `GetService` returns `E_NOINTERFACE` when the endpoint's AEC does
not allow that control, or when there is no AEC
([Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nn-audioclient-iacousticechocancellationcontrol),
[sample](https://github.com/microsoft/Windows-classic-samples/blob/main/Samples/AcousticEchoCancellation/README.md),
[APO AEC framework](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/windows-11-apis-for-audio-processing-objects)).
The `wasapi` crate 0.24 wraps it
([docs](https://docs.rs/wasapi/latest/wasapi/struct.AcousticEchoCancellationControl.html)).
Neither cpal 0.16 nor flexaudio 0.2 sets the category, so we would need our
own WASAPI capture path (the `wasapi` crate, or the `windows` crate that
flexaudio-os-windows already uses). Verdict: **not guaranteed on a given
machine**, because it depends on the driver/OEM (and possibly Windows 11
"Voice Clarity" on some hardware, **unverified**). Worth a later probe as an
opportunistic `Aec::System`, but it cannot be the default.

**Windows: the Voice Capture DSP** (`CLSID_CWMAudioAEC`, a DMO). In-process on
any Windows: AEC, mic-array processing, NS, AGC, VAD. In source mode it opens
both the capture and render devices itself
([Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/medfound/voicecapturedmo)).
Always available, but it is a legacy DMO (IMediaObject COM), it takes over
device I/O from cpal/flexaudio, and its quality is dated **(unverified)**.
Not recommended.

**Linux: PipeWire `libpipewire-module-echo-cancel`.** It creates a virtual
`echo-cancel-capture` source and an `echo-cancel-playback` sink, WebRTC-backed
through `aec/libspa-aec-webrtc`
([docs](https://docs.pipewire.org/page_module_echo_cancel.html)). It is
normally loaded by **user config** (`~/.config/pipewire/pipewire.conf.d/*.conf`,
no root), or at runtime through pipewire-pulse as
`pactl load-module module-echo-cancel aec_method=webrtc source_master=… sink_master=…`
([docs](https://docs.pipewire.org/page_pulse_module_echo_cancel.html)). It
cancels only audio routed **through its sink**. Our Player would have to open
that sink and our mic that source; if the user sets them as defaults, cpal's
ALSA→pipewire-alsa default path picks them up with no code change. Whether the
distro ships the webrtc AEC SPA plugin by default is **unverified** per
distro. PulseAudio's `module-echo-cancel` has the same shape. Verdict: a good
documented manual option ("Linux users on PipeWire can load echo-cancel"), and
a possible later auto-probe. It is session-global, so an app loading it
silently is intrusive, and it does not exist on PulseAudio-less or JACK
setups. Not the default.

## 3. Ranking

1. **sonora**. Same algorithm as the gold standard, zero build friction on all
   three OSes, f32 10 ms API that matches our chunks, built-in ERLE and delay
   statistics. Risk: young, AI-assisted port, one main maintainer (§6).
2. **webrtc-audio-processing (tonarino)**. The battle-tested C++ AEC3. It
   ranks second only because Windows MSVC does not build (PR #102) and
   `bundled` adds meson/ninja/clang to every build host and to the Linux
   Dockerfile (knowledge: `facts/theuth-linux-build-deps-docker-image-drift-2026-09-23`
   shows how that drifts). It becomes first if #102 merges and a sonora defect
   turns up. The API is close enough that swapping is a day, not a rewrite.
3. **aec3 (RubyBit)**. Same idea as sonora, less stable API, smaller.
4. **LiveKit libwebrtc**. Works everywhere, but pulls the whole WebRTC for one
   module, i16 only, no stats.
5. **aec-rs / speexdsp**. Builds anywhere, but a weaker algorithm with no delay
   estimation. Our unknown output latency is exactly the case it handles
   worst.
6. **OS routes**. Windows AEC APO (driver-dependent) and PipeWire echo-cancel
   (user config): document and maybe probe later, never rely on.

## 4. Integration sketch

```
TTS 24 kHz ─► Player::write ─(rubato)─► Ring ─► Ring::fill (cpal callback, device rate)
                                                 │  exact samples sent to the device
                                                 └─► reference tap: rtrb SPSC<f32>  ─┐
                                                                                      │ (device rate; resample to 48 kHz if ≠)
mic 48 kHz ─► voice::mic thread ─► [Aec::Software: sonora APM @ 48 kHz, 10 ms] ◄────┘
                                        │ per 20 ms chunk: drain reference → process_render ×n,
                                        │ then process_capture ×2 (480 + 480)
                                        ▼
                                 Chunk (960 f32) ─► talk Task::chunk ─► Utterer (16 kHz VAD/STT)
```

- **Crate and rate.** `sonora` with echo canceller on, NS and AGC off at first
  (they change what VAD and Nemotron see; measure them separately), HPF on
  (AEC3 enforces it by default). Run at **48 kHz mono**, because the mic is
  already 48 kHz and the output device is usually 48 kHz, so the common case
  needs no resampling. Frame = **480 samples (10 ms)**, two per existing
  20 ms chunk. CPU per the upstream bench is about 13 µs per frame, around
  0.15 % of a core (to be measured on the Windows and Linux boxes).
- **Far-end tap.** In `Ring::fill`, after the output buffer is written, push
  the mono sample stream (one channel's value, i.e. the mono source) into a
  lock-free SPSC (`rtrb`, already in the tree). The callback must not lock or
  allocate: `rtrb` push is wait-free, and on overflow it drops samples and
  bumps an atomic counter. The tap sees paused zeros and post-flush audio, so
  the reference always equals what the speaker got. If the device rate is not
  48 kHz (24 kHz when the device accepted it, 44.1 kHz on some hosts),
  resample on the **consumer** side (mic thread) with rubato to 48 kHz.
- **Hand-off across actors.** The Player belongs to `voice::Voice` and the mic
  is opened by `talk`, so add a small process-wide `voice::out::Reference` hub
  (an `Arc` with the SPSC consumer slot plus the device rate). `Player::open`
  installs a producer, `Drop` clears it. `Capture::open_with(Aec::Software)`
  takes the consumer. With no Player, or none yet, the canceller gets zero
  render frames: correct, since nothing is playing. If the Player reopens with
  a different rate, reinitialise the resampler and the APM render config.
- **Near end.** In `voice::mic::run` (the non-VPIO thread) when `want ==
  Aec::Software`: accumulate 480-sample frames. Before each capture frame,
  drain every whole 10 ms reference frame available into `process_render_f32`,
  then call `process_capture_f32`. Output keeps the 960-sample `Chunk` shape,
  so `talk`, `Utterer` and the meeting's mic path do not change. It runs
  before the 48→16 kHz resampler, VAD and STT.
- **Delay strategy.** Causality comes for free: the reference is tapped at the
  **callback**, which runs before the sound leaves the speaker (by the output
  latency). By the time the echo reaches the mic, its render frames are
  already queued. The lag the filter must find is output latency + acoustic
  path + input latency + queue residence, all positive, typically tens to
  ~150 ms **(estimate, measure)**. AEC3 finds it with its own matched-filter
  delay estimator. We still call `set_stream_delay_ms` each frame, because
  the API requires it, with a coarse hint: `ring latency` (from
  `Ring::latency`, default 40 ms) + a fixed input-latency guess (≈ 20 ms, one
  chunk) + the reference queue depth converted to ms. `flexaudio-mic` drops
  `InputCallbackInfo`, so the capture timestamp is not available without a
  flexaudio change (not needed for v1). Log `statistics().delay_median_ms` /
  `delay_standard_deviation_ms` at debug every second, which shows directly
  whether the hint and the estimator agree. Two clocks (speaker and mic
  devices) drift slowly; AEC3 re-estimates, and the PoC's 30 s and 10 min runs
  check it.
- **Reference queue bounds.** Cap the SPSC at ~500 ms at device rate. If the
  mic thread stalls and the queue fills, drop the oldest reference so render
  never runs ahead of capture by more than the estimator window; count drops.
- **Reporting.**
  - `voice::mic::Aec` gains `Software` (serde `software`; ts export). What
    opened goes into `TalkStarted.aec` as today.
  - `AecMode::Prefer` becomes: VPIO on macOS, else `Software` when the talk
    has a device sink. Add `talk.aec: software` to force it on macOS for A/B
    against VPIO, and keep `off` for measurements.
  - `TalkDegradation::MicNoAec` is published only when neither opened, or for
    `Sink::Client`: the phone's browser does its own `echoCancellation`, and
    the daemon has no reference for a remote speaker.
  - The meeting crate's `Support::Partial("no echo cancellation; headphones
    recommended")` stays. The talk's reference is our own Player, and the
    meeting's far end is other participants. A follow-on can feed the
    meeting's **system-loopback capture** (flexaudio WASAPI/PipeWire) as the
    reference and retire `MicNoAec` there too. That has the same shape with a
    different producer, noisier timing.
  - Optional: a new `TalkDegradation::AecWeak` when `statistics()` shows ERLE
    < ~6 dB for > 5 s of far-end-only audio (divergence). Decide after the PoC.
- **Thresholds.** Keep text-domain echo rejection. Once measured, lower the
  default `barge_in_vad` for `Aec::Software` hosts toward the gate's 0.5
  (as on VPIO hosts) only if the false-barge-in count stays at zero.
- **What it does not cover.** Software AEC with our Player as reference
  cancels only theuth's own voice: music or a video playing in another app
  still reaches the mic, unlike VPIO. Using the system loopback as reference
  would cover that; defer it.

## 5. Proof-of-concept measurement plan

A probe example, `crates/voice/examples/aec_probe.rs`, so the daemon is not
touched until the numbers are in:

1. Open `Player` (default output) and `Capture::open_with(Aec::None)`
   together. Play a fixed 30 s TTS script (Pocket TTS output saved once as
   `script.wav`, 24 kHz) through `Player::write`. Record three WAVs with a
   shared sample clock: **raw mic** (48 kHz), **reference tap** (device rate),
   and the Ring's latency/position log. Script: 20 s agent-only, then 10 s
   where a person reads a fixed sentence over the agent (double talk).
2. **Offline**, on any machine, deterministic and repeatable: run raw mic +
   reference through (a) sonora, (b) tonarino `bundled` (Linux/macOS only),
   (c) nothing. Output per run:
   - **ERLE** over the agent-only span: `10·log10(Σ mic² / Σ out²)`, plus
     sonora's own `echo_return_loss_enhancement`, median and p10 per second.
   - Delay estimate median/stddev vs the Ring-reported latency.
   - **VAD**: TEN-VAD frames > 0.5 and > 0.7 during agent-only speech (these
     are the frames that open the barge-in gate).
   - Nemotron transcript of the agent-only span. Should be empty or near
     empty after AEC; today it is the agent's own words.
   - Double-talk span: transcript of the person's sentence (WER against the
     script) to check AEC does not eat the near end.
   - CPU µs per 10 ms frame.
3. **Live**, per host with speakers at normal volume, laptop mic, no
   headphones: 10-minute talk session, `aec: off` then `aec: software`, same
   script of questions. Count **false barge-ins** (`Holding` entered with no
   human speech; `Resumed{Echo}`; turns cancelled by the agent's own voice)
   from `RUST_LOG=talk=debug`, plus **true barge-ins caught** out of 10
   deliberate cut-ins. Use 76 §10 step 7's corpus where it applies.
4. **Hosts.** Sky's Mac first: speakers are available there, `aec: off` +
   software AEC can be compared against VPIO, which gives a ceiling. Then a
   Windows box and a Linux box **with real speakers and a mic, driven at the
   console, not over SSH**. build-B's audio was silent over SSH and build-A is
   a headless LXC with a null sink (knowledge
   `facts/theuth-meeting-capture-verified-windows-linux-2026-09-23`), so both
   can build and run the offline stage but cannot produce acoustic echo. The
   acoustic runs need Sky, or a physical machine.
5. **Pass criteria** (proposed, sharpening 76 §10 step 9):
   - builds with `cargo build` on all three hosts with no new system packages;
   - median ERLE ≥ 20 dB over agent-only speech (typical AEC3 territory on
     laptops, **unverified expectation**);
   - agent-only VAD frames > 0.7 drop by ≥ 90 %;
   - zero false barge-ins in the 10-minute run;
   - ≥ 9/10 true barge-ins still caught;
   - double-talk WER no worse than +5 points vs `aec: off`;
   - < 1 % of one core.

   If sonora misses ERLE/WER where tonarino passes, that is a sonora defect:
   report it upstream and go with rank 2.

## 6. Risks

- **sonora maturity**. 0.x, first released 2026-02, one main maintainer,
  AI-assisted port, few dependents. Mitigation: the offline A/B against C++
  AEC3 on the same recordings; pin `=0.2.x`; the API mirrors WebRTC, so the
  fallback crate is a small swap. The high download count is unexplained;
  treat it as no signal.
- **tonarino on Windows**. If needed there before #102 merges, it means a git
  dependency on the PR branch (loses the `v2_` symbol prefix on MSVC, which
  we do not need) and meson/ninja/clang on build-B.
- **Delay and alignment**. WASAPI's reported output latency is one buffer, an
  underestimate. Bluetooth outputs add 100–300 ms **(unverified figure)**,
  near the edge of what AEC3 tracks. The PoC should include one Bluetooth
  speaker run; if the estimator fails there, report `AecWeak` rather than
  claim AEC.
- **Clock drift** between separate mic and speaker devices over long talks.
  AEC3 re-estimates; the 10-minute run checks it.
- **Non-linear echo** (small laptop speakers at high volume). AEC3's
  suppressor handles much of it, and the text-domain rejection stays.
- **Near-end damage in double talk.** Over-suppression could clip the user's
  first syllables and delay barge-in. Measured by the WER and true-barge-in
  criteria.
- **Realtime safety** of the tap in the cpal callback. It must be wait-free
  (rtrb), with no locks or logging in `fill`.
- **Other apps' audio** is not cancelled (Player-only reference). That
  differs from macOS VPIO; document it.

## 7. Effort (worker-hours)

| Piece | Hours |
|---|---|
| `aec_probe` example (record raw mic + reference + latency log) and offline analyser (sonora, tonarino-bundled, none → ERLE/VAD/transcript/CPU) | 5–7 |
| PoC runs: Mac, Windows, Linux with speakers (+ Sky's time at two physical boxes) | 2–3 |
| Production: reference tap (rtrb in `Ring::fill`), `Reference` hub, `Aec::Software` in `voice::mic`, resample path, delay hint, stats logging, `talk.aec` semantics, degradation changes, ts bindings, unit tests (synthetic echo: reference convolved with an impulse response + delay, assert ERLE) | 8–12 |
| Three-host build and packaging check, clippy on Windows/Linux | 2–3 |
| As-built doc, 76/77 cross-references, knowledge record | 1–2 |
| **Total** | **18–27** (spike-only part: 7–10) |

## 8. Sources

- sonora: https://github.com/dignifiedquire/sonora · https://docs.rs/sonora/latest/sonora/struct.AudioProcessing.html · https://docs.rs/sonora/latest/sonora/stats/struct.AudioProcessingStats.html · https://docs.rs/sonora-aec3/latest/sonora_aec3/config/struct.Delay.html · https://crates.io/crates/sonora
- tonarino: https://github.com/tonarino/webrtc-audio-processing · https://github.com/tonarino/webrtc-audio-processing/pull/102 · https://github.com/tonarino/webrtc-audio-processing/issues/34 · https://lib.rs/crates/webrtc-audio-processing · https://docs.rs/webrtc-audio-processing
- aec3: https://github.com/RubyBit/aec3-rs · aec-rs: https://github.com/thewh1teagle/aec-rs · speexdsp-rs: https://github.com/rust-av/speexdsp-rs · LiveKit APM: https://docs.rs/libwebrtc/latest/libwebrtc/native/apm/struct.AudioProcessingModule.html
- Windows: https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nn-audioclient-iacousticechocancellationcontrol · https://github.com/microsoft/Windows-classic-samples/blob/main/Samples/AcousticEchoCancellation/README.md · https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/windows-11-apis-for-audio-processing-objects · https://learn.microsoft.com/en-us/windows/win32/medfound/voicecapturedmo · https://docs.rs/wasapi/latest/wasapi/struct.AcousticEchoCancellationControl.html
- PipeWire: https://docs.pipewire.org/page_module_echo_cancel.html · https://docs.pipewire.org/page_pulse_module_echo_cancel.html
- crates.io / GitHub API numbers read 2026-09-29. search_web was degraded for some queries (brave/google rate-limited), so pages were read directly.
