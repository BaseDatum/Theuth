# 212 — show_file and show_diff: source and diffs in a tile

Status: contract for the build (2026-11-23). Siblings of `show_web` (205), `show_pty`/`show_note` (211, `crates/artifact/src/tileview.rs`). Owner: "show_file opens a file syntax highlighted in a tile for the user to see, not like show markdown — raw file content with syntax highlighting of source files. show_diff is specifically git related: a colorized diff based on git history or comparing two files, also in a tile."

Fixed contract between the backend and UI builders. Do not rename anything here without the orchestrator.

## 0. Principle
Events and receipts carry a small DESCRIPTOR, never content. The tile fetches content through new RPCs owned by the `artifact` crate, forwarded to the session's owner engine (pattern: `job_log` in `crates/agent/src/rpc_jobs.rs` with `authorize_fwd` + `forward::route`). So the web client, iOS and front-door clients work, and a card's Open after a restart re-fetches from the receipt alone.

## 1. Tools (crate `artifact`, new file `crates/artifact/src/codeview.rs`)
Both: `Risk::ReadOnly`, `DIRECT = true`, `RETURNS = "dict"`, registered next to `ShowPtyTool`/`ShowNoteTool` in `lib.rs`, re-exported likewise. Titles via `clean_title`. Relative paths resolve with `tools::seat::resolve(cx.session, …)`; the repo root from `tools::seat::cwd(cx.session)` + `gix::discover::upwards` (see `crates/codeindex/src/root.rs`) or `git -C <cwd> rev-parse --show-toplevel`.

### show_file{path, line?, end_line?, title?}
- Resolve, `refuse_cell_record` (`crates/tools/src/seat.rs`), must be a regular file, ≤ 2 MiB, not binary (NUL in the first 8 KiB, or an image per `tools::images::sniff`) → `show_file: … is binary — use show for images and documents`.
- `line`/`end_line` 1-based; `end_line` requires `line`; `end_line ≥ line`. Lines past EOF → error naming the file's line count.
- Default title: the file name. Language: from the extension (UI maps it; the backend sends `lang` as the lowercase extension or a well-known filename like `Dockerfile`/`Makefile`/`justfile`).
- Event `FileViewOpened`, topic `artifact.file-view-opened`:
  `{type:"FileViewOpened", session, call_id, id, path /*absolute*/, title, lang, line: Option<u32>, end_line: Option<u32>, size: u64, lines: u32}`
- `id` = first 16 hex of sha256(session + "\n" + absolute path). Same file again → same tile.
- Receipt, EXACT first two lines:
  `file view opened · {title} · tile file:{id}`
  `path: {absolute path}` + (`#L{line}` or `#L{line}-L{end_line}` when given)
  then `{lines} lines, {size} bytes.`, the no-app note when `!crate::preview::feed_live()`, and "The user sees the file as it is on disk now; it does not follow later edits until they press Refresh."
- `structured: {session, id, path, title, lang, line, end_line, size, lines}`.

