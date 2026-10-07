# 129 — EVENT-FOCUS: the desktop must not blank under concurrency

Status: design + contract, 2026-10-07. Built by wave 129 (W1 engine, W2 UI). As-builts: `129-as-built-W1.md`, `129-as-built-W2.md`.

## 1. Problem

During the 2026-10-06/07 overnight run (7 concurrent model streams, 12 sessions per 10 min, ~200k output
tokens per 10 min ≈ 330 delta events/s) the desktop window repeatedly went blank. The webview did not crash
(unified log: no WebContent termination, no jetsam) and the engine log is quiet. The blanking is the client's
own `Lagged` handling:

1. Every event of every session is published once into one `tokio::broadcast` ring of 1024 slots
   (`crates/rpc/src/bridge.rs` `CAPACITY`). `TextDelta` / `ReasoningDelta` are published per model stream
   chunk (`crates/agent/src/loop_.rs` ~3732, ~3747); `ToolCallProgress` per argument chunk.
2. The main window subscribes with `subscribe_events(null, null)` (`App.tsx attach()`), so `wanted()`
   (`api.rs` ~5968) passes everything, including subagents nobody is looking at.
3. The per-subscription task (`api.rs` ~5892–5929) does `rx.recv()` then `sink.send().await`. jsonrpsee's
   per-subscription buffer (default 1024) applies backpressure, so a webview that stops draining for a few
   seconds stalls the task, the ring reader falls >1024 behind, and `RecvError::Lagged` is turned into a
   `{"type":"Lagged","count":n}` frame. Total slack ≈ 2048 events ≈ 6 s at the observed rate.
4. The client (`App.tsx reload()`) answers `Lagged` with `sessions.unload()` — every slice `loaded:false`,
   so the visible chat renders empty — plus nine query invalidations and a full history refetch. The refetch
   keeps the main thread busy while the storm continues, so it lags again: a feedback loop of blank screens
   and "Events dropped, reloaded" toasts.

A perf fixture from 2026-09-15 (`crates/desktop/perf_fixture.ts`) handled ~700 deltas/s at 59.9 fps, so the
raw rate is not the issue; the budget is: any long task on the main thread (a large tool result rendering, a
history load) exceeds the 6 s of slack, and the recovery path is catastrophic.

## 2. Decisions

- D1 **Per-connection focus.** A client tells the engine which sessions it renders live text for; delta-class
  events for other sessions are not sent to it. A client that never says so gets everything (TUI, link relay,
  older clients): fully backward compatible.
- D2 **Deltas never cause `Lagged`.** The subscription task is decoupled from the sink: the ring is read
  promptly no matter how slow the client; delta-class events are merged while they wait (text concatenates,
  cumulative args keep the latest); only non-delta events queue, in a bounded backlog, and only that
  backlog's overflow (or a true ring lag) produces `Lagged`.
- D3 **`Lagged` is a resync, not a reset.** The client marks slices stale and re-settles what is on screen;
  it never unloads. Reloads are debounced; the toast is rate-limited.
- D4 **Instrument the main thread.** The client logs any drain or long task over 50 ms with a histogram of
  the event types in that batch, so the next heavy event type names itself.
