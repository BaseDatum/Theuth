# 129 — as built, W1 (engine, `crates/rpc`)

Contract: `129-event-focus.md` §3.1–§3.3. Files: `crates/rpc/src/api.rs`, `crates/rpc/src/bridge.rs` (doc only).

## What

- `set_event_focus(sessions)` — trait `api.rs:1850–1858`, impl `api.rs:6020`. Keyed by `peer(ext)?.id()`;
  stored in `Server.focus: FocusMap` (`api.rs:1942`, `Arc<RwLock<HashMap<u64, Arc<HashSet<String>>>>>`,
  session ids as the strings the serialized `session` field carries). Replaces the set; `[]` = no deltas;
  no entry = everything. The first insert for a connection spawns one task awaiting `RpcPeer::closed()`
  that removes the entry — the set dies with the connection (brief allowed "last stream closes"; the
  connection-close hook is simpler and also covers a connection that never subscribed).
- `is_delta` (`api.rs:6040`): `TextDelta | ReasoningDelta | ToolCallProgress | VisionProgress` by `type`.
- Subscription task (`api.rs:5912–6018`): two tasks per stream sharing `Stream` (`api.rs:6056`,
  `parking_lot::Mutex`, locked per statement, never across an await).
  - Reader: `rx.recv()` → `wanted` → `admitted` → focus check (deltas only, one `RwLock` read per delta)
    → `Backlog::push`, then `Notify`. `RecvError::Lagged(n)` adds `n` to `lost`; `Closed` flags the
    stream and the writer returns once drained.
  - Writer: `Backlog::next` → `Frame` (serialize + `sink.send().await`), `Lagged(n)` (frame + fresh
    `PendingPrompts` snapshot, as before), or `Wait(at)` (select on `sink.closed()`, the notify, and
    `sleep_until(at)` for the flush deadline). The reader is aborted when the writer ends.
- Merge/backlog state machine `Backlog` (`api.rs:6069–6160`), one `VecDeque<Event>` in emit order:
  - non-delta: appended; at `BACKLOG = 8192` non-deltas the oldest non-delta is removed and `lost += 1`.
  - delta: scan from the back over frames of the same session; stop at the first non-delta of that
    session (preserves per-session order exactly, not just "all deltas before any non-delta"); a frame
    with equal `type`, `turn`, `call_id` merges — `text` concatenates (`Arc::make_mut`), otherwise the
    newer event replaces (`ToolCallProgress.args`, `VisionProgress` are cumulative). Deltas are never
    dropped or counted.
  - flush: a delta batch becomes due `FLUSH_MS = 40` ms after its first delta, or immediately when a
    non-delta is queued behind it; once due, everything pending goes (`due` counter), later arrivals
    start the next batch. A slow sink therefore gets fewer, larger deltas.
- Constants: `FLUSH_MS = 40` (`api.rs:6048`), `BACKLOG = 8192` (`api.rs:6052`). `bridge.rs CAPACITY`
  unchanged (1024), doc updated.

## Tests (`api.rs` `mod tests::event_focus`)

- `backlog_merges_deltas_in_session_order` — (b) 10 `TextDelta` → one event, `ToolCallStarted` after it,
  `ToolCallProgress` keeps latest args, a same-session non-delta splits the merge, other session merges
  across it, trailing batch waits `FLUSH_MS`.
- `backlog_overflow_drops_oldest_and_counts` — (d) pure: 8197 non-deltas → `Lagged{count:5}` first, oldest
  gone, deltas untouched.
- `focus_filters_deltas_only` — (a) end to end over `Methods::subscribe`: unfocused deltas dropped, their
  non-deltas pass, set replaced on a pre-existing stream, `[]` drops all deltas, no call passes everything.
- `stalled_sink_merges_deltas_without_lag` — (c) sink buffer 1, 10 000 deltas through a 1024 ring: no
  `Lagged`, text intact in ≤ 3 frames.
- `backlog_overflow_lags_once_then_snapshots` — (d) end to end: one `Lagged`, then `PendingPrompts`, then
  the remaining titles in order ending at the last one.
- `focus_dies_with_the_connection` — (e).

## Deviations / notes

- Focus entry removal is on connection close (`RpcPeer::closed()`), not "last stream closed". Same
  bound (one entry per live connection), fewer moving parts.
- Serialize-once (§2 optional) skipped: still `to_raw_value` per subscriber.
- `Lagged` on a true ring lag is also routed through the backlog's `lost` counter, so both sources
  produce one frame with the summed count, followed by the snapshot.
- `Backlog::push` on overflow scans from the front for the oldest non-delta (`ponytail:` comment); the
  deque holds at most 8192 non-deltas plus pending deltas, and the oldest non-delta is near the front.
