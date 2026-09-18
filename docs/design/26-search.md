# 26 — Session search: fuzzy, scored, over titles and messages

Status: as-built v0 · 2026-09-15 · **superseded for the human surface by
[49](49-search.md)** on 2026-09-18 · extends `00-architecture.md` §7 (RPC),
§8 (TUI), §10 (tree); `25-desktop.md` §3 (parity table: search),
`08-repl.md` (`/sessions`). §8 is the as-built record of v0; §9 records what
49 kept, moved and removed. Sections 1–7 describe v0 as designed and are
historical where §9 says otherwise.

## 0. Why

The desktop app's ⌘K searched `list_sessions` titles with cmdk's built-in
scorer, client-side, and said so ("titles only"). The TUI had no session
search at all. jarvis already solved this — `nucleo` (helix's fuzzy matcher,
the fzf algorithm) over titles and message bodies with per-conversation
collapse, snippets and match ranges — and the user asked for that library
here. This doc ports it, server-side, once, for every frontend.

## 1. Goal, and what is not one

- One RPC, `search_sessions(query, limit?) -> [SearchHit]`: the best hit per
  session over every session's title and the text of its visible user and
  assistant messages, best score first, each with a snippet and the matched
  char ranges so a frontend can highlight.
- fzf query syntax (`Pattern::parse`): space-separated atoms, `'exact`,
  `^prefix`, `suffix$`, `!not`. Smart case, Unicode normalisation.
- Both frontends: desktop ⌘K (25), TUI `/search <query>` picker.
- No lag: the index is a cached in-memory extraction, matching runs off the
  actor mailbox and off the runtime threads.

Not goals: searching tool results (large, noisy, and what matters in them
the assistant repeats — a later opt-in), knowledge (10 has FTS5 already),
jump-to-message on open (the hit carries `seq`; the transcript's paged
loader (25 §6.2) lands on the tail — scrolling to `seq` is an upgrade
listed in §7), incremental index maintenance (§2.2).

## 2. Index

### 2.1 What is indexed

`crates/agent/src/search.rs` `Index::build(store)`: two scans.

- `Store::search_titles()` — `(id, title, updated_at)` of every session.
- `Store::search_bodies()` — `(session, seq, ChatMessage)` for every row
  with `seq < COALESCE(cut_at, MAX)` (undone turns are not history) and
  `json_extract(body, '$.role') IN ('User', 'Assistant')`. The role filter
  is in SQL so tool rows — the bulk of the table by bytes — are never
  decoded. Text is `loop_::text_of` (17 §3.5: binary parts render as
  `[image: name]`), empty texts dropped.

Measured on the author's store (2026-09-15): 100 sessions, 3706 messages,
8.6 MB; user+assistant rows are 2387 / 6.4 MB. One build is one SQLite
scan plus JSON decode of those rows.

### 2.2 Caching and invalidation

`search::Cache` lives in the `Agents` supervisor: a generation counter
(`AtomicU64`) and a `tokio::sync::Mutex<Option<(generation, Arc<Index>)>>`.

- `invalidate()` bumps the generation — lock-free, called from the
  supervisor's `AgentEvent` handler on `TurnFinished`, `TitleChanged`,
  `SessionDeleted`, `Compacted`, and from the `Undo` / `Redo` handlers.
- `search(store, query, limit)` locks the slot; if the cached generation is
  stale it rebuilds (awaiting sqlx under the lock, so concurrent searches
  serialise on one build, not N), clones the `Arc`, unlocks, and matches on
  `spawn_blocking`.

Rejected: rebuild on every query (blocks the actor for a scan per
keystroke; the whole point of the actor is the event stream); incremental
updates on `append` (correct but couples the store to the index and needs
undo/compaction/delete deltas — the full rebuild is one scan after a change
and only when somebody searches, which is rare relative to turns; upgrade
when a rebuild is measured slow); a persistent FTS table (FTS is token
search, not fuzzy scoring — different product, and 10 has it for knowledge).

### 2.3 Off the mailbox