### show_diff{commit?, range?, staged?, file_a?, file_b?, paths?, title?}
Exactly one mode (error otherwise; no mode given = worktree):
- `worktree` (default): working tree + index vs HEAD, untracked files included (shell: `git diff HEAD` plus untracked via `git ls-files --others --exclude-standard` rendered as new-file diffs, or `git add -N`-free equivalent — do NOT modify the index).
- `staged: true`: `git diff --cached`.
- `commit: "<rev>"`: that commit vs its first parent (root commit vs empty tree) — `git show --format= --no-color <sha>` or `git diff <sha>^ <sha>`.
- `range: "A..B"` or `"A...B"`: `git diff A..B` / `A...B`.
- `file_a` + `file_b`: two files (need not be in a repo), unified diff with `similar` (workspace dep `similar = "3"`; add to artifact's Cargo.toml), headers `--- a/<file_a>` `+++ b/<file_b>`.
- `paths: [..]` limits git modes to pathspecs (relative to the repo root or cwd-resolved, git semantics).
Git runs as a subprocess: `git -C <root> -c core.quotepath=off diff --no-color --no-ext-diff -M …`, hooks/pager off, env scrubbed of GIT_* (copy the spelling of `crates/autoresearch/src/git.rs` `diff_bytes`; don't depend on the autoresearch crate — copy the small helper). Commit/range revs are resolved to full shas at CALL time (`git rev-parse --verify <rev>^{commit}`); unknown rev → `show_diff: unknown revision …`. Not a git repo (git modes) → error naming the cwd.
- The call computes the diff once to validate and count: files, +added, −removed. Empty diff → still opens, receipt says "no changes".
- Spec (what the tile re-requests), serde-tagged: `DiffSpec { mode: "worktree"|"staged"|"commit"|"range"|"files", root: Option<String>, a: Option<String> /*sha, or file_a abs*/, b: Option<String> /*sha, or file_b abs*/, three_dot: bool, paths: Vec<String> }`.
- Event `DiffViewOpened`, topic `artifact.diff-view-opened`:
  `{type:"DiffViewOpened", session, call_id, id, title, spec: DiffSpec, files: u32, added: u32, removed: u32}`
- `id` = first 16 hex of sha256(session + "\n" + canonical JSON of spec). Default title: `working tree` / `staged` / `<short sha> <subject ≤60>` / `A..B` / `<file_a name> ↔ <file_b name>`, then ` · <paths joined>` when paths given.
- Receipt EXACT first two lines:
  `diff view opened · {title} · tile diff:{id}`
  `spec: {compact JSON of DiffSpec}`
  then `{files} files changed, +{added} −{removed}.` (or `no changes.`), the no-app note when applicable, and for worktree/staged "It shows the tree as it is now; the user presses Refresh to see later changes."
- `structured: {session, id, title, spec, files, added, removed}`.

### Prompt
Append ONE paragraph to `PROMPT` in `crates/artifact/src/tools.rs` right after the show_pty/show_note paragraph (pure constant):
"`show_file{path, line?, end_line?, title?}` opens a source file in a tile, raw and syntax highlighted with line numbers, scrolled to and marking the lines you name — for code the user should look at, not rendered documents (`show` renders markdown, PDFs and images). `show_diff{commit? | range? | staged? | file_a+file_b, paths?, title?}` opens a colorized diff in a tile: the working tree against HEAD by default, staged changes, one commit, a range like `main..HEAD`, or two files. Use them when you point the user at code or a change instead of pasting it into the reply."
Each tool description: one sentence of purpose + "show is for rendered documents; show_web for web pages".

## 2. RPCs (crate `artifact`, next to `web_view_state` in `crates/artifact/src/rpc_artifacts.rs` or a sibling trait the same way)
- `file_view_read(session: SessionId, path: String) -> FileViewText { text: String, size: u64, lines: u32, mtime_ms: u64, truncated: bool, lossy: bool, lang: String }`
- `diff_view_read(session: SessionId, spec: DiffSpec) -> DiffViewText { text: String /* unified patch */, files: Vec<DiffFileStat{path, status /*A|M|D|R|?*/, added, removed}>, added: u32, removed: u32, truncated: bool }`
Both: `authorize_fwd` + forward to the owner engine like `job_log`. Caps: file text 2 MiB (truncate on a char boundary, `truncated: true`); diff text 1 MiB (append `\n[diff truncated at N bytes]`, `truncated: true`).
Authorization of what may be read (closing the arbitrary-read hole): a path/spec is allowed if (a) it was published by a show_file/show_diff call of that session in this engine lifetime (in-memory set keyed by session), OR (b) every path it reads is inside the session's seat cwd or the git repo root containing it (canonicalised; symlinks resolved), and never a cell record (`refuse_cell_record`). Otherwise error `not a shown file` / `not a shown diff`. This keeps card Open working after a restart for anything inside the project.
Ts-rs + schemars on all new types like the other artifact RPC types. Methods must appear in the generated bindings (`method.artifact.*`).

## 3. Wire plumbing (backend builder)
Same list as 211: `register_wire_event` ×2; `republish!` in `crates/rpc-compose/src/client_mirror.rs`; `WIRE_RELAYED` + pin test in `crates/sessionsync/src/events.rs`; goldens in `crates/rpc-compose/tests/wire_golden.rs` and `wire_parity_inc5.rs` with fixtures `artifact.file-view-opened.json` / `artifact.diff-view-opened.json`; names in both `"output"` lists of `crates/tools/src/builtins.rs`, Slack `is_deliverable`, `crates/theuth/harness.md`. Openrpc fixture + TS bindings: ORCHESTRATOR regenerates.

## 4. UI (UI = `crates/desktop/ui/src`)
- Listeners in `App.tsx` beside the pty/note ones; handlers `fileViewOpened`/`diffViewOpened` in `lib/tile-arrival.ts` with the same `visible && tileable` guard.
- Openers in `components/tiles/open.ts`: `openFileTile(host, {session, path, id, title, lang?, line?, end_line?})` key = id; `openDiffTile(host, {session, id, title, spec})` key = id.
- `components/code/FileView.tsx` (new): source viewer — numbered gutter, `data-line` rows, marked range tint (Excerpt's look, `components/chat/surface/Excerpt.tsx`, but NOT mounting Excerpt: it needs chat contexts), highlighting via `lib/shiki.ts` `tokensOf`/`shikiLang` (map extension/filename → lang; unknown → plain), `@tanstack/react-virtual` for rows, scroll to `line` on load, plain text while tokens load, highlight in chunks or only up to ~5k lines (plain beyond, say so in the header meta). Fetch with `file_view_read` via TanStack `useQuery`; loading skeleton, error text (`could not load …`).
- `components/code/DiffBody.tsx` (new): full-height multi-file diff — reuse `parseUnified` (`lib/editDiff.ts`) and the row/word-mark rendering of `components/chat/DiffView.tsx` (extract the file-section renderer; parametrise the 400-row cap and the card chrome so the existing DiffView keeps its look and tests). Per-file collapsible sections with path, status and +N −M; sticky file headers. Unified view only (split view is out of scope). Fetch with `diff_view_read`.
- Tile kinds `file` (`kinds/file.tsx`) and `diff` (`kinds/diff.tsx`) in `registry.tsx`: `TileFrame` title, meta (`path · lang · N lines` / `N files +A −R`), controls: Refresh (refetch), Copy (file text / patch). No pop-out (no PopoutTarget) for now.
- Cards `components/chat/FileViewCard.tsx`, `DiffViewCard.tsx`: parse the receipt's first two lines:
  - File: `/^file view opened · (.*) · tile file:([0-9a-f]{16})$/m` and `/^path: (.+?)(?:#L(\d+)(?:-L(\d+))?)?$/m`
  - Diff: `/^diff view opened · (.*) · tile diff:([0-9a-f]{16})$/m` and `/^spec: (\{.*\})$/m` (JSON.parse)
  Fall back to call args while pending. Open → the tile when tileable; otherwise (phone / no pane) expand the content inline in the card (FileView / DiffBody with a capped height). Show `+A −R` on the diff card. Branches in `MessageTool.tsx`, both names in `DELIVERABLE_TOOLS`.
- Hand-write the generated bindings for the two events and two RPC methods (git-ignored, mirror the existing ones incl. the runtime `TOPICS` map and method typing) so tsc passes; the orchestrator regenerates them.

## 5. Tests
- Rust (inline in codeview.rs; git tests make a temp repo with `git -c commit.gpgsign=false`): receipt first-two-line pins for both tools; event wire shape; path resolution and refusals (binary, too big, cell record, bad line range); diff modes worktree (incl. untracked), staged, commit, range, files, paths; unknown rev; read-authorization (shown path ok, inside cwd ok, outside refused); truncation.
- UI: receipt parsers, tile-arrival for both kinds, DiffBody file-section parsing, FileView lang mapping and chunking decision (pure functions).


## 6. Amendment (owner, 2026-11-23 13:57) — show_file follows edits on its own

Owner: "file should follow edits on its own." This REPLACES the "does not follow later edits / Refresh" parts of §1, §2 and §4 for show_file:

- New subscription RPC (artifact crate, same forwarding + authorization as `file_view_read`): `subscribe_file_view(session, path)` / `unsubscribe_file_view`, item `FileViewChange { mtime_ms: u64, size: u64, deleted: bool }`. Copy the subscription shape of `subscribe_pty` / `unsubscribe_pty` (`crates/agent/src/rpc_jobs.rs`) for the jsonrpsee subscription plumbing and forwarding.
- Server side: watch with the workspace `notify = "8"` crate (patterns: `crates/document/src/watch.rs`, `crates/schedule/src/lib.rs`). Watch the file's PARENT directory non-recursively and filter to the file name (editors save by rename/replace). Debounce ~150 ms; coalesce; emit one `FileViewChange` per settled change; emit `deleted: true` when it disappears and resume when it reappears. One watcher per (path) shared by all subscribers (refcount; drop on last unsubscribe). If the watcher cannot be created, fall back to a 2 s mtime poll for that subscription.
- `file_view_read` gains an optional `if_mtime_ms: Option<u64>`; when equal to the current mtime it returns `unchanged: true` with empty text (field `unchanged: bool` added to `FileViewText`).
- UI: the file tile subscribes while mounted; on a change it refetches `file_view_read`, keeps the scroll position (anchor on the top visible line), briefly tints lines that changed (simple line-by-line compare of old vs new text, only when ≤ 5k lines), and shows "deleted" state in the header meta when the file is gone. Keep a manual Refresh control too. If the subscription method is unavailable (older engine), fall back to refetching every 3 s with `if_mtime_ms`.
- Receipt: replace the last sentence with "The tile follows the file as it changes on disk." (Receipt first two lines unchanged.)
- show_diff is unchanged (Refresh button), unless the orchestrator says otherwise.
- Tests: Rust — watcher fires on write and on rename-replace, deleted/reappear, debounce coalesces, unsubscribe drops the watcher (use a temp dir; allow generous timeouts); UI — the scroll-anchor and changed-lines helpers as pure functions.
