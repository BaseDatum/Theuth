# 224 — Lean engines: subsystems start for the engine's role

Status: built on `feat/node-slim`. Owner report 2026-11-27: a fresh 1 GB cloud node (`t --headless --data-dir /data`,
2.31.2, 70 min after joining) sat at 415 MB RSS (318 MB anon), 79 threads, 371 MB swapped. Every engine started every
plugin regardless of role: meetings, voice, talk and computer use on a box with no microphone or user; dreaming and the
Signal/Slack/Telegram listeners on every engine of a mesh.

Out of scope (owner, 2026-11-27): session replication on join. A new member keeps syncing the full history exactly as
today; no horizon, no lazy session bodies.

## 1. The gate
`crates/theuth/src/subsystems.rs`. Each gated subsystem declares what it needs; `decide(rule, override, env)` returns
`Start`, `Skip(reason)` or `FollowHub`. Plugins not in the table always start (agent, tools, mesh, replica, sessionsync,
roster, schedule, …).

| Need | Met when |
|---|---|
| `Hub` | this engine holds the hub lease (67 §4.4); followed at runtime (§3) |
| `Display` | a display is reachable now: Linux/BSD `WAYLAND_DISPLAY` socket or `DISPLAY` whose `/tmp/.X11-unix/X<n>` exists (TCP `host:N` counts) — Xvfb counts; macOS/Windows always. `--headless` does not matter |
| `User` | an interactive host (desktop app daemon, TUI, `-p`, `--acp`; never a plain `t --headless`) and an audio device (Linux: `/dev/snd/pcm*`, PulseAudio/PipeWire socket or `PULSE_SERVER`; elsewhere assumed) |

`Env` is probed once at start (`Env::probe(host, daemon)`); `host` is the existing `rpc::engine::Host` —
`THEUTH_LAUNCH` makes the desktop app's `--headless` daemon `Host::Desktop`, so the desktop keeps everything.

## 2. Table

| Subsystem | Rule | Config |
|---|---|---|
| dreaming | `Hub` (absent elsewhere) | `subsystems.dreaming`, `dreaming.enabled` |
| signal | `Hub` for the listener; control-only (`channel: false`) off the hub and in terminal mode; listener needs linked store | `subsystems.signal`, `signal.enabled` |
| slack | `Hub` for the listener; control-only elsewhere; listener needs both tokens | `subsystems.slack`, `slack.enabled` |
| telegram | `Hub` for the listener; control-only elsewhere; listener needs the token | `subsystems.telegram`, `telegram.enabled` |
| meeting | `User` + `Display` | `subsystems.meeting`, `meeting.enabled` |
| voice (TTS, STT, dictation) | `User` | `subsystems.voice`, `voice.enabled` |
| talk | `User` | `subsystems.talk`, `talk.enabled` |
| computer | `Display` | `subsystems.computer`, `computer.enabled` |

`subsystems.<name>: auto | on | off` (YAML bools too). `on` starts regardless of the rule — a hub-only subsystem then
runs on every engine; `off` never. The existing `<name>.enabled: false` keeps working: the plugin still starts and runs
its own disabled path (dreaming switches its routine file off), reported as skipped. Skipped plugins still declare their
wire and RPC (the contract does not change with the role); their methods answer as when the plugin is disabled or not
running. Lifecycle participants (`ExpectParticipants`, 51 §8.2) list only plugins that start, so a skipped
voice/talk/computer never holds a private root.

A headless hub that should transcribe Signal/Slack voice notes (45) sets `subsystems.voice: on`.

One startup line in the log:

```
subsystems: started [...]; skipped [meeting (headless: no user or microphone), ...]; hub-only, following the lease [dreaming, signal, slack, telegram] (interactive=false, display=false, audio=false)
```

## 3. Hub transitions
`mesh::hub_watch()` (new) is a `watch<Option<bool>>`: `None` until the mesh actor's first `MeshEvent::Lease`, then
`Some(holder == self)`, set beside the existing `set_hub`. `follow_hub` waits for the first value (15 s, then falls back to
`mesh::is_hub()` — no mesh, or it failed), starts each hub-only subsystem's phase plugin, and on every change that holds
for 5 s stops the old phase and starts the new one. A lease of `None` (warming, no holder) is "not the hub".

Each phase runs on `Runtime::scoped()` (new): a clone whose `shutdown` is a child token. `Scope::stop` cancels it — the
channel loops already select on `rt.shutdown` — and waits for the plugin's `on_scope_stop` hooks:
`release_on_scope_stop(actor)` kills named actors (kameo drops the registry name, so the next start registers again),
`agent::unregister_sink_on_scope_stop` drops the `message_user` sink (`UnregisterUserSink`, new), dreaming unregisters its
tools and prompt block. On the engine's own runtime these are no-ops, so plugins behave as before outside the gate.

Before: `HUB` defaulted to `true`, so anything reading `is_hub()` at boot saw a hub on every engine until the first lease.
The gate waits for the lease instead.

## 4. Measurements
See §6.

## 5. Binary size and eager loads (report; no packaging change)
No model weights are linked into `t`; ONNX Runtime is `load-dynamic` (`ort`, `parakeet-rs`), MLX is
`cfg(macos, aarch64)`. Size comes from: no `[profile.release] strip`/`lto` (symbol tables stay; a study build went
130 → 80 MB stripped), V8 (`v8` via `deno_core` and obscura, ~60–100 MB text), microsandbox `embed-binaries`
(~28.5 MB runtime archive), four crypto stacks (BoringSSL ×2, aws-lc, ring), Typst fonts, the obscura snapshot, JS
bundles, tree-sitter grammars. `.rodata` is file-backed on Linux: it costs page cache, not anon RSS, unless copied.

Eager at boot on every host: `browser::warmup` (an obscura V8 isolate kept for the process, unless
`browser.enabled: false`) and `js_runtime::init_platform` (V8 platform + one dropped isolate). Lazy already: voice
STT/TTS, dictation, talk, meeting, codeindex (indexes on first tool call; no watcher), decision engines and the
embedder, sandbox runtime extraction. `document` reindexes in the background at boot and may load the embedding model
on a first run or model change.

## 6. Designed, not built
- Lazy browser warmup on headless nodes (move the obscura isolate to the first `browser_*` call; ordering constraint
  noted in `browser::warmup`).
- `[profile.release] strip = "symbols"`, feature-gating microsandbox `embed-binaries` and obscura `render` for a node
  build — packaging changes, left to the release owner.
