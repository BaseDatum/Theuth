# 10 — Knowledge: an OKF bundle the agent builds as it works

Status: as-built v0 · 2026-09-10 · extends `07-context.md` (frames, notes,
compaction), `08-repl.md` §3 (runtime tool exposure), `04-prompt-skills-mcp.md`
§1 (prompt assembly), `01-config-and-subagents.md` §1.1. Where this doc
disagrees with those, this doc wins; §15 lists the edits (applied).

The knowledge base is a directory of markdown files in Google's Open Knowledge
Format (OKF v0.1) under `<data_dir>/knowledge/`. The agent reads it by id,
searches it, and — the part jarvis never got — **writes it while the user
works**, without a single byte of the cached prefix changing.

## 0. Decisions in one screen

1. **Format = OKF v0.1, files are the truth.** One bundle at
   `<data_dir>/knowledge/`; a concept is `<id>.md` with YAML frontmatter; the
   `okf` crate (0.1.0-alpha.1, zero deps, Apache-2.0) parses, serialises,
   validates, resolves links and regenerates `index.md`. Any editor is a UI.
   The bundle is a git repository kept **in-process by `gix`** (gitoxide,
   pure Rust, no network features): every write is a commit, history and
   revert are tools, and no `git` binary is needed — while a user who has
   one can `git log`/`push` the same `.git`.
2. **Named by function.** Crate `knowledge` (a plugin, 12 §9; the harvest
   *model call* stays in the agent loop, which owns the frame), directory
   `knowledge/`, tools and RPC `knowledge_*`, config `knowledge:` (the store:
   `enabled`) and `agent.knowledge` (the loop's use of it: hint and harvest
   cadence). "OKF" names the file format and nothing else (00 §1.1).
3. **Retrieval = SQLite FTS5.** A derived index in `<data_dir>/knowledge.sqlite`
   (bm25 over title/description/tags/body, porter stemming, a scope boost for
   the current directory), rebuilt from the files, resynced by mtime at every
   turn start. Delete the file and it rebuilds. Not jarvis's token-overlap
   ranker, not embeddings (§14).
4. **Hints are tail notes, never prompt text.** A `<knowledge>` note (07 §8.7,
   third kind) lists matching ids — id, title, type, one-line description —
   at turn start against the user's input and every `hint_rounds = 4` rounds
   against what was appended since. Each id is hinted once per epoch. Nothing
   when nothing matches. Zero cache cost by construction. The knowledge index
   is **not** in the system prompt: it changes every few turns and would
   rebase every turn (§4.3).
5. **Progressive building = harvest.** After every `harvest_every = 3`
   tool-using turns of a root session, and always right before a compaction
   cut, the harness sends the finished frame plus one instruction — the
   summariser's shape (07 §8.3), cache-hot — asking for durable knowledge as
   OKF documents. The harness parses, validates and writes them; existing
   related concepts are listed so the model updates instead of duplicating.
   No tool loop, no approval, no new model, no prefix change. The model may
   also `knowledge_put` at any time.
6. **Harness truth crosses the boundary.** The compaction head gains one
   line: `Knowledge: read a, b` — the ids fetched this epoch.
7. **Surfaces.** Tools `knowledge_search/get/list/put/delete/history/restore`;
   RPC the same names plus `knowledge_harvest`; events `KnowledgeChanged`,
   `Harvested`; TUI `/knowledge [query | history [id] | restore <id> [commit]
   | harvest]`. No graph view, no editor: the files are the editor.
   Human fuzzy search over current concepts and their complete history is the
   unified `search` surface of [49](49-search.md) (RPC + `search` tool +
   `/search`), fed by knowledge-owned `SearchConcepts`/`SearchHistory`; it
   reads the FTS rows as stored data and leaves `knowledge_search`/hints
   (§3.3) unchanged.

## 1. What jarvis does, and where it breaks

Verified against `crates/jarvis-okf/src/{store,rank,service}.rs`,
`jarvis-agent/src/{epoch.rs:1054-1078,tools/okf.rs}`, `prompts/system.md:455-493`,
`docs/design/okf-knowledge.md`.

| jarvis | effect | here |
|---|---|---|
| `{{okf_hints}}` rendered into `system.md` per run (`epoch.rs:215,341`) | the system prompt differs between turns whenever the hints differ → a prefix miss on every turn that has hints; `"(none)"` is injected when there are none | tail note, only when there are hits (§4) |
| Hint query = user text only (`load_okf_hints_block(core, input.user_text)`) | a concept about a path the model is *about to read* is never surfaced mid-turn | turn start on user text; mid-turn on assistant text + tool arguments (§4.2) |
| `rank.rs`: token overlap with fixed boosts, body prefix 2 KiB, no stemming, no idf | "the file" matches everything; "toolchains" misses "toolchain" | FTS5 bm25 + porter, ≥2-term rule, scope boost (§3) |
| Writes only on explicit `okf_put`; the doc's own risk table opens with "Empty catalog → seed + CTA" | the catalog stays empty; nothing is learnt from a session unless the model decides to file it | harvest: the harness asks at boundaries the frame is already paying for (§5) |
| Whole bundle in memory (`Index { metas, bodies, extras, edges }`), `Bundle::load` walk on every put | fine at 100 concepts; the model pays nothing, the daemon pays RSS and a walk per write | files + FTS rows; a put touches one file, one row, one `index.md` |
| External edits invisible until the next put or boot | a user fixing a concept in their editor is not seen | mtime/size resync at every turn start, like the skills index (04 §1.1) |
| Project scoping "Phase D — later" | a fact about repo A is hinted while working in repo B | `scope` frontmatter key + boost (§2.3) |
| Kept: hard-facts-only, ids in hints and `okf_get` before relying, own ranker, filesystem truth, reserved `index.md`/`log.md`, atomic writes, alpha crate isolated behind one module | | all kept |

jarvis also pairs OKF with a "soft" memory (Hindsight retain/recall). wth has
no soft memory and this doc adds none: the compaction summary's `## Notes`
carries soft context inside a session, and anything durable is a concept.

## 2. The bundle

### 2.1 Layout

```
~/.wth/knowledge/
  index.md                      generated (okf::index::regenerate_indexes) after every write
  log.md                        appended per write: "- put facts/wth-toolchain" under today's date
  .git/                         created by Repo::open (gix init, isolated options); one commit per write, committer "wth <wth@localhost>"
  facts/wth-toolchain.md        a concept; id = facts/wth-toolchain
  entities/grafana-dev.md
  decisions/std-mutex-not-parking-lot.md
  procedures/signal-link.md
  projects/wth.md
```

Directories are free; the harvest instruction (§5.3) and the guidance sentence
suggest one directory per type. Ids are OKF `ConceptId`s: `/`-separated
segments, `[a-z0-9._-]`, no `..`, no leading `/`, never `index` or `log` as a
segment (`okf::RESERVED_FILENAMES`). The harness (`store::concept_id`)
additionally trims, lowercases, strips a leading `/` and a trailing `.md`, and
refuses ids with more than three segments (deep trees hide concepts from
`knowledge_list` prefixes).

### 2.2 A concept

```markdown
---
type: fact
title: wth's Rust toolchain lives under rustup, not on PATH
description: cargo/rustc are at ~/.rustup/toolchains/stable-aarch64-apple-darwin/bin; homebrew's rustup does not proxy them
tags: [rust, toolchain, wth]
scope: /Users/Sky.Moore/Develop/wth
source: harvest
timestamp: 2026-09-10T09:41:03Z
---
`which cargo` fails in a fresh shell; the justfile prepends the toolchain bin
(`just toolchain` prints it). Every non-`just` cargo invocation needs
`export PATH="$(just toolchain):$PATH"`. Established 2026-09-09 when
`cargo check` was "command not found". Related: [wth](/projects/wth.md).
```

Frontmatter keys. OKF's: `type` (required, non-empty — the only conformance
rule), `title`, `description`, `tags`, `timestamp`, `resource`. wth adds two,
both optional, both ignored by any other OKF consumer:

