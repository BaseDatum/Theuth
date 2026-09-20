# Meetings phase 5 — the brief cache: briefs on disk, searchable, shown before the model wakes

Status: phase design v0 · 2026-09-20 · extends 53 §4.6 (topics and briefs)
and §3.4 (prompt block); adds one variant to `BriefSource` and two optional
fields to `Brief` (README §3 allows additions). Depends on phase 2 (topics,
briefs, batch lines, `AddTopic`/`AddBrief`), phase 3 (island, control center,
sidebar) and phase 4 (post tools registered at plugin start — the same block
registers the cache tools). Runs after phase 4, before the integration pass.

Goal of the phase: the delay between *a subject comes up* (or *the user asks*)
and *a brief is on the island* is bounded by the actor, not by a model turn,
whenever the harness has seen the subject before. Briefs become a durable,
searchable store that any agent session can fill before a meeting and prune
when they go stale; the meeting's fast model consults it first and only
researches what is not there.

## 0. Decisions in one screen

1. **Briefs live in their own SQLite file, and SQLite is the truth.**
   `<data_dir>/meetings/briefs.sqlite`, one table + one FTS5 mirror (§5).
   Unlike `meetings.sqlite` (derived from manifests) and knowledge (derived
   from files), a cached brief has no other home: it is agent- or
   user-authored, small, and expendable. Rejected: a third migration in
   `meetings.sqlite` (the user asked for a separate db, and the cache
   outlives any meeting); JSON files + derived index (a second copy of
   knowledge's machinery for rows nobody edits by hand).
2. **The hot path is an alias scan in the actor — no model, no FTS.** Every
   cached brief carries `aliases: Vec<String>` (how people *say* it: `SRE-2386`,
   `the DR cutover`, `Dana`). The actor holds them in memory and scans every
   closed utterance, every `[meeting request]` text and the start form's
   title + context (case-insensitive, word-boundary on both ends, alias
   ≥ 3 chars or containing a digit). A hit → `TopicAdded` with the cached
   brief attached, or for a request, `Brief{source: Request}` — published
   within milliseconds of the utterance closing. Hundreds of aliases × one
   `find` each per utterance is nothing; `ponytail:` Aho-Corasick if the
   cache grows past ~5 000 aliases. Rejected: FTS on free text in the hot
   path (a wrong brief on the island mid-meeting is worse than a late one;
   fuzzy matching is the model's job, §0.4).
3. **Everything the model authors during a meeting goes into the cache.**
   `meeting_topic` writes the brief to the cache with the label as its first
   alias; the next meeting that says the label gets the brief for free.
   `meeting_brief` caches only when the model passes `aliases` (a `Deeper` or
   `Request` answer is usually too specific to be a cache row). Rejected: a
   separate "save this" step (the model never remembers to).
4. **The model reuses by reference and learns aliases.** `meeting_topic`
   gains `brief: Option<BriefId>` as an alternative to `title`/`bullets`:
   when `brief_search` finds a cached brief for a subject the alias scan
   missed (paraphrase: *"the failover thing"*), the model registers the topic
   by id — no bullets rewritten, no knowledge lookup — and the actor appends
   the spoken label to the row's aliases. The cache gets better at the hot
   path by being used.
5. **Four general tools in every session; put/delete are `ReadOnly`.**
   `brief_search`, `brief_put`, `brief_delete`, `brief_list` are registered at
   plugin start like the post tools. `brief_put`/`brief_delete` are
   `Risk::ReadOnly` on the `knowledge_put` argument (`knowledge/src/tools.rs:229-256`:
   asking on every fact would stop the flow) plus this one: a cache row is
   not world state — a meeting's own `briefs.jsonl` keeps the record of what
   was shown, knowledge keeps the facts, and a deleted row costs one
   research turn to recreate. Rejected: `Destructive` delete (as knowledge's)
   — a stale brief must be evictable mid-meeting without an approval card.
6. **Prefill is a conversation, not a feature.** The user tells any session
   *"I have a meeting about x, y, z"*; the prompt block tells the model to
   research (knowledge first) and `brief_put` one brief per subject with
   aliases as they will be spoken and a `tag` for the meeting. Nothing new
   on the daemon beyond the tools; the `/briefs` page (§7) shows the result.
   Rejected: a "prepare meeting" form/RPC (a template for what the chat
   already does better).
7. **Staleness is visible, not automatic.** Rows carry `updated_at`, `uses`,
   `last_used_at`; the batch line marks cached topics with their age
   (`live: SRE-2386 (cached 3d)`); the prompt tells the model to `brief_put`
   over the same id when it knows better, and the `/briefs` page sorts by
   age with a delete. Rejected: TTL expiry (a ticket brief is right until the
   ticket moves, not until Tuesday).
8. **Cache changes are one event, `BriefEvent::Changed { id, removed }`.**
   Not a `MeetingEvent` (those carry a `meeting`; the cache has none). The
   bridge forwards it; the desktop invalidates `briefs` queries.

## 1. Files

```
crates/meeting/src/cache.rs            Cache { open, put, delete, get, list, search, aliases() }, Alias scan (pure fn, unit-tested)
crates/meeting/migrations/briefs/0001_briefs.sql   briefs + briefs_fts (sqlx::migrate!("./migrations/briefs"))
crates/meeting/src/tools.rs            + BriefSearchTool, BriefPutTool, BriefDeleteTool, BriefListTool; PROMPT gains §6; TopicArgs.brief
crates/meeting/src/topics.rs           + cached-id bookkeeping (shown/dismissed per meeting)
crates/meeting/src/session.rs          batch_line: `(cached Nd)` suffix; request_line: `(cached brief "…" shown)` suffix
crates/meeting/src/lib.rs              BriefEvent, BriefSource::Cache, Brief.cached/aliases, hot path in closed()/TypedRequest/PushToTalk/StartMeeting, brief_* RPC messages
crates/rpc/src/{api.rs,bridge.rs,export.rs}   §8
crates/desktop/ui/src/routes/briefs.tsx, components/briefs/{BriefSearch,BriefCard,BriefEditor}.tsx, store/meeting.ts, rpc/{api,events}.ts, lib/rpc.ts, sidebar
```

## 2. Data model

```rust
// cache.rs — CachedBrief is the row; Brief (README §3) is a shown instance.
pub struct CachedBrief {
  pub id: BriefId,                 // v7; stable across updates
  pub title: String,
  pub bullets: Vec<String>,        // 2..=3, same validation as topics::check_bullets
  pub aliases: Vec<String>,        // ≥ 1; each ≤ 4 words (label rule); first = display label
  pub tags: Vec<String>,           // free; the prefill conversation uses one per meeting
  pub source: String,              // "meeting:<id>" | "session:<id>" | "user"
  pub created_at: String, pub updated_at: String,   // RFC 3339
  pub uses: u32, pub last_used_at: Option<String>,
}
```

README §3 additions (serde defaults keep phase 1–4 bindings valid):
`BriefSource::Cache`; `Brief { …, cached: Option<BriefId> /* the row this
instance came from */, }`. A `Brief` shown from the cache has a fresh `id`
(it is an instance in this meeting's `briefs.jsonl`) and `cached: Some(row)`.

## 3. Hot path (actor)

`Cache::aliases() -> Arc<Vec<(String /* lowercased alias */, BriefId)>>`,
rebuilt on every `put`/`delete` (the cache is small; a rebuild is a `SELECT`).
`cache::scan(text, &aliases) -> Vec<(BriefId, &str /* alias as matched */)>`
is pure: lowercase `text`, for each alias `find` with word boundaries
(`!is_alphanumeric` or edge on both sides), dedupe by id, first occurrence
order.

Where it runs, all before any model involvement:

- **`closed(u)`** (`lib.rs:969-982`), before `batcher.feed`: hits whose row is
  not already shown or dismissed in this meeting (`Topics.cached_shown`,
  `Topics.cached_dismissed: Vec<BriefId>`) → `Topics::insert_cached(alias,
  row)` (same label validation, same `max_topics`/expire rules; the label is
  the alias as spoken) → `TopicAdded { topic{ brief: Some(instance) } }`;
  the precomputed `Brief { source: Cache, cached: Some(row) }` is logged and
  stored exactly as an `Auto` one, so tap shows it instantly. `uses += 1`,
  `last_used_at = now`. Dismiss of a cached topic → `cached_dismissed` (not
  re-shown this meeting; the label goes to `dismissed` as today).
- **Requests** (`closed` with `request`, `TypedRequest`): scan the request
  text; the first hit → `show_brief(Brief { source: Request, cached })` at
  once; the request line to the model gains ` (cached brief "<title>"
  already shown — add only what it lacks)`. No hit → unchanged.
- **`StartMeeting`**: scan `title + " " + context` → cached topics published
  right after `Started`, before the `started.` line reaches the model.
- **Tap** on a cached topic: unchanged path (the brief is precomputed);
  `[meeting tapped]` still goes to the model, which does nothing if the
  brief exists (prompt).

Batch line (`session::batch_line`): cached live labels render as
`<label> (cached <age>)` where age is `Nd`/`Nh`/`Nm` from `updated_at`.
`dismissed:` unchanged.

Latency budget: utterance close → `TopicAdded` published < 20 ms in the
actor (a `Vec` scan + a jsonl append + one sqlite `UPDATE`, off the hot
path via `tokio::spawn`); island paint is the RPC + React hop, ≈ 100 ms.

## 4. Model path

`meeting_topic` (`TopicArgs`): `label`, and **either** `title` + `bullets`
**or** `brief: BriefId`. With `brief`: the actor resolves the row (unknown →
`ToolOutput::err("no cached brief <id>")`), inserts the topic with the row's
brief (`source: Cache`), appends `label` to the row's `aliases` if new (≤ 4
words; `updated_at` untouched — an alias is not a content change), bumps
`uses`. Without: today's path, then `Cache::put(CachedBrief { aliases:
[label], source: "meeting:<id>", .. })` — so a meeting-authored brief is
cached the moment it is shown. `meeting_brief` (`BriefArgs`) gains
`aliases: Option<Vec<String>>`; `Some` → also cached.

The four general tools (`tools.rs`, registered in `register_post`'s block,
every session):

| tool | args | risk | returns |
|---|---|---|---|
| `brief_search` | `{ query, limit? (≤ 20, default 8) }` | ReadOnly | lines `` `<id>` — <title> [<aliases, joined>] (<age>, used N×): <bullets joined by " · ">``; `structured` = the rows; `"no matches"` |
| `brief_put` | `{ id?, title, bullets, aliases, tags? }` | ReadOnly (§0.5) | `"put <id> (new|updated)"`; validation as §2; `source` = `"session:<cx.session>"` unless the row exists (kept) |
| `brief_delete` | `{ id }` | ReadOnly (§0.5) | `"deleted <id>"`; unknown → err |
| `brief_list` | `{ tag?, limit? (≤ 50) }` | ReadOnly | newest `updated_at` first, same line format |

`brief_search` query: `fts_query`-style quoting of each word, joined by
` OR `, ranked by `bm25(briefs_fts, 10.0 /*aliases*/, 5.0 /*title*/, 2.0
/*tags*/, 1.0 /*bullets*/)` then `updated_at DESC`. A single-token query that
equals an alias (case-insensitive) is returned first regardless of rank.

## 5. Storage (`briefs.sqlite`)

```sql
CREATE TABLE briefs (
  id TEXT PRIMARY KEY, title TEXT NOT NULL, bullets TEXT NOT NULL /* json */,
  aliases TEXT NOT NULL /* json */, tags TEXT NOT NULL /* json */,
  source TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
  uses INTEGER NOT NULL DEFAULT 0, last_used_at TEXT);
CREATE VIRTUAL TABLE briefs_fts USING fts5(id UNINDEXED, aliases, title, tags, bullets,
  tokenize = 'porter unicode61 remove_diacritics 2');
```

`Cache::open(data_dir)` = `store.rs:231-259`'s connect options
(`create_if_missing`, WAL, `busy_timeout` 5 s, 2 connections) +
`sqlx::migrate!("./migrations/briefs")`. `put` is one transaction: upsert
`briefs`, `DELETE FROM briefs_fts WHERE id = ?`, `INSERT` (the phase-4
`index_transcript` shape). `aliases` and `tags` are stored as JSON arrays and
indexed in FTS as space-joined text. `Cache` lives on `Meeting` as
`Arc<Cache>` (the tools hold the same `Arc`); the alias list is an
`ArcSwap`-free `RwLock<Arc<Vec<_>>>` rebuilt on write (writes are rare).

## 6. Prompt block (replaces phase 2's; `RegisterPrompt` swaps the block)

Appended to the existing paragraph (`tools.rs:22-40`), one paragraph:

> Briefs are cached: a `live:` topic marked `(cached …)` already has a brief
> on the user's screen — do not research or re-register it; if you know it
> is wrong or out of date, `brief_put` over the same id. For a new subject,
> call `brief_search` first; a hit → `meeting_topic` with its `brief` id and
> the label as spoken. Only when nothing is cached, look it up (knowledge
> first) and `meeting_topic` with title and bullets — it is cached for next
> time. Outside a meeting: when the user says a meeting is coming up,
> research each subject and `brief_put` one brief per subject with `aliases`
> as people will say them (ticket ids, product names, first names) and a
> `tag` for the meeting; when the user says a brief is stale or wrong,
> `brief_put` the fix or `brief_delete` it.

## 7. UI

- **`/briefs`** (`routes/briefs.tsx`; sidebar link *Briefs* under
  *Meetings*): `components/briefs/BriefSearch.tsx` = the knowledge
  `SearchBox.tsx` pattern (200 ms debounce, `brief_search`, ≥ 2 chars; empty
  query shows `brief_list` newest first with a tag filter chip row);
  `BriefCard.tsx` (title, bullets, alias chips, tag chips, `updated_at` age,
  `uses`, *edit*, *delete*); `BriefEditor.tsx` (title, bullets 2–3, aliases,
  tags → `brief_put`; also *New brief*). `BriefEvent::Changed` → invalidate
  `queryKeys.briefs*`.
- **Island**: a `Cache` brief renders like `Auto` (stack rules unchanged);
  the topic button shows a small dot for cached topics (the brief is
  already there — the tap is instant).
- **Control center brief log**: header shows `cached` for `source ===
  'Cache'` and links the row on `/briefs`.
- **Chat**: nothing — prefill is ordinary tool use; the `MeetingCard`
  (phase 3) is untouched.

## 8. RPC and events

`brief_search(query, limit?) → Vec<CachedBrief>`, `brief_list(tag?, limit?)
→ Vec<CachedBrief>`, `brief_get(id) → CachedBrief`, `brief_put(CachedBriefPut
{ id?, title, bullets, aliases, tags }) → CachedBrief`, `brief_delete(id) →
()`. Actor messages of the same names (`ask`), the tools and RPC both go
through the actor so the alias list and the live meeting's bookkeeping stay
consistent. `BriefEvent` (`#[serde(tag = "type")]`, `Changed { id: BriefId,
removed: bool }`) → `bridge.rs` `forward!` + subscribe; `export.rs` gains
`CachedBrief, CachedBriefPut, BriefEvent`; `WireEvent` gains `BriefEvent`.

## 9. Not in this phase

Automatic expiry (§0.7). Sharing briefs between harnesses / export. Semantic
(embedding) matching in the hot path. Joining 49's unified search. A prefill
form. TUI surfaces (53 is desktop-only; the tools work from the TUI already).

## 10. Check

1. `cargo test -p meeting`: `cache::scan` (word boundaries — `DR` does not
   match `drive`; digits; case; multiple hits in order; dedupe), `Cache`
   put/update/delete/search round trip on a temp db (bm25 order: alias hit
   beats bullet hit; exact-alias first), `Topics::insert_cached` respects
   `max_topics`/dismissed, `batch_line` renders `(cached 3d)`.
2. Headless, phase-2 setup (two-voice clip mentioning `SRE-2386` and KEDA),
   cache empty: first meeting → the model's `meeting_topic` for SRE-2386
   lands a row (`brief_list` shows it, `source: meeting:<id>`, alias
   `SRE-2386`). Stop. Second meeting, same clip: `TopicAdded` for SRE-2386
   arrives **< 1 s after the utterance closes** (log `utterance.t1 → publish`
   in the actor; expect < 20 ms) and before any `TurnStarted`; the batch line
   shows `SRE-2386 (cached …)`; the model does not re-register it (no second
   `meeting_topic` for that label across the meeting); `uses` is 1.
3. In a plain chat session: *"I have a meeting tomorrow about the DR cutover
   and KEDA scaling with Dana"* → the model calls `brief_search`, knowledge
   tools, and `brief_put` ×3 with aliases (`DR cutover`, `KEDA`, `Dana`) and
   one tag; `/briefs` lists them under the tag. Then *"the KEDA one is
   wrong, the limit is 40k now"* → `brief_put` over the same id
   (`updated_at` moves, `uses` kept). *"drop the Dana brief"* →
   `brief_delete`; `BriefEvent{removed: true}`; the page updates without
   reload.
4. Start a meeting with context *"DR cutover planning"* → the DR topic is on
   the island before the first batch (`TopicAdded` precedes the first
   `TurnStarted`).
5. `meeting_ptt` / `meeting_request("what's the KEDA limit")` → the cached
   KEDA brief is shown as `Brief{source: Request, cached}` immediately
   (< 1 s after release / call); the model's steer carries the `(cached brief
   … already shown)` suffix and the model adds nothing or a `Deeper`-style
   supplement only.
6. Paraphrase: the clip says *"the failover thing"* → alias scan misses; the
   model's `brief_search("failover")` hits `DR cutover`; `meeting_topic {
   label: "failover thing", brief: <id> }` → `TopicAdded` with the cached
   brief; `brief_get` shows the new alias; the next meeting matches
   *"failover thing"* in the actor.
7. Dismiss a cached topic → not re-added in this meeting although the alias
   recurs; a new meeting adds it again.
8. `/briefs`: search, edit bullets, add an alias, delete; the island
   receives nothing (no meeting); with a meeting live, editing a cached
   brief that is on the island re-publishes it (the island shows the new
   bullets).
9. `briefs.sqlite` deleted → recreated empty at start; the meeting still
   works (no aliases, model path only).

## 11. Status

Implemented 2026-09-20. `crates/meeting` +≈1 100 lines incl. tests (`cache.rs`,
`migrations/briefs/0001_briefs.sql`, `topics.rs` cached bookkeeping,
`session.rs` age/labels/request suffix, `tools.rs` four brief tools + prompt
paragraph + `TopicArgs.brief`/`BriefArgs.aliases`, actor hot path and six
messages); `crates/rpc` six methods + three wire types + bridge forward;
desktop `routes/briefs.tsx`, `components/briefs/{BriefSearch,BriefCard,BriefEditor}.tsx`,
sidebar link, island dot, control-center `cached` link, store/rpc/events/api
wiring. Verified headless over JSON-RPC on `/tmp/wth-meet` (the phase-2/3/4
scratch dir; `RUST_LOG=info,meeting=debug`, `ORT_DYLIB_PATH` at the TDT
dylib, `WTH_SECRETS__KEY_STORE=file`), macOS 15, `anthropic/claude-haiku-4-5`,
the phase-2 knowledge bundle, QuickTime Player playing the 114 s two-voice
`say` clip, the external mic live (speakers, no headphones — the mic track
carries the far end, 01 §9 check 5), a debug build. The user spoke for the
mic track in meeting 2.

### Exercised

1. `cargo test -p meeting` 32/32 (+3: `cache::scan_rules` — `DR`/`dr` never
   match `drive` (too short, no boundary), `kedab` no boundary, digits
   (`2386`), case (`sre-2386` as spoken), first-occurrence order, one hit per
   row across two aliases, a failed boundary on the first occurrence does not
   hide the second; `cache::round_trip` on a temp db — validation (no alias,
   5-word alias, 1 bullet, blank title), case-insensitive alias dedupe,
   update keeps `source`/`created_at`/`uses` and moves `updated_at`, alias
   hit ranks above a bullet hit, single-word exact alias first, `add_alias`
   once and indexed with `updated_at` untouched, list by tag newest first,
   `tags()`, delete removes row/FTS/aliases; `topics::insert_cached_rules` —
   `source: Cache` + `cached` on topic and brief, blocked once shown (also
   under a second alias), `max` expiry, dismissed row barred, a fresh
   `Topics` admits it, `refresh_cached` rewrites bullets and names the live
   topic, an Auto topic written to the cache blocks the scan too;
   `session` — `age` 3d/2h/11m/0m/`?`, `SRE-2386 (cached 3d)` in the batch
   header, the request suffix). `cargo check --workspace --all-targets`,
   `cargo clippy -p meeting -p rpc -p wth --all-targets --no-deps -D warnings`,
   `cargo check -p rpc --features ts`, `just ts-desktop` clean; `bun run check`
   clean; vitest 34 files / 185 (one pre-existing `link/pairing.test.ts`
   timing flake under the full parallel run, green alone and on rerun).
2. Meeting 1 (cache empty): the model's three `meeting_topic` calls each
   produced `BriefChanged` + `TopicAdded{cached}`; `brief_list` showed three
   rows, `source: meeting:<id>`, alias = the label. **Labels were not the spoken
   tokens** (*DR cutover blockers*, *KEDA scaling quota hits*, *Pulumi state
   migration*) and the streaming STT renders the ticket as `SRE2386` and KEDA
   as *Gida*, so meeting 1 did not yield alias `SRE-2386`; the aliases meeting
   2 hit on came from the check-3 conversation. Meeting 2, same clip:
   `Utterance{final}` *"…First item is the DR cutover"* at 16:16:52.769 →
   `TopicAdded{label: "DR cutover", cached}` at 16:16:52.770 on the wire
   (**1 ms**), before the meeting's first batch `TurnStarted` (12.8 s later);
   actor log `since_t1_us=130115` for that hit — measured from `Piece.at`
   (the closing chunk's receipt stamp), so it includes the Nemotron flush
   window and the mailbox hop; the actor's own share, measured from entry on
   the rebuilt binary (meeting 3, two hits): **0.69 ms and 0.94 ms** each
   incl. the sqlite `get`. The batch line read `live: DR cutover (cached 2m)`;
   the model never re-registered it (its only `meeting_topic` of the meeting
   was *Pulumi state migration*, from the clip's last line); `uses` on the row
   is 1.
3. Plain session over RPC (`new_session` + `send_input`): *"I have a meeting
   tomorrow about the DR cutover and KEDA scaling with Dana. Get briefs ready
   for it."* → `brief_search`×2, `knowledge_get`×3, `brief_put`×3 with
   `tags: ["dana-meeting"]` and aliases added; no Dana brief (nothing in
   knowledge — the model asked). *"Dana is the platform SRE lead …"* +
   a request for bare spoken aliases → `brief_put` Dana (new) and re-put the
   other two with aliases `SRE-2386`/`DR cutover` and `KEDA`. *"the KEDA one
   is wrong, the limit is 40k now"* → `brief_put` over the same id
   (`updated_at` 16:14:23 → 16:15:09, `uses` kept). *"drop the Pulumi brief"*
   → `brief_delete` → `BriefChanged{removed: true}`. The `/briefs` page
   itself was not opened (item 8 below).
4. Meeting 3, `context: "DR cutover planning with Dana"`: `TopicAdded` ×2
   (*DR cutover*, *Dana*) at 16:23:24.842, the `started.` `TurnStarted` at
   .845 — 3 ms later, before any batch.
5. Typed `meeting_request("what is the KEDA limit")` → `Brief{source:
   Request, cached: <KEDA row>}` at 16:17:47.167, `TurnStarted` with
   `[meeting request, typed] … (cached brief "KEDA autoscaling for ingest
   workers" already shown — add only what it lacks)` at .169; the model
   replied in text, was nudged (03 §9), then `meeting_brief` → `Brief{Typed}`
   supplement. Spoken: `meeting_ptt(true)`, the user said *"Who is Dana"*,
   `meeting_ptt(false)` → `Utterance{request: true}` at 16:21:28.422,
   `Brief{source: Request, cached: <Dana row>}` at .423, the steer with the
   suffix at .424; the model added a `Request` supplement 2.8 s later.
6. **Half.** The user said *"We should ask the platform SRE lead about the
   runbook"* (STT: *"platform as a lead"*); the model's batch turn did nothing
   (no `brief_search`). Driven with an explicit instruction in the meeting
   session, `meeting_topic { label: "failover thing", brief: <Dana id> }` →
   `TopicAdded{cached}` in 1 ms, `topic registered with the cached brief`,
   `brief_get` shows aliases `["Dana", "failover thing"]`, `uses` 2. The
   by-reference handler and alias learning are exercised; the model doing it
   unprompted from a paraphrase is not.
7. `meeting_dismiss` on the cached *DR cutover* → `TopicRemoved{Dismissed}`,
   `dismissed: DR cutover` in the next batch line. The alias did not recur in
   the clip and the user's *"The DR cutover is still on"* came through STT as
   *"DUR cutover"*, so the not-re-added half rests on the unit test; the next
   meeting (3) added it again.
8. **Daemon side only.** Meeting 4 with `context: "Dana"` → cached topic;
   `brief_put` over the Dana row via RPC → `BriefChanged` + `Brief{source:
   Cache, topic: <live topic>, cached}` carrying the new bullets, 0 ms apart.
   The `/briefs` page, the island dot and the control-center link were type-
   checked and unit-tested (store rules) but not opened in the `.app`; that
   run is the user's: sidebar → Briefs → search `KEDA`, edit bullets, add an
   alias, delete one; with a meeting live, edit a brief whose topic is on the
   island and watch the pane update.
9. Fresh data dir state: `briefs.sqlite` did not exist before this build;
   the plugin created it (4 KB, migrated) at start and `brief_list` answered
   `[]`; the whole of meeting 1 ran on the model path. The corrupt-file case
   (`Cache::open` → `Err` → `cache: None`, tools not registered, RPC says
   "the brief cache is unavailable") is code-path only.

### Not exercised

- The `/briefs` route in a browser or the `.app` (item 8); the island's dot
  and the `Cache` stack rule on a real island (unit-tested in
  `store/meeting.test.ts` only through the reducer).
- Check 6 as written (the model spontaneously `brief_search`ing a paraphrase);
  check 7's not-re-added half on the mic (STT).
- `cache::scan` on non-ASCII text where lowercasing changes byte length (the
  alias is returned in stored case instead of as spoken — see deviation 9).
- `LIST_CAP`/`limit` clamping over the wire; `brief_tags` ordering with ties;
  a `brief_put` while the same row's topic is live *and* already viewed.

### Deviations from the design above

1. **Wire tag `BriefChanged`, not `Changed`.** `ScheduleEvent` already emits
   `{"type":"Changed"}` and the desktop dispatches every wire event by that
   field alone (`on('Changed', …)` in `Schedules.tsx`), so the Rust variant
   stays `BriefEvent::Changed` with `#[serde(rename = "BriefChanged")]`.
2. **`Topic.cached: Option<BriefId>`** (`#[serde(default)]`) added beside
   `Brief.cached`: the island's dot needs to know a button is backed by a
   cache row, and the precomputed brief is not on the island until tapped.
   README §3 allows the addition; `Brief`/`Topic` literals in the UI tests
   and fixture gained `cached: null`.
3. **Everything goes through the actor.** The four brief tools hold
   `ActorRef<Meeting>` (like the live pair), not `Arc<Cache>`; §5's "the tools
   hold the same `Arc`" is the actor holding it. One code path for tools and
   RPC, one place that publishes `BriefEvent` and re-publishes a live brief.
4. **`cache: Option<Arc<Cache>>` on `Meeting`.** A `briefs.sqlite` that will
   not open logs a warning and disables the hot path and the brief tools; the
   recorder is not held hostage by its cache (§0.1's "expendable").
5. **A meeting-authored brief is written to the cache *before* the topic is
   inserted**, so the row id rides on `Topic.cached`/`Brief.cached` and the
   row is in `cached_shown` — otherwise the alias scan would re-add the same
   subject as a `Cache` topic the moment the label is spoken again after the
   button was viewed. `Topics::admit` (label + duplicate check) runs first so
   a refused topic leaves no row; a cache failure only logs (the topic still
   registers, `cached: None`).
6. **`brief_tags` RPC** (`Vec<String>`, most-used first) for the `/briefs`
   chip row; §8 listed five methods. `brief_list`'s tag filter is
   `json_each(tags)` in SQL.
7. **`brief_put`'s live re-publish** covers every instance in the meeting
   whose `cached` is the row (title and bullets rewritten in place so a later
   tap or deeper sees the new text) and publishes one `Brief{source: Cache,
   topic: <the live topic if any>}`; §8 said "whose topic is currently live"
   — a request-shown cached brief with no topic is refreshed the same way.
8. **The request path shows a cached brief regardless of `cached_shown`**
   (an explicit question re-shows even a subject already on the island as a
   topic) and does not mark the row shown; only topics do. `Brief.source` is
   `Request` for typed requests too, as §3 specifies.
9. **`scan` returns `Vec<(BriefId, String)>`**, the alias as it appears in
   `text` (the label "as spoken", e.g. `sre-2386`); when lowercasing changed
   the byte length the stored alias is returned instead. `topics::place`
   normalises whitespace; `check_label` still applies (an alias is ≤ 4 words
   by `put`). Aliases shorter than 3 chars without a digit are skipped in
   `scan`, not refused by `put` (`DR` may be stored, it is just never
   scanned — say `the DR cutover`).
10. **Latency log** is two fields, `since_t1_us` (from `Piece.at`: includes
    the STT flush) and `actor_us` (from `cached_topics` entry); §3's "< 20 ms
    in the actor" is the second number (≈1 ms measured); the first is
    ≈130 ms on this machine and is the streaming model's, not the actor's.
11. **`SRE-2386` is not what the streaming STT hears.** Nemotron writes
    `SRE2386` (no hyphen) and `Gida` for KEDA in the clip, and haiku's labels
    were not the spoken tokens, so a meeting-authored alias rarely matches
    the next meeting's transcript on its own; the prefill conversation
    (bare spoken aliases) is what made the hot path fire. Worth a phase-6
    note: `put` could also store a punctuation-stripped variant of each alias
    (`SRE2386`), one line in `clean`. Not done here (scope).
12. **`meeting_topic { brief }` was first called with a knowledge id**
    (`brief: "sre-2386"`, meeting 1) — a UUID parse error the model recovered
    from by looking the ticket up. The argument doc now says "the UUID `id`
    as returned by `brief_search` (not a knowledge id)".
13. `CachedBriefPut` is the `brief_put` tool's `Args` directly (as knowledge's
    `Put`); `brief_search`/`brief_list` set `RETURNS = "list[dict]"` and fill
    `structured` with the rows, lines formatted as §4 plus a trailing
    ` #tag` list when tags exist.
14. **Briefs are a tab of `/meetings`, not a sidebar entry** (user call after
    the first build): route `/meetings/briefs`, `MeetingTabs` in the shared
    `MeetingHeader` (Meetings | Briefs), the control-center `cached` link
    points there; §7's `/briefs` and the sidebar item are gone.
