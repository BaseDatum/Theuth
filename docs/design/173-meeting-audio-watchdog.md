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
| ZEROS (app tap) | every sample 0.0 for 15 s, *no* activity precondition | PROBE: a concurrent `SystemLoopback` (`exclude_self`) for 4 s |
| PROBE audible | ≥ 1 s of 100 ms blocks peaking ≥ −70 dBFS | episode `Silent` from the first zero chunk, reopen + retarget |
| PROBE silent | — | genuine silence: no episode, no gap, no warning; re-probe after 30 s, 60 s, then every 120 s while the zeros last |
| PROBE failed to open | — | warn once; this zero run uses the activity-gated rule below |
| Reopen judged (app tap) | 5 s of zeros after it, then a second probe | audible → failed (warning from the first); silent → episode ends as genuine silence |
| Reopen judged (stall) | no chunk 3 s after it | failed |
| Backoff | after a failed reopen | 1, 2, 5, 10, then every 30 s; reset after 30 s clean |
| RETARGET | every reopen | tapped pid not running output and a family member is → tap it |
| FALLBACK | 3 consecutive failed reopens, each confirmed by an audible probe (stalls: family running output) | `SystemLoopback` + `exclude_self` |
| RETURN | every 60 s in the fallback | 4 s probe of the app's tap (target pid while it lives, else a family member running output); ≥ 0.5 s non-zero → back on it, health `Ok` |
| Global tap (pid 0, the fallback) | 15 s of zeros and anything running output (30 s / every 2 min if unknown) | reopen (nothing to probe against) |
| `PermissionDenied` / `TerminalError` | at once | stream dropped, retried with backoff (no fallback on permission) |

Family (`in_family`): the target itself, the same bundle id or one under it (`us.zoom.xos.*`, not
`com.google.drivefs` for `com.google.Chrome`), or an executable inside the target's outermost `.app`
(`libc::proc_pidpath`; `…/zoom.us.app/Contents/Frameworks/aomhost.app/…` is zoom.us's); never our own
pid. `IsRunningOutput` now only picks retarget/return candidates, gates the global tap's rule, and
appears in the health line. The probe floor is `AUDIBLE_DBFS` (Zoom's natural-pause floor in the
incident recording: −88…−95 dB RMS; speech −25…−45 dB); `THEUTH_MEETING_PROBE_DBFS` overrides it as a
calibration knob. The mic gets STALL and device-error reopening only: a muted mic is legitimately
zero. Every flexaudio event is logged at warn (`ChunkDropped` / `Clipped` summed into one line a
minute); each track gets one info line a minute: `meeting capture health: system chunks= frames=
zero=% peak=dBFS target= active= reopens= episode=`.

**Accepted false positive:** a muted call while *another* app plays sound at ≥ −70 dBFS is
"audible elsewhere": the app tap reopens, warns after the first judged failure, and after three falls
back. The fallback then records that app plus Zoom, until a return probe hears the app's own tap.

## What is recorded and shown
- Each episode becomes a manifest `gaps` entry with `track`, `reason` (`Stalled | Silent |
  PermissionDenied | Failed`) and `retarget` (the new pid). An entry without `track` is still a
  pause. The fallback adds `degraded: SystemAudioFallback { reason, at_ms, until_ms }` (`until_ms` set
  when a return probe brings the app's tap back); a denied permission adds
  `PermissionDenied(AudioCapture)`.
- `MeetingEvent::Capture` and `meeting_status.capture` carry `{mic, system}` health:
  `Ok | Recovering | Lost | Degraded`. A track stays `Ok` until the first reopen failed (about 29 s
  of zeros: 15 s + probe 4 s + 5 s + probe 4 s; 7 s for a stall); a denied permission or terminal error shows at once. The control center
  shows "Not hearing zoom.us since 21:08 — reconnecting…", then "Still silent — check the output device
  or System Audio Recording permission". The web island and the native companion island show a badge
  and the same line. The warning clears by itself when audio returns.

## Follow-ups
- A playing source on a non-default output device is not in the probe's global tap (it follows
  the default device); such zeros read as genuine silence.
- flexaudio upstream: zero-filled buffers count as healthy; the silence detector is one-shot and
  per-object; no property listeners (process list, default device, nominal sample rate).