| key | meaning |
|---|---|
| `scope` | absolute directory this concept is specific to; the ranker boosts it when the session's cwd is under it and demotes it when the cwd is elsewhere (§3.3). Absent = global |
| `source` | `harvest` (§5), `model` (`knowledge_put`), absent for hand-written files. Shown by `knowledge_get` so the model can weigh provenance |

Types are open (the spec is permissive) and the harness never validates them;
the guidance names five and the model is told to prefer them: `fact`
(something true about the user's world), `entity` (a host, service, repo,
person, account), `decision` (what was chosen and why), `procedure` (steps
that worked), `project` (a hub concept linking the others). Unknown keys are
preserved byte-for-byte by `okf::Frontmatter` (spec §4.1).

Links are ordinary markdown links, bundle-absolute (`/projects/wth.md`) or
relative (`./wth.md`), resolved by `okf::Link::resolve`. Broken links are kept
(spec §5) and reported by `knowledge_get` as `→ missing`.

Body cap: 16 KiB (`Err` beyond; the model is told to link, not paste).

### 2.3 Writes

Every write path (`knowledge_put`, harvest, `knowledge_delete`,
`knowledge_restore`) does, under one `tokio::sync::Mutex<()>` in `Knowledge`
(`Knowledge::write_concept` for the put shapes):

```
1. concept_id(id)?; body ≤ 16 KiB; type non-empty; title defaults to the id, description to the title
   frontmatter = type, title, description, tags (if any), scope (if any), source, timestamp = now (RFC 3339, seconds)
   Document::new(fm, body).serialize(); re-parse; validate_conformance()?
2. write <root>/<id>.md atomically (<id>.md.tmp + rename; create parents)
3. upsert the FTS row and the links rows (§3.1)
4. okf::index::regenerate_indexes(root)               spec §6, progressive disclosure for other consumers
5. append "- put <id>" (or "- delete <id>", "- restore <id>") under today's `## YYYY-MM-DD` in log.md (okf::Log)
6. commit (§2.4): the touched paths (concept, every index.md, log.md) on top of HEAD's tree, message "put <id>" / "delete <id>" / "restore <id> from <commit>"
7. publish KnowledgeChanged { id, op, source }        source = Harvest | Model for put; User for delete and restore
```

Every document a harvest writes goes through `put` on its own: *n* documents
are *n* commits `put <id>`. `sync` commits the external edits it finds as one
commit `sync: edit <id>, remove <id>, …` (publishing `KnowledgeChanged` with
`source: sync` per id), so the history is complete whoever wrote the file.
`delete` also removes a directory's `index.md` (and the directory) when the
deleted concept was its last file, so the parent index stops listing it.

### 2.4 History without a `git` binary

Two git uses, two answers. Skills clone/pull (04 §2.2) need the network and
the user's keys, proxies and credential helpers — that stays on the user's
`git`, and brush (a shell) does not bundle one. The knowledge bundle needs
init, commit, log, diff and restore, all local, on a few hundred small
files: exactly the slice `gix` does well with no C library and no
transport.

```toml
gix = { version = "0.87", default-features = false, features = ["revision", "blob-diff", "sha1", "parallel"] }
```

No default features: no `interrupt` (`parking_lot`, `signal-hook`), no
`status`/`dirwalk`/`worktree-mutation` (we own the worktree: the write path
knows which files it touched, so there is nothing to discover), no network.
`revision` for `rev_walk`/`rev_parse_single`, `blob-diff` for the unified
diff, `sha1` for hashing, `parallel` only because `ThreadSafeRepository` is
`Send` solely with it (`Arc` instead of `Rc` sharing, documented on the
type; nothing is parallelised). The repository is opened and initialised
with `gix::open::Options::isolated()`: no global or system git config, no
`GIT_*` environment — the bundle's history never depends on the user's git
setup. Commit ids are abbreviated to a fixed 7 hex chars (`abbrev`), so
they stay stable in logs and messages. What is used, all in
`knowledge::repo::Repo` (synchronous; the store calls it inside
`spawn_blocking`, each call on `to_thread_local()`):

```
open:      ThreadSafeRepository::open_opts(root, isolated) | init_opts(root, WithWorktree, default, isolated)    a normal .git, loose objects
reconcile: (at open) write_blob every file under root except .git/; build the full tree from the empty tree;
           differs from HEAD's tree → commit "reconcile" (catches edits made while wth was down and anything a failed
           commit left behind; writing an existing loose object is a no-op)
commit:    base = HEAD's tree or the empty tree; editor = edit_tree(base); upsert(rel, Blob, write_blob(read(path)))
           per written path; remove(rel) per removed; tree == base → return HEAD's id (nothing to commit);
           commit_as(sig, sig, "HEAD", message, tree, parent)      sig = "wth <wth@localhost>", time = now
history:   rev_walk([HEAD]).all(); keep a commit when the entry id at <id>.md differs from its first parent's
           (every commit when no id) → Change { commit: 7 hex, time: local "YYYY-MM-DD HH:MM", message: first line }
show:      rev_parse_single(commit) → tree → the blob at <id>.md, as bytes; None when absent
last_diff: newest touching commit vs its first parent: gix::diff::blob Histogram, UnifiedDiff with 3 context lines,
           "--- a/<rel>\n+++ b/<rel>\n" + hunks; None when the path has fewer than two states
