# 49 — as-built SEARCH-DEBOUNCE: one `search` RPC at a time from the desktop (2026-09-29)

Worktree `/Users/Sky.Moore/Develop/theuth-search-fix`, branch `fix/search-debounce` off `4d11125a`.
Brief `/tmp/talk76/omp-SEARCH-DEBOUNCE.md` plus Sky's rule mid-flight: a fast-typed word is one intent —
never send or show intermediate prefixes; an older in-flight answer is stale; the list reflects only the
latest settled query. Scratch engine `THEUTH_DATA_DIR=/tmp/search-fix-data` (`127.0.0.1:7777`), UI
`bunx vite --port 5188` with `VITE_THEUTH_URL/TOKEN`, headless Chromium.

## What Sky saw, traced

- `SearchDialog` and `AttachPicker` already debounced `typed` → `debounced` (150 ms, resetting) and keyed
  every React Query request by query text, so a 30 ms/key burst never sent prefixes. Human typing is
  150–250 ms/key: each pause settled a prefix and `useQuery` fired `api.search` for the new key while the
  previous key's fetch was still running — React Query never serialises fetches across keys. Show-more
  pages and the 500 ms indexing poll (`building`) added parallel calls for the same query.
- Server: every owner matches under a one-permit semaphore and refuses a concurrent match with a group
  `error` page — `artifact/src/search.rs` `"artifact search is busy; retry"`, `knowledge/src/search.rs`
  `BUSY = "knowledge search is busy; retry"`, `agent/src/search.rs` likewise. Not an RPC error: the
  `search` reply is `Ok` with that group in `state: error`, rendered by `Section`/`status()` with Retry.
  49 §4.3 and §11.2 say this is deliberate ("refused, not queued"; "obsolete queued client requests are
  coalesced in the frontend").

## Decision

**Server guard kept; the client never triggers it.** 49 §4.3 chose refusal over an unbounded queue and
names the frontend as the coalescer; queueing or allowing concurrent matches server-side would only
absorb a client bug and would still let the agent's `search` tool and the UI collide unboundedly. The
result shape is untouched. The only Rust change is one `tracing::debug!(%query, ?categories, ?limit,
"rpc search")` in `Api::search` (`crates/rpc/src/api.rs`) so the engine log can count RPCs
(`RUST_LOG=info,rpc=debug`).

## Client (`crates/desktop/ui/src`)

`lib/search-queue.ts` — `SearchQueue`, one instance per surface (`useRef(new SearchQueue()).current` in
`SearchDialog` and `AttachPicker`; `components/settings/Search.tsx` is the web-search settings pane and
issues no `search`):

- `type(query, settle)` — resetting debounce, `DEBOUNCE_MS = 200` (was 150 in both surfaces); `cancel()`.
  Enter still settles by hand (`flush()` in the dialog, the picker's `attach` action) and the effect
  cleanup cancels the timer.
- `run(query, send)` — single flight. Nothing in flight → send now. Otherwise the request waits; requests
  for the **same** query text queue FIFO (base page, Show more page, indexing poll); a request for a
  **different** text drops every waiting one with `SUPERSEDED` (depth 1 by query). An in-flight request is
  never cancelled — its answer lands under its own React Query key, which nothing observes once
  `debounced` moved on, so stale successes and stale errors are discarded alike. A rejected `SUPERSEDED`
  query is unobserved, so React Query's `retry: 1` does not re-send it (`Query.removeObserver` →
  `cancelRetry`).
- `clear()` — cancel the debounce and drop what waits. Both surfaces call it when the box empties (which
  now settles `''` at once instead of after the debounce) and when the surface closes.
- All five `api.search` call sites go through `queue.run(debounced, () => api.search(...))`.

Error display: unchanged — the RPC-level error is the inline `<Notice tone="error">` with Retry; a group's
`busy` page is the section's status line with Retry; no toasts existed on this path and none were added.

## Tests

- `lib/search-queue.test.ts` (5): debounce restarts per keystroke and settles once (`aquarium` at 30 ms/key
  → one `settle('aquarium')`), cancel/clear never fire; single flight + same-query FIFO + supersede +
  failed send frees the slot; clear drops waiting not in-flight; and two pipeline tests against a real
  `QueryClient`/`QueryObserver` wired exactly as `useQuery` is in the surfaces: **`aquarium` typed at
  30 ms per letter → `sent === ['aquarium']`, rendered results `['aquarium']`, cache holds only that
  key**; slow typing (`a`,`aq`,`aqu`,`aqua` with pauses > debounce, slow server) → `a` in flight, `aq`/`aqu`
  never sent, `aqua` sent when `a` returns, the `a` answer never rendered, clearing the box mid-wait drops
  `aquari` and empties the list. The dialog itself is not rendered: the suite has no DOM (`vitest` node
  env, no jsdom/testing-library; `include: src/**/*.test.ts`), and adding one for this was out of
  proportion — the pipeline test covers everything between `typed` and the rendered `data`.
- Rust: no server behaviour changed; `cargo clippy -p rpc -p search --all-targets` and
  `cargo test -p rpc -p search` run for the log line (see below).

## Verification

- `bunx tsc --noEmit` clean; `bunx vite build` ok; `CI=1 bunx vitest run < /dev/null` → 117 files, 658
  tests: 657 passed, 1 failed — `lib/mobile-zoom.test.ts` "Test timed out in 5000ms" under the full
  parallel run (the same environmental timeout 76-as-built-DESKTOP-TALK recorded). Alone, with the search
  files: `4 passed (4) · 14 passed (14)`.
- Live, `target/debug/t --headless` on the scratch data dir, Chromium on `:5188`, ⌘K dialog, All six
  categories:
  - `aquariumfish` (12 chars, ~50 ms/key): engine log `rpc search` lines **2** (≤ 3 asked): the base page
    `aquariumfish [Sessions, Artifacts, Knowledge, KnowledgeHistory, Notes, Meetings]` and one same-query
    indexing poll `aquariumfish [Sessions, Artifacts, Knowledge]` 157 ms later. Zero `busy` lines; dialog
    showed "No matches in selected categories" with six `· no matches` sections, no error line.
  - Clear → no request; `notebook` typed via the tool's `type({delay: 260})` → 1 request; erasing 8 chars
    at 40 ms → 0 requests. Explicit 300 ms pauses per key (`n`…`noteboo`) → one request per settled
    prefix, ~330 ms apart, none overlapping (the empty corpus answers in a few ms) — the overlap/latest-
    wins path is exercised by the vitest pipeline test, not live: no corpus on the scratch engine is slow
    enough to make two matches collide.
- Rust: `cargo test -p rpc -p search` → `35 passed` (rpc) + `1` + `6` (search) + doctests, 0 failed.
  `cargo clippy -p rpc -p search --all-targets` does not reach `rpc`: it stops at the dependency `agent`
  with `type_complexity` on `crates/agent/src/supervisor.rs:564` (`acp: Option<(String, Option<PathBuf>,
  Option<Risk>, Vec<String>)>`), present on `main` @ `4d11125a` and outside this brief's allow-list; not
  touched. `cargo build -p theuth` (which builds `rpc`) is clean.

## Not done / notes

- `AttachPicker` was not driven live; it shares the class and the same wiring (five call sites total).
- The two surfaces have independent queues (the brief's "one controller per surface"). If the picker and
  the dialog ever search at the same instant the server can still answer one group `busy; retry` — the
  existing Retry handles it; a per-engine queue would couple the picker's poll to the dialog's typing.
- ponytail: `SUPERSEDED` is one shared `Error` instance; give it a class if a caller ever needs to
  distinguish it from a network failure programmatically (today only unobserved queries receive it).