`Message<SearchSessions>` on `Agents` uses kameo's `DelegatedReply`
(the `Compact` / `Harvest` precedent): it clones the `Arc<Cache>` and the
`Store`, spawns the search, and returns at once. The supervisor keeps
draining events while a rebuild or a match runs.

## 3. Scoring and hits

`Index::search(query, limit)` (port of jarvis `conversation_search.rs`):

- `Pattern::parse(q, CaseMatching::Smart, Normalization::Smart)`; one
  `Matcher` (`Config::DEFAULT`); `pattern.indices()` per title and per
  message text — a score and the matched char indices, or no match.
- A title match gets `+32` (`TITLE_BOOST`) so an exact title outranks a
  weak body substring in the same band.
- Per session keep the best candidate; count matching messages.
- Sort: score desc, then `updated_at` desc, then id. Truncate to `limit`
  clamped to `1..=50` (`MAX_LIMIT`; default 30).
- Snippet: a 160-char (`SNIPPET_CHARS`) window centred on the matched span
  (or from the first match when the span is wider), `…` where cut; the
  matched indices re-based into the snippet and merged into
  `[start, end)` runs. **Char offsets**, not bytes or UTF-16 units — a
  frontend indexes `Array.from(snippet)` / `snippet.chars()`.

```
SearchHit { session, title, kind: title|message, seq?, role?: user|assistant,
            snippet, ranges: [{start, end}], score, matches, updated_at }
```

Empty or whitespace query → `[]` without touching the index.

## 4. Frontends

- **Desktop** (`SearchDialog.tsx`): `shouldFilter={false}` on cmdk; the
  typed text is debounced 150 ms into a `useQuery(['search', q])` with
  `keepPreviousData` so rows never flash empty; empty query shows the recent
  list as before. Each hit: title, then the snippet with `<mark>` over the
  ranges; the right column says `title`, the role, or `n matches`. Footer is
  the syntax hint. Enter opens `/c/$session`.
- **TUI**: `/search <query>` → `Menu::Search { query, hits, cursor }`, a
  picker shaped like `/sessions`: one row per hit, title · snippet with the
  ranges emphasised, Enter switches session, Esc closes. Bare `/search` is a
  usage hint.
- Slack / Signal: none — a chat channel is bound to one thread; there is no
  "switch session" affordance to search for (21 §4, 09 §4).

## 5. Decisions

1. **nucleo server-side over a TS fuzzy lib client-side.** One matcher, one
   ranking, TUI and desktop agree; message bodies never leave the daemon in
   bulk (the alternative ships every message to the webview to search it).
   Beat: `fuse.js` / cmdk scoring in the UI (titles only unless bodies are
   shipped), `fuzzy-matcher` (jarvis's TUI crate; older algorithm, no
   pattern syntax).
2. **User + assistant text only.** Beat: everything including tool results
   (2.2 MB more per build, and hits on log dumps drown the conversation
   hits). Opt-in later if wanted.
3. **Cached full rebuild with generation invalidation** over incremental
   maintenance and over per-query scans (§2.2).
4. **fzf syntax** (`Pattern::parse`) over plain fuzzy atoms (`Pattern::new`,
   what jarvis used): the user knows fzf/helix; `'exact` and `!not` are free.
5. **Best hit per session** over one row per matching message: the picker
   opens a session; `matches` tells how many rows agreed.

## 6. Checks

- `cargo test -p agent search::` — 7 unit tests: empty query, title boost
  and ranges, per-session collapse and counts, snippet windowing with
  ellipses, run merging, limit clamping, fzf syntax (`'exact`, `!not`).
- `cargo test -p rpc --features ts export_bindings` — `SearchHit`,
  `HitKind`, `HitRole`, `MatchRange` emitted; `search_sessions` in
  `methods.json`; the desktop `api.test.ts` drift test passes.
- Smoke: `search_sessions("cookie")` over the author's real store returns
  the cookie-import session first with a message snippet (§8).

## 7. Upgrades, when measured

- Jump to `seq` on open (load the page containing it; 25 §6.2 paging).
- Tool-result text behind a flag if users search for things only tools saw.
- Incremental index when a rebuild is measured slow (> ~100 ms on a real store).
- Restrict to a subtree / a date range (`SearchSessions` gains fields; the
  index is already per session).