previous:  the first parent of the newest touching commit (knowledge_restore's default)
restore:   show(<id>.md, commit) → the ordinary write path (§2.3) with message "restore <id> from <commit>"
```

`Repo::open` failing (unwritable root, damaged `.git` gix cannot even init)
is a warning, not an error: `Knowledge.repo` is `None`, every write still
lands on disk and in the index, and `history`/`restore` return `Err("…
unavailable: no repository")`.

Everything is loose objects; nothing is ever packed or gc'd by the harness.

```rust
// ponytail: loose objects only; a bundle with tens of thousands of commits should run `git gc` by
// hand. Adding gix's pack writer is the upgrade if that ever happens.
```

Sharing across machines is the user's `git remote add` + `push` on
`~/.wth/knowledge` (or a future `knowledge` source like `add_skill_repo`);
not in this doc.

## 3. Index and ranker

### 3.1 Schema (`<data_dir>/knowledge.sqlite`, sqlx, WAL, no migrations)

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS concepts USING fts5(
    id UNINDEXED, type, title, description, tags, body,
    scope UNINDEXED, source UNINDEXED, timestamp UNINDEXED,
    path UNINDEXED, mtime UNINDEXED, size UNINDEXED,
    tokenize = 'porter unicode61 remove_diacritics 2'
);
CREATE TABLE IF NOT EXISTS links (src TEXT NOT NULL, dst TEXT NOT NULL, PRIMARY KEY (src, dst));
CREATE INDEX IF NOT EXISTS links_dst ON links(dst);
```

Run by `open_db` at every `Knowledge::open`; the index is derived, so it has
no migration history — a schema change is `rm knowledge.sqlite`. An
unopenable file is deleted (with `-wal`/`-shm`) and recreated. `type` is
indexed so `knowledge_search("decision mutex")` works with no filter syntax;
`id` is not (path punctuation is noise). `path`/`mtime`/`size` are the sync
key. `links.dst` holds resolved concept ids (`okf::Link::resolve`), so a
broken link is a `dst` with no `concepts` row. The bundled sqlite has FTS5
(`libsqlite3-sys` bundled build sets `SQLITE_ENABLE_FTS5`; verified 0.36.0
and 0.37.0).

### 3.2 Sync

`Knowledge::sync()` — at `Knowledge::open` and at every turn start
(`Turn::rebase`, next to the skills index rebuild). Under the write mutex:

```
walk <root> (skip .git/, index.md, log.md; *.md only)          spawn_blocking
  for each file: stat → (path, mtime as unix nanos, size)
    row absent or (mtime, size) differ → parse (okf::Document) → upsert row + links; parse error → warn once per path, skip
rows whose path no longer exists → delete row + links
anything changed → one commit "sync: edit <id>, remove <id>, …"; publish KnowledgeChanged { source: sync } per id
return Sync { added, updated, removed }   // logged at info when non-zero
```

A few hundred `stat`s per turn — microseconds; a full parse only for changed
files. `knowledge_get` on a file `sync` has not seen yet (or could not parse
before) indexes it on the spot.

### 3.3 Search and hint

```rust
pub fn terms(text: &str) -> Vec<String>;
// split on non-alphanumeric (so `/a/b/c.rs` → a, b, c, rs), lowercase, drop len < 3 and
// STOPWORDS (≈60 English + chat/code words: the, and, for, with, this, that, from, file, use,
// run, please, let, can, …), dedupe preserving first occurrence, cap 64
```

`search(query, limit)`: `terms(query)` joined with `OR` as quoted FTS5 strings,
ranked by `bm25(concepts, 0, 3.0, 10.0, 5.0, 4.0, 1.0)` (id, type, title,
description, tags, body); `snippet(concepts, 5, '', '', '…', 12)` from the
body. Then, in Rust, on the top `4 × limit`:

- **scope**: `score × 1.5` when `cwd.starts_with(scope)`; `× 0.5` when a
  scope is set and does not contain cwd; unscoped unchanged (bm25 is
  negative-better in FTS5; the multipliers apply to `-bm25`);
- **≥2-term rule** (hints only): a hit must contain at least two distinct
  query terms (prefix match on the row's lowercased text, so `toolchain`
  matches `toolchains`), or one when the query has one term. Without it a
  single common word would hint the whole catalog; bm25 ranks it, the rule
  gates it.
- truncate to `limit`.

`hint(text, cwd, exclude, limit)` = `search` with the rule, minus `exclude`
(the ids already hinted this epoch), returning metas only.

This FTS path is the model's retrieval and the hint source only. The human
search of [49](49-search.md) does not issue FTS `MATCH`: `knowledge::search`
reads every valid current row (id, metadata, whole body) and matches it with
the shared nucleo matcher from `crates/search`, so short ids, `'exact`,
`^prefix`, `suffix$` and `!not` behave the same as session search. The
snapshot is keyed by an index-mutation generation (bumped by every row
upsert/remove and by `sync`); `search_refresh` runs one `sync` so the next
query rebuilds, and forgets a failed history build so it retries — a good
snapshot stays until the fresh one is published. History search walks the
captured full HEAD's reachable commits, diffs each tree against its first
parent, and matches the complete Before and After blob text plus
id/frontmatter/subject — not the unified diff — one hit per `(commit,
concept)`; an external HEAD move is simply a new key. Both are lazy
owner-local caches: a cold key answers `indexing` and builds off the
mailbox; no new store.

```rust
// ponytail: bm25 + a two-term gate + a path prefix. If hints are noisy in practice, the
// upgrade is an embedding column in the same table (sqlite-vec) ranked alongside — not a new store.
```

## 4. Hints: `<knowledge>` notes

### 4.1 Format

```
<knowledge>
Concepts that may be relevant; call knowledge_get before relying on one:
- `facts/wth-toolchain` — wth's Rust toolchain lives under rustup, not on PATH [fact]: cargo/rustc are at ~/.rustup/…
- `procedures/signal-link` — Linking wth as a Signal device [procedure]: run --link-signal, scan, then --headless
</knowledge>
```

One line per id, description clipped at 120 chars, at most `hint_limit = 5`
lines, best first. Persisted as a user-role note through `Turn::note(
"knowledge", …)`, so it is history afterwards: frozen, replayed on resume,
hidden by UIs like every wrapper (`tui::model::injected`).

### 4.2 When

In `Turn::round`, before the model call, after the pressure note and the
todo reminder (07 §8.7, §9), when the knowledge actor is registered
(`Shared.knowledge` is `Some`, 12 §9.2):

| moment | query text | condition |
|---|---|---|
| round 0 of a turn | the turn's inputs (user text, steering that arrived while idle) | hits ∖ `hinted` non-empty |
| round `r` with `r − knowledge_hinted_round ≥ hint_rounds` (4) | text appended since the last hint: assistant texts, tool-call **arguments** (paths, symbols, commands, code — the strongest signal of what the model is about to touch), steering. Not tool results (too much text, mostly already read) | same |

After a check: `knowledge_hinted_round = round` (hit or not — the cadence
counts checks, not notes) and `hint_since = history.len()`; after a note:
`hinted ∪= ids`. `hinted` is on `SessionState`, cleared by compaction (the
model lost the note with the rest of the epoch) and on resume (`hinted` is
not persisted; the worst case is one repeat hint per resumed session).
`knowledge_hinted_round` is reset to 0 at every turn start.

No hits → no note, no cost. The mid-turn note shares the rate limit reason
with the todo reminder: a user text block after a `tool_result` strips
earlier thinking on Haiku and pre-4.6 Sonnet (07 §3), so it is not sent
every round.

Subagents get hints like any session (their first message is the task; the
turn-start hint runs on it).

### 4.3 Why not the system prompt

Two reasons, both cache. (a) A hint block in the baseline changes whenever
the input changes: a rebase per turn, exactly jarvis's miss. (b) An *index*
of the catalog in the baseline (the skills-index shape, 04 §1.1) is stable
per turn but changes on every harvest — every third turn a rebase — and
grows without bound. The skills index is in the prompt because skills change
rarely and are few; knowledge is neither. So the plugin registers one static
sentence as its prompt block (`tools::PROMPT`, `RegisterPrompt`; 00 §4, 07 §4
part 7) and nothing else:

> Durable facts about the user's machines, projects, accounts and decisions
> belong in the knowledge base: check `<knowledge>` hints and
> `knowledge_search` before assuming, record with `knowledge_put`.

## 5. Harvest: building the base while the user works

### 5.1 The idea

The frame that just finished a turn is sitting in the provider's cache
(07 §7 shows 92–99 % hits). Sending it once more with one new user message
costs the cache-read rate on the prefix plus a few hundred output tokens —
the summariser's economics (07 §8.3), which the §12.5 run measured. That is
where the knowledge is extracted: not per turn (too often, too costly), not
only at compaction (too rare in short sessions), but at both boundaries the
frame already has.

### 5.2 Trigger

State on `SessionState`: `turns_since_harvest: u32`, in memory only — a
resumed session starts at 0 and its first harvest comes up to two tool-using
turns late. Not worth a column and a migration.

```
Turn::run, at the end of a completed turn, BEFORE TurnFinished is published:
  harvestable = calls > 0 (any tool, `todo`/`python` included) && stop_reason != "cancelled" && depth == 0
  if harvestable && knowledge is on:
      turns_since_harvest += 1
      if harvest_every > 0 && turns_since_harvest >= harvest_every:
          turns_since_harvest = 0
          publish Harvesting { session }
          harvest_with(registry)       // resolve model/tools/transport, then §5.4; Err → warn
  publish TurnFinished
  return state                          // TurnDone; queued steering becomes the next turn's input

Turn::compact, before the summariser (07 §8.3):
  if harvest_every > 0 && depth == 0 && (turns_since_harvest > 0 || calls > 0) && knowledge is on:
      turns_since_harvest = 0; calls = 0   // this turn's work is folded now; the turn end recounts only work after the cut
      harvest(model, tools, transport)  // same prefix, then the cut; Err → warn, compaction proceeds

HarvestNow (ask on the Session; `/knowledge harvest`, RPC knowledge_harvest → Agents `Harvest { session }`):
  Err("turn in flight") while a turn (or another mini-turn) owns the state,
  else a Turn-less task: between_turns() (registry + rebase) → harvest_with → Ok(HarvestResult) | Err(String)
```

The cadence harvest runs **inline at the end of the turn, before
`TurnFinished`**, not concurrently with the next one. Before: one-shot
clients (`wth -p`) exit on `TurnFinished`, and a harvest published after it
was cancelled every time (found in §12.6's first run). The reply text has
already streamed; only the idle flip waits, and `Harvesting { session }`
lets the TUI show `harvesting…` in the status line the way `Compacting`
does. Input arriving meanwhile queues as steering. The cost is a few
seconds every third tool-using turn; the gain is no clone of the history
and no second writer of the session's state.

`depth == 0`: children do not harvest. A child's findings reach the parent
as `<subagent_result>` text and the parent's harvest sees them; ten children
harvesting the same task would be ten calls for one fact.

Not harvested: turns with no tool calls (a chat answer produces no new facts
about the user's world), cancelled turns (the count is untouched), turns
with an empty history (`Err("nothing to harvest")`), and the moments before
process exit — the last one or two turns of a session are lost unless the
user runs `/knowledge harvest`; a model call on the shutdown path is not
worth it.

### 5.3 The instruction (`knowledge::HARVEST_INSTRUCTION`, one user message)

```
Review this conversation for knowledge worth keeping across sessions: facts about the user's
environment (hosts, paths, toolchains, accounts, conventions), decisions and the reasons behind
them, procedures that worked, and entities (projects, services, people) and how they relate.
Not: transient task state, todo progress, file contents that live in a repository, or anything
already recorded below unless it changed.

Existing concepts that may already cover this — update by reusing the id; never create a
near-duplicate:
{related}

Reply with text only — do not call tools. Emit zero or more documents, each exactly:

<concept id="facts/example-slug">
---
type: fact            # fact | entity | decision | procedure | project
title: one line
description: one line, what a reader needs to decide whether to open this
tags: [a, b]
scope: {cwd}          # only when the fact is specific to this directory or project; omit otherwise
---
The knowledge itself, with the evidence that established it (the command, path, error text,
or the user's words). Link related concepts as [title](/other/id.md).
</concept>

Ids are lowercase `a-z0-9-` segments, directory/slug. Prefer updating an existing concept over
adding one. If nothing qualifies, reply `none`.
```

`{related}` = `hint(hint_query(history), cwd, ∅, 10)` — `hint_query` is the
same projection §4.2 uses mid-turn (steering, assistant texts, tool-call
arguments; not tool results or harness wrappers), here over the whole
epoch's history — rendered as hint lines without the lead sentence
(`render_related`), or `none`. `{cwd}` = the process working directory (the
baseline already carries it; repeated here so the model copies it
verbatim). `{cwd}` is substituted first, so a related description containing
the literal `{cwd}` cannot be expanded.

### 5.4 The call and the write

```
Turn::harvest(model, tools, transport):
  related     = knowledge.hint(hint_query(history), cwd, ∅, 10)          // Err → warn, treated as none
  instruction = knowledge::harvest::instruction(&related, cwd)
  full        = messages(history) + [ChatMessage::user(instruction)]
  text        = continuation(model, tools, transport, full, instruction)  // shared with Turn::summarise: Frame { tools, baseline, head, full };
                                                                          // a reply with only tool calls (or no text) replaces the last message with
                                                                          // "Text only. {instruction}" and retries once; a second → Err("no text twice");
                                                                          // a model error (overflow included) → Err (no trim: an overflowing frame is about to be compacted anyway)
  for (id, doc) in parse_harvest(&text).take(harvest_max = 8):           // §5.5
      put(Put { id, type, title (id when absent), description, tags, scope, body }, Source::Harvest)
        Ok created → added += 1; Ok updated → updated += 1; Err → skipped += 1, warn
  info!(session, epoch, added, updated, skipped, "harvest")
  publish Harvested { session, added, updated }
  Ok(HarvestResult { added, updated, skipped })
```

`skipped` counts documents `put` rejected (id shape, body cap); blocks
`parse_harvest` could not parse are warned and never reach the count. The
harvest borrows the live `SessionState` (the turn is over; nothing else can
touch it) and the request shares the finished frame's cached prefix. On
Anthropic the continuation's 5-minute mark lands on the instruction — a
small write on a dead branch, as with the summariser; the lookback finds the
previous marks for the read. On OpenAI-like transports the
`prompt_cache_key` is the session id, so the branch shares the same cache.

The harvest call and instruction are **not persisted** to `messages`: the
next turn's frame is the finished frame plus the user's next input, exactly
as if no harvest had run (07 §0).

### 5.5 Parsing

`parse_harvest(text) -> Vec<(String, okf::Document)>` (`knowledge::harvest`):
a reply that is `none` (case-insensitive, trimmed) yields nothing; otherwise
scan for `<concept id="`, take the attribute up to `"`, skip to the tag's
`>`, the body up to the next `</concept>`, repeat. Per document: lowercase
the id, `ConceptId::parse`, reject > 3 segments and the reserved names
`index`/`log`, `Document::parse`, `validate_conformance`; strip any
`timestamp`/`source` the model wrote (the harness sets them). A document
that fails is `warn!`ed with its id and reason and skipped; the others are
returned. The 16 KiB body cap is `put`'s (§2.3). No regex crate — `find`s in
a loop.

### 5.6 What makes it good rather than noisy

- **Boundary, not cadence.** Only turns that did work count; a fact appears
  at most every third such turn, and always when the epoch closes.
- **Update-first.** The `{related}` list shows what exists; the id is the
  merge key; `put` on an existing id is an update (timestamp refreshed;
  `knowledge_history` shows the diff).
- **Evidence in the body.** The instruction asks for the command/path/error
  that established the fact; a later reader (the model, six sessions on)
  can tell a guess from a measurement.
- **Provenance and undo.** `source: harvest`, one commit per document,
  `log.md`, `KnowledgeChanged` events; `knowledge_restore` puts any concept
  back to any earlier commit. Nothing happens silently, nothing is lost.
- **Bounded.** `harvest_max = 8` documents per batch, 16 KiB bodies, the
  id-shape rules.

## 6. Tools (`knowledge::tools`, registered by `KnowledgePlugin::start` once the registry is up (`lookup_when`, 12 §9.1); direct = true, so also runtime functions under 08 §4.2)

Wire arg names are the field names below (`type`, not `type_`); `limit`s are
`Option<u32>`.

| tool | args | risk | returns (text for the transcript; the runtime gets `structured` where noted, 08 §3.2) |
|---|---|---|---|
| `knowledge_search` | `query`, `limit?` (default 10, clamped to 1..=50) | ReadOnly | one line per hit, best first: `` `id` — title [type] (score, one decimal): snippet ``; `no matches` when none. Python: `list[dict]`, each the frontmatter fields plus `score` and `snippet`; `[]` when none |
| `knowledge_get` | `id` | ReadOnly | the file verbatim (frontmatter + body), then `Links: facts/a, facts/b → missing` (resolved ids; `→ missing` when no such concept) and `Cited by: c, d`; each line only when non-empty |
| `knowledge_list` | `prefix?`, `type?`, `tag?`, `limit?` (default 50, no cap) | ReadOnly | `` `id` — title [type]: description `` per line, sorted by id (`: description` omitted when empty); `no concepts` when none. Python: `list[dict]` of frontmatter |
| `knowledge_put` | `id`, `type`, `title`, `description?`, `tags?`, `scope?`, `body` (the `Put` type) | ReadOnly (below) | `put facts/x (new)` / `put facts/x (updated)` |
| `knowledge_delete` | `id` | Destructive | `deleted facts/x` |
| `knowledge_history` | `id?`, `limit?` (default 20) | ReadOnly | one line per commit touching `id` (the whole bundle when absent), newest first: `` a1b2c3d  2026-09-10 09:41  put facts/x `` (two spaces between fields); with `id`, a blank line and the unified diff of the latest change follow; `no history` when none. Python: `dict {changes: list[dict], diff: str | None}` |
| `knowledge_restore` | `id`, `commit?` (default: the commit before the latest change) | Mutating | rewrites `<id>.md` from that commit — also the way back for a deleted concept — through the ordinary write path; `restored facts/x from a1b2c3d` |

`knowledge_put` is `ReadOnly` deliberately, unlike `add_skill` (Mutating):
its only effect is a file inside the harness's own bundle under a validated
id, logged, committed, restorable — the trust class of `todo` and the
runtime dump, not of the shell. An `always` skill rewrites every future
system prompt; a concept is only ever pulled by id. Under the default policy
(`mutating: ask`) a Mutating `knowledge_put` would prompt on every fact the
model records and it would stop recording. `knowledge_delete` and
`knowledge_restore` ask: they replace content the model did not just write.

`Turn::round` records ids passed to `knowledge_get` in
`state.knowledge_read` (a `BTreeSet<String>` per epoch) for the head (§7).

The bundle is also a directory: `read_file ~/.wth/knowledge/index.md` and
`grep` work, and the `/cwd` mount does not include it (08 §4.4) — the tools
are the runtime's path to it. When `knowledge.enabled` is false the plugin
starts nothing: no actor, no tools; when the repo failed to open,
`knowledge_history`/`knowledge_restore` are `Err(… unavailable: no
repository)` and the rest work.

## 7. Compaction head and epoch boundary

07 §8.4's head gains a part after `Runtime:`:

```
Knowledge: read facts/wth-toolchain, procedures/signal-link
```

— the ids the model fetched this epoch, so the model knows what it held
verbatim before the summary and can `knowledge_get` again instead of
trusting the summariser's paraphrase. Harness truth, rendered from
`state.knowledge_read`, which is then cleared with `hinted` (§4.2).

Order inside `Turn::compact`: harvest (§5.2) → summarise → cut. The harvest
runs on the frame that is about to be folded — the last time its detail is
cheap to read.

## 8. Surfaces

**RPC** (`crates/rpc/src/api.rs`, jsonrpsee positional/named params):

```
knowledge_search  { query: String, limit?: u32 }                          → Vec<Hit>       limit default 10, max 50
knowledge_get     { id: String }                                          → Concept
knowledge_list    { prefix?: String, type?: String, tag?: String, limit?: u32 } → Vec<Meta>  wire param is `type` (#[argument(rename)]); limit default 50
knowledge_put     { put: Put }                                            → Meta           written as `source: user`
knowledge_delete  { id: String }                                          → ()
knowledge_history { id?: String, limit?: u32 }                            → History        limit default 20
knowledge_revision { commit: String, id?: String }                       → Revision       selected commit vs first parent
knowledge_read_revision { id: String, commit: String }                   → HistoricalConcept  full text as committed; full object id; read, never restore (49 §7)
knowledge_restore { id: String, commit?: String }                         → Change
knowledge_harvest { session: SessionId }                                  → HarvestResult  refused while a turn runs
```

The `knowledge` and `knowledge_history` groups of the unified `search` RPC
and `search_refresh` ([49 §4.2](49-search.md#42-one-rpc-and-typed-results))
ask the same actor with `knowledge::search::{SearchConcepts, SearchHistory,
RefreshSearch}`; the rpc crate composes the groups and never opens the
bundle or its `.git`.

All but `knowledge_harvest` `ask` the `"knowledge"` actor
(`knowledge::KnowledgeActor`, 12 §9.1) with `knowledge::{Search, Get, List,
PutConcept, Delete, HistoryOf, RevisionOf, Restore}` and
`knowledge::search::ReadRevision`; when the plugin is disabled the
actor is not registered and the frontend answers `Err("knowledge is
disabled")` from the failed lookup. The store asks are local I/O and run on
the mailbox. `knowledge_harvest` asks the `"agent"` supervisor
(`agent::Harvest`), which delegates (it is a model call) to the session's
`HarvestNow`. Events on the stream: `knowledge::KnowledgeChanged { id, op:
put | delete, source: harvest | model | user | sync }` (the plugin's own
event struct, published by the store, global, no session — not an
`AgentEvent` variant), `Harvesting { session }` (the continuation is in
flight; the TUI shows `harvesting…`), `Harvested { session, added, updated }`.
Bindings via `just ts`.

`Revision { change: Change, parent: Option<String>, files: Vec<ChangedFile> }`
is read-only. Each `ChangedFile { id, status, diff }` has `status` = `added`,
`modified`, or `deleted` and a unified diff. `parent` is the full commit id;
root commits compare with the empty tree. Paths come from the git tree diff,
not the commit message: multi-concept sync/reconcile commits and deleted
concepts remain inspectable. Generated `index.md`/`log.md` files are excluded.
Both UIs fetch history in increments of 20, asking for one extra row to know
whether older commits exist, and request revision details only on selection.

`HistoricalConcept { id, commit, text }` is the file verbatim as committed:
a `knowledge_history` search hit's `snapshot_commit` (the change commit for
an After hit, its first parent for a Before hit) is read through
`knowledge_read_revision` without walking history pages and without a
present-day concept — a deleted concept reads the same way. Unknown
id/commit is an error, never the current text. Restore stays the separate
confirmed `knowledge_restore`. The desktop reads it at
`/knowledge/$id?change=<commit>&snapshot=<commit>` (a read-only **Historical
version** view with the History sheet's Changes for that commit); the TUI's
search-owned Historical reader is the same text with `c` for changes.

**TUI** (`crates/tui`), `/knowledge [query | history [id] | show <commit> [id] | restore <id> [commit] | harvest]`
(`Model::knowledge_cmd`; first word decides, everything else is a search).
`/search [query]` ([49 §6](49-search.md#6-terminal-interaction)) is the
fuzzy, live, all-category search; its Knowledge hit opens a read-only
current-concept reader fed by `knowledge_get`, its Knowledge history hit the
Historical reader above — neither is the history picker below:

| input | does | renders |
|---|---|---|
| `/knowledge` | `knowledge::List { limit: 50 }` — the first 50 concepts by id | picker: `↑↓` selects; `⏎`/`h` opens that concept's history; `H` opens the whole bundle, including when the picker is empty |
| `/knowledge <query>` | `knowledge::Search { query }` (limit default 10), metas of the hits | the same picker, best first |
| `/knowledge history [id]` | `knowledge::HistoryOf { id?, limit: 21 }` initially | commit picker with `⏎` to inspect, an older row loading 20 more, `r` refresh/retry, Esc closes |
| `/knowledge show <commit> [id]` | `knowledge::RevisionOf { commit, id? }` | the selected revision as a transcript card (plain text in line mode) |
| `/knowledge restore <id> [commit]` | `knowledge::Restore { id, commit? }` | info line `a1b2c3d  restore facts/x from b2c3d4e`; `/knowledge restore` alone is the error `usage: /knowledge restore <id> [commit]` |
| `/knowledge harvest` | `agent::Harvest { session }` for the shown session | info line `knowledge +a ~b`; `no session` when none is shown |

The selected revision shows commit metadata and each changed concept's
unified diff. `↑↓` scrolls rows, `PgUp`/`PgDn` scrolls 20 rows, `←→` scrolls
long lines, and Esc/`h`/Backspace returns to the commit list. Loading, empty
history and request errors stay in the menu; late replies cannot reopen a
dismissed history menu. Line mode keeps printing history and revision text.

The `Harvested` **event** (the cadence harvest, or one triggered from
another client) is interjected as the same `knowledge +a ~b` info line in the
transcript — an entry, not a status-line flash. The TUI does not subscribe to
`KnowledgeChanged` (the log has it). `<knowledge>` notes are hidden with the other
wrappers (`injected`). History errors stay in the open menu for retry;
other command errors arrive as `UiEvent::Error` entries.

**Config** (two sections, each read by its own plugin — 00 §9; commented
out with these defaults in the first-run template):

```yaml
knowledge:               # the store (KnowledgePlugin)
  enabled: true          # false: no actor, no knowledge_* tools, no hints, no harvest
agent:
  knowledge:             # the loop's use of it (AgentPlugin, `KnowledgeUse`)
    hint_limit: 5        # lines per <knowledge> note
    hint_rounds: 4       # mid-turn hint cadence in rounds
    harvest_every: 3     # tool-using root turns per harvest; 0 disables harvest
    harvest_max: 8       # documents accepted per harvest
```

Env overrides follow 01 §1.1 (`WTH_KNOWLEDGE__ENABLED`,
`WTH_AGENT__KNOWLEDGE__HARVEST_EVERY`).

## 9. Cache accounting

Every knowledge input, where it lives, and what it does to the prefix
(07 §2.3's table, extended):

| input | lives in | changes when | cache effect |
|---|---|---|---|
| guidance sentence | system (plugin prompt block, `tools::PROMPT`) | never | none |
| `<knowledge>` hints | history tail | turn start / every 4 rounds, only with hits | none (append) |
| ids read this epoch | head | compaction | part of the one expected miss |
| harvest instruction + reply | a dead-end branch off the finished frame | every 3rd tool-using turn, before compaction | prefix read at the cached rate; the branch's own small write; the session's next frame is unaffected |
| the bundle, the FTS index | disk | every write | none — never in a frame |
| tool specs (`knowledge_*`) | tools | never within a process (pure functions of config, 07 §2.2 rule 5) | none |

The design adds **no** rebase, **no** new mutation of history, and one
cache-hot continuation whose cost is the same as one summariser call.

## 10. Types

As built (`crates/knowledge/src/{lib,types,store,repo,harvest,tools}.rs`; the
crate boundary is 12 §9.1):

```rust
// knowledge (lib.rs)
pub const KNOWLEDGE_ACTOR: &str = "knowledge";
pub struct KnowledgePlugin;                     // impl runtime::Plugin; name() = "knowledge"
pub struct KnowledgeConfig { pub enabled: bool } // section `knowledge:`; #[serde(default)], enabled = true
pub struct KnowledgeActor { .. }                // registered as KNOWLEDGE_ACTOR; wraps Arc<Knowledge>
pub struct Handle;                              // ask → Arc<Knowledge> (the loop's hot path: hint/sync/put without a mailbox hop)
pub const KNOWLEDGE_HINT_ROUNDS: u32 = 4;      // default of hint_rounds
pub const DIR: &str = "knowledge";             // §2.1
pub const INDEX_FILE: &str = "knowledge.sqlite"; // §3.1
pub const BODY_MAX: usize = 16 * 1024;         // §2.2
pub const HINT_DESCRIPTION_CHARS: usize = 120; // §4.1
pub use store::{Knowledge, terms}; pub use types::*;

// knowledge::types (serde + schemars + ts_rs; wire name of `type_` is `type`)
pub enum Source  { Harvest, Model, User, Sync }              // snake_case; as_str() / parse()
pub enum KnowledgeOp { Put, Delete }                         // snake_case
pub struct Meta  { pub id: String, pub type_: String, pub title: String /* the id when absent */, pub description: String /* "" when absent */,
                   pub tags: Vec<String>, pub scope: Option<PathBuf>, pub source: Option<Source>, pub timestamp: Option<String> }
pub struct Concept { pub meta: Meta, pub text: String /* file verbatim */, pub links: Vec<(String /* id */, bool /* exists */)>, pub cited_by: Vec<String> }
pub struct Hit   { pub meta: Meta, pub score: f32 /* scope-adjusted -bm25, higher better */, pub snippet: String }
pub struct Put   { pub id: String, pub type_: String, pub title: String, pub description: Option<String>,
                   pub tags: Vec<String>, pub scope: Option<PathBuf>, pub body: String }     // Default; the knowledge_put tool's Args
pub struct Sync  { pub added: u32, pub updated: u32, pub removed: u32 }   // Copy; is_empty()
pub struct HarvestResult { pub added: u32, pub updated: u32, pub skipped: u32 }   // Copy, Default
pub struct Change { pub commit: String /* 7 hex */, pub time: String /* "YYYY-MM-DD HH:MM" local */, pub message: String }
pub struct History { pub changes: Vec<Change> /* newest first */, pub diff: Option<String> /* latest change to the requested id */ }

// knowledge::store
pub struct Knowledge { root: PathBuf, db: SqlitePool, repo: Option<Arc<Repo>>, rt: Runtime /* publishes KnowledgeChanged */,
                       write: tokio::sync::Mutex<()>, warned: Mutex<HashSet<PathBuf>> /* sync warned once per path */ }
impl Knowledge {
    pub async fn open(data_dir: &Path, rt: Runtime) -> anyhow::Result<Arc<Self>>;   // §2.1, §2.4 (Repo::open; failure → repo None), §3.1 (open_db), §3.2 (sync)
    pub fn root(&self) -> &Path;
    pub async fn sync(&self) -> anyhow::Result<Sync>;                                // §3.2
    pub async fn get(&self, id: &str) -> anyhow::Result<Concept>;
    pub async fn put(&self, p: Put, source: Source) -> anyhow::Result<(Meta, bool /* created */)>;   // §2.3
    pub async fn delete(&self, id: &str) -> anyhow::Result<()>;
    pub async fn list(&self, prefix: Option<&str>, type_: Option<&str>, tag: Option<&str>, limit: u32) -> anyhow::Result<Vec<Meta>>;
    pub async fn search(&self, query: &str, cwd: &Path, limit: u32) -> anyhow::Result<Vec<Hit>>;        // §3.3
    pub async fn hint(&self, text: &str, cwd: &Path, exclude: &BTreeSet<String>, limit: u32) -> anyhow::Result<Vec<Meta>>;
    pub async fn history(&self, id: Option<&str>, limit: u32) -> anyhow::Result<History>;   // §2.4; Err without a repo
    pub async fn restore(&self, id: &str, commit: Option<&str>) -> anyhow::Result<Change>;  // §2.4; through the write path; Err without a repo
}
pub fn terms(text: &str) -> Vec<String>;                                        // §3.3

// knowledge::repo (sync, blocking; called inside spawn_blocking)
pub const SIGNATURE: (&str, &str) = ("wth", "wth@localhost");
pub struct Repo { root: PathBuf, inner: gix::ThreadSafeRepository }
impl Repo {
    pub fn open(root: &Path) -> anyhow::Result<Self>;                              // open_opts | init_opts, isolated; then reconcile
    pub fn reconcile(&self) -> anyhow::Result<Option<String>>;
    pub fn commit(&self, written: &[PathBuf], removed: &[PathBuf], message: &str) -> anyhow::Result<String>;
    pub fn history(&self, path: Option<&Path>, limit: usize) -> anyhow::Result<Vec<Change>>;
    pub fn show(&self, path: &Path, commit: &str) -> anyhow::Result<Option<Vec<u8>>>;
    pub fn last_diff(&self, path: &Path) -> anyhow::Result<Option<String>>;
    pub fn previous(&self, path: &Path) -> anyhow::Result<Option<String>>;
}

// knowledge::harvest (pure)
pub const HARVEST_INSTRUCTION: &str;                                            // §5.3, with {related} and {cwd}
pub fn instruction(related: &[Meta], cwd: &str) -> String;
pub fn render_hints(metas: &[Meta]) -> String;                                  // §4.1 body
pub fn render_related(metas: &[Meta]) -> String;                                // hint lines or "none"
pub fn parse_harvest(text: &str) -> Vec<(String, okf::Document)>;               // §5.5

// knowledge::tools
pub const GET: &str = "knowledge_get";
pub async fn register(reg: &ActorRef<ToolRegistry>, knowledge: Arc<Knowledge>);   // the seven tools of §6

// SessionState (loop_.rs) gains
pub hinted: BTreeSet<String>,            // ids hinted this epoch; cleared by compaction, not persisted
pub knowledge_hinted_round: u32,         // round of the last <knowledge> check this turn; 0 at turn start
pub knowledge_read: BTreeSet<String>,    // ids passed to knowledge_get this epoch; cleared by compaction
pub turns_since_harvest: u32,            // in memory; 0 after resume

// Session actor messages (loop_.rs)
pub struct HarvestNow;                   // ask → kameo::reply::DelegatedReply<Result<HarvestResult, String>>; Err("turn in flight")

// KnowledgeActor messages (knowledge lib.rs), every reply Result<_, String>
pub struct Search { pub query: String, pub limit: Option<u32> }          // → Vec<Hit>; limit default 10, 1..=50; cwd = current_dir
pub struct Get { pub id: String }                                        // → Concept
pub struct List { pub prefix: Option<String>, pub type_: Option<String>, pub tag: Option<String>, pub limit: Option<u32> }  // → Vec<Meta>; Default; limit default 50
pub struct PutConcept { pub put: Put, pub source: Source }               // → Meta
pub struct Delete { pub id: String }                                     // → ()
pub struct HistoryOf { pub id: Option<String>, pub limit: Option<u32> }  // → History; limit default 20
pub struct Restore { pub id: String, pub commit: Option<String> }        // → Change

// knowledge event (own struct, published by the store; not an AgentEvent variant)
pub struct KnowledgeChanged { pub id: String, pub op: KnowledgeOp, pub source: Source }

// Agents message (agent lib.rs)
pub struct Harvest { pub session: SessionId }                            // → Result<HarvestResult, String> (DelegatedReply; forwards HarvestNow)

// AgentEvent gains
Harvesting { session: SessionId }
Harvested { session: SessionId, added: u32, updated: u32 }

// AgentConfig gains (agent lib.rs)
pub knowledge: KnowledgeUse   // { hint_limit: 5, hint_rounds: 4, harvest_every: 3, harvest_max: 8 }, #[serde(default)]
```

`KnowledgePlugin::start` reads `knowledge:`; disabled → `Ok(())` and nothing
is spawned. Else `Knowledge::open(&rt.data_dir, rt.clone())`, spawn
`KnowledgeActor`, register it as `KNOWLEDGE_ACTOR`, and once the tool
registry is up (`rt.lookup_when`) register the seven tools. The agent's
`Shared.knowledge: Option<Arc<Knowledge>>` is filled per turn in
`Turn::rebase` (`lookup(KNOWLEDGE_ACTOR)` then `ask(Handle)`; `None` →
knowledge off, 12 §9.2); an open error is a warning and the plugin starts
nothing, as if disabled.
Cargo, in `[workspace.dependencies]`, both used only in `knowledge`:
`okf = "=0.1.0-alpha.1"` (pinned exact; alpha) and `gix = { version =
"0.87", default-features = false, features = ["revision", "blob-diff",
"sha1", "parallel"] }` (§2.4). `gix::ThreadSafeRepository` is `Send +
Sync`; each `Repo` operation takes a `to_thread_local()` handle inside
`spawn_blocking`.

## 11. Failure modes

| failure | behaviour |
|---|---|
| bundle file unparsable (hand edit) | `sync` warns once per path, skips the row; the file is untouched |
| `knowledge.sqlite` missing or corrupt | deleted (with `-wal`/`-shm`) and recreated; full sync on open |
| `.git` unusable, `Repo::open` fails | warn `knowledge repo unavailable; history is off`; writes still land on disk and in the index; `knowledge_history`/`knowledge_restore` are `Err` |
| `Knowledge::open` fails (unwritable data dir) | warn `knowledge bundle unavailable`; the agent runs with knowledge off (no tools, no hints, no harvest) |
| commit fails (disk full, `.git` damaged) | warn; the file write, row, `index.md` and `log.md` stand; the next `open` reconciles (§2.4) |
| `.git` deleted by the user | `open` re-inits and the reconcile commit captures the whole bundle; history starts over; the files are untouched |
| harvest reply malformed | per-document skip with a warning; a `none` reply is silent |
| harvest model error / overflow / no text twice | warn, `turns_since_harvest` stays 0 (already reset) — the next boundary tries again on a longer history; at compaction the summariser still runs |
| two sessions harvest the same fact concurrently | the second `put` on the same id is an update; the `write` mutex serialises the file, row, index and commit |
| id collision with a hand-written concept | the harvest updates it (`source` becomes `harvest`); `knowledge_history` shows the diff; the instruction's update-first rule makes this the intended path |

## 12. Verification

Each item is a runnable check with a real provider. Result lines are filled
by the e2e pass.

1. **Hints, turn start.** Seed `knowledge/facts/toolchain.md` (the §2.2
   example). New session, input "why does cargo check say command not
   found?". The first frame ends with a `<knowledge>` note naming
   `facts/toolchain`; the TUI hides it; `sessions.sqlite` has it as a user
   row. `cache 0%` on call one (fresh session), then ≥ 90 % on the next
   round with no `prefix cache miss` warning.
      Result 2026-09-10: PASS. Seeded `facts/wth-toolchain`; input about `cargo check`
   → history row 1 is the `<knowledge>` note naming it, the model called
   `knowledge_get facts/wth-toolchain` in round 0, TUI hides the note; no
   `prefix cache miss` warning.
2. **Hints, mid-turn.** A turn that reads `crates/signal/src/channel.rs` in
   round 2 with a seeded concept tagged `signal`; round 4 shows the note;
   round 8 does not repeat it (`hinted`).
      Result 2026-09-10: PASS (variant). In the §12.8 run a second `<knowledge>`
   note landed mid-turn at seq 10 with ids not in the round-0 note; no repeat.
3. **Hints, none.** Empty bundle: no note at any round; the frame is
   byte-identical to the pre-10 frame except the guidance sentence.
      Result 2026-09-10: not run end to end; `hint()` returns nothing for an
   empty index (unit-tested) and the loop appends no note on empty hits.
4. **Ranker.** Unit: `terms("read /Users/x/wth/crates/agent/src/loop_.rs")`
   → `users wth crates agent src loop` (`read` is a stopword, `x` and `rs`
   are under three chars); a concept matching only `src` is not hinted
   (two-term rule); scoped concept ranks above an unscoped twin when cwd
   matches and below it when not.
      Result 2026-09-10: PASS — `knowledge::store::tests::{terms_example,
   hint_needs_two_terms_and_honours_exclude, search_ranks_title_over_body_and_boosts_scope}`.
5. **Sync.** Edit a concept's title in an editor → next turn's `/knowledge`
   shows the new title; delete the file → the row goes; `rm
   knowledge.sqlite` → rebuilt on start with the same row count.
      Result 2026-09-10: PASS. Title edited in an editor while wth was down →
   next start `knowledge sync updated=1` and a `reconcile` commit; `/knowledge`
   list shows the new title; `index_rebuilt_when_deleted` covers the `rm`.
6. **Harvest.** Three turns that run tools in `/tmp/wth-know` with the
   toolchain fact discovered in turn one; after the third `TurnFinished` the
   log shows `harvest added=1`, `knowledge/facts/*.md` exists with `source:
   harvest` and a body citing the command, `knowledge_history` shows one
   commit and the system `git log` (where present) agrees byte-for-byte on
   the commit id, `index.md` and `log.md` mention it. The next turn's first
   call is a cache hit (the harvest branch did not disturb the session's
   prefix).
      Result 2026-09-10: PASS (with `harvest_every: 1`; the counter is in memory
   and print mode is one turn per process). A `read_file` + two `shell` calls
   → `harvest added=2`: `entities/grafana-prod`, `facts/macos-version` with
   `source: harvest`, evidence blocks quoting the commands, `knowledge +2 ~0`
   in the footer; one commit per document; `git log` agrees on the ids.
   First run FAILED (`harvest: cancelled`): the harvest ran after
   `TurnFinished` and print mode had exited — fixed by moving it before (§5.2).
7. **Harvest, update and restore.** A fourth–sixth turn that corrects the
   fact; the harvest updates the same id (`updated=1`, `added=0`);
   `knowledge_history facts/…` shows two commits and a unified diff of the
   change only; `knowledge_restore facts/…` (approval prompt) brings the
   first text back as a third commit, and the FTS row follows.
      Result 2026-09-10: PASS. Notes file changed to `grafana-prod2` → next
   harvest `updated=1 added=0` on `entities/grafana-prod`; `knowledge_history`
   shows two commits and a diff of the description/body lines only;
   `knowledge_restore` over RPC reverted `grafana-stage`'s title as a new
   commit (`restore entities/grafana-stage from 7597485`) and `knowledge_get`
   showed the old title. Under `approvals: true` the tool asked and was denied
   (stdin closed) — the Mutating gate works.
7b. **No git binary.** `PATH` without `git`: `open` inits the repo, §12.6–7
   pass unchanged, `/knowledge history` works. Then `rm -rf knowledge/.git`
   → restart → one `reconcile` commit containing every file.
      Result 2026-09-10: PASS. Headless daemon started with `PATH=/tmp/nogit-bin`
   (no `git`): search, history, diff, restore, put, list, delete all worked;
   the harvest committed. `rm -rf .git` → restart not exercised separately
   (`open_with_files_reconciles_once` covers the reconcile).
8. **Harvest before compaction.** `compact_at: 0.02` run (07 §12.5 shape):
   the log shows `harvest` immediately before `compaction`; the head carries
   `Knowledge: read …` when `knowledge_get` was called in the epoch.
      Result 2026-09-10: PASS. `compact_at: 0.02`: log shows `harvest` (epoch 0)
   immediately before `compaction epoch=1`; the epoch-1 head ends with
   `Knowledge: read facts/wth-toolchain`. First run showed the hook not firing
   on a session's first turn (`turns_since_harvest == 0`) — fixed by counting
   this turn's calls (§5.2).
9. **Tools.** In one turn: `knowledge_put` (no approval prompt),
   `knowledge_get` shows `source: model` and the link lines,
   `knowledge_search` finds it by a stemmed word, `knowledge_delete` asks.
      Result 2026-09-10: PASS. `knowledge_put` ran with no prompt (create, then
   update on the same id), `knowledge_search "grafanas dashboards"` found it
   (stemmed), `knowledge_get` showed `source: model` and the link line,
   `knowledge_history` showed two commits + diff, `knowledge_list prefix=entities`.
   `knowledge_delete` exercised over RPC; its approval prompt not exercised.
10. **Runtime.** With the Python runtime on, `knowledge_search` is callable
    from a `python` cell and appears in the `## Runtime` index.
        Result 2026-09-10: not run; the tools are ordinary registry tools and the
    runtime index is built from `ListTools` (08 §3.2), same path as every tool.
11. **Surfaces.** RPC `knowledge_search` over the WebSocket; `/knowledge`
    and `/knowledge harvest` in the TUI; `KnowledgeChanged` on the event
    stream; `just ts` emits the new types.
        Result 2026-09-10: PASS for RPC (all eight methods over HTTP) and `just ts`
    (ten types exported); `Harvested` printed as `knowledge +2 ~0` in print
    mode. The interactive TUI `/knowledge` card was not visually checked;
    its command parsing and wrapper hiding are unit-tested (`tui` tests).
12. **Cache ratio.** The print-mode footer over a five-turn session with
    hints and one harvest stays in the 07 §12 band (≥ 90 % from call two
    on) — the design's promise, measured.
        Result 2026-09-10: single-call turns after the first showed `cache 94–96 %`
    with hints and a harvest in the session; turns containing compactions
    dropped to 34–61 % (two deliberate misses each), no unexpected
    `prefix cache miss` warnings in any run.

## 13. Implementation order

Each step leaves `wth` runnable. Steps 1–7 are built; 8 moved the module out
of `agent` (paths above are post-move; steps 1–7 name the module as it was).

1. ✅ `agent::knowledge`: `Knowledge` store (open, sync, get, put, delete,
   list, search, hint, terms); the FTS schema is created at open (no
   migration — the index is derived, §3.1); unit tests against a tempdir
   bundle (`store::tests`, `repo::tests`, `harvest::tests`).
2. ✅ Tools + registration in the agent plugin's start; `KnowledgeConfig`;
   guidance sentence (now the plugin's `RegisterPrompt` block). Check §12.9.
3. ✅ Hints in `Turn::round`; `SessionState` fields; sync in `Turn::rebase`.
   Check §12.1–3.
4. ✅ Harvest: `continuation()` factored out of `summarise`, `HARVEST_INSTRUCTION`,
   `parse_harvest`, the end-of-`Turn::run` trigger, the compaction hook,
   `HarvestNow`. Check §12.6–8.
5. ✅ Head line; `knowledge_read` tracking. Check §12.8.
6. ✅ RPC + events + bindings; TUI command. Check §12.11–12.
7. ✅ Docs: §15 edits, README paragraph.
8. ✅ Crate `knowledge` (12 §9): `KnowledgePlugin`, `KnowledgeActor` and its
   messages, `KnowledgeChanged` as the plugin's own event, `knowledge:`
   config section; `agent.knowledge` shrinks to `KnowledgeUse`; TUI/RPC ask
   the knowledge actor; the harvest model call stays in the agent.

## 14. Rejected

- **Hints in the system prompt** (jarvis `{{okf_hints}}`). One prefix miss
  per turn; the whole point of 07 is that volatile text lives at the tail.
- **A catalog index in the system prompt** (the skills-index shape). Stable
  within a turn but rebases every harvest and grows with the base; §4.3.
- **Token-overlap ranker** (jarvis `rank.rs`). bm25 with stemming is already
  compiled into the sqlite we ship; no code to keep.
- **Embeddings now.** A model dependency, a vector store, and a second
  ranker for a base that is hundreds of documents. The FTS table can grow a
  vector column later (§3.3 ponytail note).
- **Harvest every turn.** Doubles the calls of a chatty session for facts
  that mostly arrive during tool work; the tool-turn counter and the
  compaction hook cover both ends.
- **Harvest as a tool loop** (let the model call `knowledge_put` in the
  continuation). A second dispatch path, approvals, and a multi-round
  branch off the frame; parsing a text reply is the summariser's proven
  shape.
- **A utility model for harvest/summary.** A different model is a different
  cache: the full prefix at the write rate, every time. Same model, hot.
- **Whole bundle in memory** (jarvis `Index`). Files plus rows is less state
  and survives restarts for free.
- **A soft memory sibling** (Hindsight). Compaction notes carry the soft
  context inside a session; anything worth keeping across sessions is a
  concept with evidence, or it is not worth keeping.
- **Graph UI / concept editor.** The bundle is a directory of markdown; the
  user's editor and `knowledge_history` are better than anything we would
  build.
- **The system `git` for the bundle's history.** Right for skills (network,
  keys, proxies — 04 §2.2), wrong here: the feature would vanish on a
  machine without `git`, and brush is a shell, not a git. `gix` without
  default features is local-only, pure Rust, and writes a `.git` the real
  git reads (§2.4).
- **`git2` (libgit2).** A C library and its own build story for the same
  local slice `gix` covers.
- **A snapshot directory instead of a repo** (`.history/<id>/<ts>.md`).
  Fewer crates, but no diff, no atomic multi-file batches, no
  interoperability with the user's own git tooling.
- **Concept ids in the todo or steering path.** Nothing else in the loop
  learns about knowledge; hints and the head are the only touch points.

## 15. Edits to other docs (applied 2026-09-10)

- `07-context.md` §2.3 table: rows `<knowledge> hints | history tail | turn
  start / every 4 rounds with hits | append` and `harvest | dead-end branch
  off the finished frame | every 3rd tool-using turn, before compaction |
  no effect on the session's prefix`; §8.4 head: `Knowledge: read …` after
  `Runtime:`; §8.7: three note kinds; §11 types: `SessionState` fields.
- `00-architecture.md` §9 (data dir): `knowledge/`, `knowledge.sqlite`.
- `02-models-and-menus.md` §4 command list: `/knowledge`.
- `04-prompt-skills-mcp.md` §1.1: the guidance sentence; a pointer that
  the knowledge index is deliberately *not* part 4's sibling. §2.2: the
  `git2`/`gix` rejection is scoped to clone/pull; the knowledge bundle's
  local history uses `gix` (10 §2.4).
- `08-repl.md` §3.2: `knowledge_*` appear in the runtime index like any
  registered tool.
- `README.md`: a `Knowledge:` paragraph after `Skills:`; `knowledge/` in the
  data-dir line.