- Out of scope: the link relay / web and phone clients (they subscribe through the same ring and keep
  today's behaviour; a follow-up can call `set_event_focus` from them), serialising each event once instead
  of once per subscriber (cheap at these rates; may be done if trivial).

## 3. Contract (frozen; both workers build against it verbatim)

### 3.1 Delta-class events

`TextDelta`, `ReasoningDelta`, `ToolCallProgress`, `VisionProgress`. Nothing else is filtered or merged.
`TalkEvent`, `VoiceEvent`, `SpeechAudio`, `JobEvent`, `ActivityAppended` and every other event pass as today.

Merge rules (per subscription, while a frame waits to be sent):
- `TextDelta` with the same `(session, turn)`: one event, `text` = concatenation in arrival order.
- `ReasoningDelta` with the same `(session, turn)`: same.
- `ToolCallProgress` with the same `(session, turn, call_id)`: the latest event (its `args` is cumulative).
- `VisionProgress` with the same `(session, turn)`: the latest event.
- Order within a session is preserved: every pending merged delta of session S is emitted before any later
  non-delta event of S (flushing all pending deltas before any non-delta event is an acceptable
  implementation).
- Flush cadence: pending deltas are flushed at least every `FLUSH_MS = 40` ms when the sink has room; a
  slow sink simply receives fewer, larger deltas.

### 3.2 RPC method (crates/rpc/src/api.rs, `Api` trait; TS in crates/desktop/ui/src/rpc/api.ts)

```rust
/// 129 §3.2: per-connection focus. Delta-class events (`TextDelta`, `ReasoningDelta`,
/// `ToolCallProgress`, `VisionProgress`) reach this connection's `subscribe_events` streams
/// only for `sessions`; every other event is unaffected. A connection that never calls this
/// receives everything. Replaces the previous set; `[]` means no deltas at all. The set dies
/// with the connection.
#[method(name = "set_event_focus", with_extensions)]
async fn set_event_focus(&self, ext: &Extensions, sessions: Vec<SessionId>) -> RpcResult<()>;
```

```ts
// api.ts — positional params, trait order, like every other method
set_event_focus(sessions: SessionId[]): Promise<null>
```

Semantics: keyed by the connection (`peer(ext)?.id()`, the same id `subscribe_events` uses); applies to
every `subscribe_events` stream of that connection, including ones opened before the call; a session in the
focus set that the connection is not authorised for is simply never matched (no error). Unknown ids are
allowed (a session may be created after the call).

### 3.3 `Lagged`

Shape unchanged: `{"type":"Lagged","count":n}` followed by the `PendingPrompts` snapshot, as today. New
meaning: `count` non-delta events were dropped for this stream. Emitted when the ring lagged
(`RecvError::Lagged`, now only if the drain itself stalls) or when the per-stream non-delta backlog
(`BACKLOG = 8192` events) overflowed — the oldest are dropped first and counted.

### 3.4 Client behaviour

- On `open` of every connection and whenever the set of sessions the window renders live text for
  changes (route, panes, popout, children shown live under a visible parent): `set_event_focus(set)`,
  debounced ~100 ms, re-sent after every reconnect (server state is per connection).
- A session leaving focus is marked `contextStale`; entering focus re-settles it (the existing
  `contextStale → settle()` path in `routes/c.$session.tsx`), because deltas were skipped meanwhile.
- `Lagged` → mark every slice `contextStale`/`todosStale`, re-settle the visible sessions, invalidate the
  sessions list; never `unload()`; at most one such reload per 5 s; the toast at most once per minute.
  A real reconnect (`reconnecting` → `open`) keeps its wider invalidations but also stops calling `unload()`.

## 4. Work split

- **W1 — engine** (`crates/rpc` only: `src/api.rs`, `src/bridge.rs`, tests): §3.1, §3.2, §3.3.
  Gates: `cargo fmt -p rpc`, `cargo clippy -p rpc --no-deps -- -D warnings`, `cargo test -p rpc`.
- **W2 — UI** (`crates/desktop/ui` only): §3.4 and D4; hand-written TS stub for `set_event_focus` in
  `rpc/api.ts` (bindings are regenerated by the integrator with `just ts-desktop`).
  Gates: `bun run check`, `bun run test` (vitest), `bun run build`.
- **Integrator**: merge W1 + W2 on `feat/129-event-focus`, fold main, `just ts-desktop`, rerun both gate
  sets, `cargo check --workspace` once, land with `merge --no-ff`.

## 5. Proof

- W1 tests: focus filter (deltas dropped for unfocused sessions, control events pass, `[]` drops all
  deltas, no call = everything); merging (10 `TextDelta`s of one turn → one event with the concatenated
  text; a `ToolCallStarted` of the same session is emitted after the merged delta); a sink that is not read
  for 10 000 deltas produces no `Lagged`; backlog overflow produces one `Lagged` with the right count;
  a connection's focus dies with it.
- W2 tests: `Lagged` leaves `loaded` untouched and marks stale; debounce and toast rate limit; focus set
  derivation from route + panes (+ children shown live); re-sent on `open`.
- Manual: 5+ sessions streaming, the window never blanks, no "Events dropped" toast; the console shows no
  drain > 50 ms, or names the event type that caused one.
