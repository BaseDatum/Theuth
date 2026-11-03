# 173 — meeting audio watchdog

## Incident
2026-11-02, meeting `01a19e11-dc39-7912-9f9f-0ed0169b1ccf` (Zoom, pid 51437, `us.zoom.xos`,
per-process tap = flexaudio 0.8 `ProcessLoopback`). 6 min 10 s in (≈ 21:08:23Z), `system.opus`
switched within one 50 ms window from −24.6 dB mid-word to exact digital zero and stayed there
for the last 38 min (458/458 five-second windows flat). The mic track and the transcript show
the call went on. No log line, no recovery, `gaps: []`, `degraded: []`, nothing on screen.

## Root cause
- **The tap kept delivering chunks of exact zeros (confirmed).** The live diariser's per-minute
  counter (`diar.rs` — it only counts chunks it pops from the system ring, fed only by the tap
  thread) went on at 2 500–2 790 per minute from 21:09 to 21:46, as before the drop. If the chunks had
  stopped, the encoder would have padded zeros (`encode.rs` `Source::take`, `out[n..].fill(0.0)`),
  and that counter would have gone quiet. The opus file alone cannot tell the two apart.
- **Why nobody noticed (confirmed, code).** flexaudio's stall watchdog counts any non-empty buffer
  as healthy (`flexaudio-0.8.1/src/stream/intake.rs:178`). Its exact-zero `SilenceDetector` runs
  once per stream generation and only while the *tapped object's* `IsRunningOutput` is true, and its
  healthy probe emits nothing. It has no Core Audio property listeners at all. theuth matched only
  four terminal events and had no watchdog of its own.
- **OS trigger (likely, not reproduced for Zoom).** `examples/tap_repro` taps an inaudible child
  (−80 dBFS). Re-creating its output unit, or switching it to another output device, keeps the
  audio (−80 dBFS, 0 % zeros). When it stops rendering, the tap goes on at 50 chunks/s of exact
  zeros, with `IsRunningOutput` = false and no event. That is the incident's signature. So Zoom's pid
  51437 most likely stopped rendering into its tapped process object: playback moved to another
  process object or pid, or the object was re-created on a route or Bluetooth profile change.
  Settling it needs the unified log for 21:08:00–21:09:00 (see the 173 report).

## Watchdog (`crates/meeting/src/capture/watchdog.rs`, driven by `system.rs` / `mic.rs`)
| Rule | Threshold | Action |
|---|---|---|
| STALL | no chunk 3 s | reopen now; if flexaudio said `StreamStalled`, wait until 5 s after it for `StreamRecovered` |
| ZEROS | every sample 0.0 for 15 s, source playing | reopen |
| ZEROS, activity unknown | 30 s of zeros | reopen, at most every 2 min |
| ZEROS, source not playing | — | genuine silence, nothing |
| Reopen judged failed | no chunk 3 s after it, or a fresh 15 s zero window | next reopen after 1, 2, 5, 10, then every 30 s; reset after 30 s clean |
| RETARGET | every reopen | pid not playing and another same-bundle pid is → tap that pid |
| FALLBACK | 3 consecutive failed reopens, source playing | `SystemLoopback` + `exclude_self` for the rest of the meeting |
| `PermissionDenied` / `TerminalError` | at once | stream dropped, retried with backoff (no fallback on permission) |

"Playing" = `kAudioProcessPropertyIsRunningOutput` of the pid or any same-bundle process. For
the global tap (pid 0) it is any process. The process list is read at most once a second. The
mic gets STALL and device-error reopening only: a muted mic is legitimately zero. Every flexaudio
event is logged at warn (`ChunkDropped` / `Clipped` summed into one warn line a minute). Each track
also gets an info line a minute: `meeting capture health: system chunks= frames= zero=% peak=dBFS
target= reopens= episode=`.

## What is recorded and shown
- Each episode becomes a manifest `gaps` entry with `track`, `reason` (`Stalled | Silent |
  PermissionDenied | Failed`) and `retarget` (the new pid). An entry without `track` is still a
  pause. The fallback adds `degraded: SystemAudioFallback(reason)`; a denied permission adds
  `PermissionDenied(AudioCapture)`.
- `MeetingEvent::Capture` and `meeting_status.capture` carry `{mic, system}` health:
  `Ok | Recovering | Lost | Degraded`. A track stays `Ok` until the first reopen failed (about 30 s
  for zeros, 7 s for a stall); a denied permission or terminal error shows at once. The control
  center shows "Not hearing zoom.us since 21:08 — reconnecting…", then "Still silent — check the
  output device or System Audio Recording permission". The web island gets a badge with that text
  as its tooltip. The warning clears by itself when audio returns.

## Follow-ups
- Native (Swift companion) island: show `capture` health there too.
- If Zoom's playback moves to a pid with a *different* bundle id, activity reads "not playing" and
  the zero rule stands down, so the incident's silence would only show in the health line. A
  bundle-prefix match (`us.zoom.xos.*`) or a device-level activity check would close that hole.
- flexaudio upstream: zero-filled buffers count as healthy, and the silence detector is one-shot
  and per-object.