## 8. Status (as-built)

Landed 2026-09-15: `crates/agent/src/search.rs`, `Store::search_titles` /
`search_bodies`, `SearchSessions` + supervisor handler and invalidation,
RPC `search_sessions`, bindings, desktop `SearchDialog`, TUI `/search`
(`Menu::Search`, line-printer arm).

Verified: the 7 unit tests and the bindings drift test; over WebSocket
against a copy of the author's store (debug build): `cookie` → the
cookie-import session first as a title hit with `ranges [8,14)`; `grafana`
→ the Grafana CI-gate session first (303 matching messages), then message
hits with the word marked in the snippet; `'pty` (exact) → the PTY session
first; a nonsense query still returns weak fuzzy hits (score 46 — fzf
behaviour, same as jarvis). Timings, debug build, 2387 texts / 6.4 MB:
first query 406 ms (includes the index build), then 195–344 ms per query,
all off the actor mailbox. Release is expected an order of magnitude
faster; not measured. The desktop dialog verified in the browser build
(five rows, 95 `<mark>`s, no console errors); the TUI verified through the
headless line printer (same ranking); the interactive picker compiled and
unit-tested, not exercised in a real terminal.

Debug-build matching at ~300 ms is the current ceiling; if release is not
~30 ms, pre-convert texts to `Utf32String` at build time and/or run the
doc loop across threads (nucleo's `Matcher` is cheap to clone per thread).

## 9. Superseded by 49 (2026-09-18)

What 49 did to this design, as built:

- **Kept.** The index (§2.1: titles plus visible user/assistant text, `cut_at`
  respected, `text_of`), the supervisor-owned `search::Cache` with generation
  invalidation (§2.2), delegated replies off the mailbox (§2.3), the scoring
  rules (§3: `Pattern::parse` smart case/normalisation, `TITLE_BOOST`, best
  hit per session, `matches`, 160-char snippets, char-offset ranges) and the
  `SearchHit` shape with its `context_removed` flag (48). `cargo test -p agent
  search::` still covers them.
- **Moved.** The nucleo pattern setup, prepared candidate text, `snippet`/run
  merging and `MatchRange` now live in the pure `search` library
  (`crates/search`), which agent, artifact and knowledge share; agent imports
  them and keeps only session records, the title boost and the per-session
  collapse. The limit contract is the shared one: default 10, clamped
  `1..=50` **per category** (v0's default 30 is gone).
- **Replaced.** The public RPC `search_sessions(query, limit?) -> [SearchHit]`
  is removed, not aliased: the desktop `Api`/`METHODS`, TUI executor, export
  list and bindings carry `search(query, categories?, limit?) ->
  SearchResponse`, whose `sessions` group is a `SearchPage<SearchHit>` with
  `state`/`has_more`/`coverage`/`notices`. The supervisor command is now
  `agent::SearchSessionPage { query, limit }` answering that page, plus
  `agent::SearchRefresh` behind `search_refresh`. A cold or stale cache answers
  `indexing` and starts one build instead of blocking the first query on it;
  one match runs at a time, a second concurrent one is refused, not queued.
- **Frontends.** `SearchDialog` is the four-category dialog of 49 §5, mounted
  at the root shell rather than inside the sidebar; the full-width Search
  chats row is gone and the entry is the header magnifier. TUI `/search` is
  the live menu of 49 §6 with its own `TextInput` and chips, not the
  submit-once picker of §4. A session message hit opens the read-only
  **Earlier messages** page at that `seq` (the §7 upgrade), in both UIs.
- **Not done.** Tool-result text, incremental index maintenance and
  subtree/date restriction (§7) remain unbuilt; nothing in 49 needed them.

Measured after the move (release, fixture of 1,003 sessions / 50,010
searchable messages): warm `search` p95 2.4 ms across all four groups, cold
first reply 1.7 ms as `indexing`, all groups ready after 4.0 s (49 §11). The
§8 debug-build ceiling is therefore obsolete; the `Utf32String`
pre-conversion it proposed is what `search::SearchText` does.
