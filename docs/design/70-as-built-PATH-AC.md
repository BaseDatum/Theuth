# 70 — as built: PATH-AC

Branch `feat/path-autocomplete`. Path autocomplete in the TUI editor and the
desktop/mobile composer: a token that looks like a path (`~/Dev`, `./src/`,
`$HOME/.th`, `/etc/ho`) completes against the file system of the engine that
runs the session. `~`, `~user` and `$VAR` are expanded for the lookup only;
the text in the box keeps what the user typed (`~/Dev` → `~/Develop/`).

## RPC

`path_complete(session: SessionId | null, input: string, cursor: usize) -> PathCompletion`
(`crates/rpc/src/api.rs`). `cursor` is a UTF-8 byte offset into `input`.

```
PathCompletion { token_start, token_end, candidates: [PathCandidate], truncated, platform }
PathCandidate  { insert, display, is_dir }
```

- `insert` is the whole token after accepting: the typed head verbatim plus
  the entry name, `/` for a directory (`\` when a Windows engine and the token
  used `\`). Clients replace `[token_start, token_end)` with it, so only the
  remainder past the typed prefix lands.
- `platform` is the listing engine's `std::env::consts::OS`; `windows` lets
  the client trigger on `C:\` / `C:/` too.
- Principal: `peer(ext)` then `authorize_opt` — the same admission as
  `send_input`. `session: null` completes on the connected engine against its
  cwd (the new-chat pane).
- Owner routing: `forward::owner_peer(session)` → `forward::to_peer(…, "path_complete", …)`
  when another engine owns the session (agent desk, remote user session). A
  read-only call never `Take`s the session. Unreachable owner, transport
  error or a forwarded error → `PathCompletion::default()` (empty), never an
  error card.
- cwd: `store.session(id).cwd` (ACP `session/new` / `spawn_subagent.cwd`),
  else the engine's `current_dir()`, else `$HOME`.

## Listing (`crates/rpc/src/pathac.rs`)

- `trigger(input, cursor, windows) -> Option<(start, end)>`: the token
  `[start, cursor)` starts with `/`, `./`, `../`, `~`, `~user/`, `$VAR/` or
  (Windows) `X:\`/`X:/`; no trigger inside an open ``` fence, after `@`
  (attachment mention) or on a token with spaces — unless the token was
  opened with `"`/`'` (quote stays outside the range) or the spaces are
  `\ `-escaped. Ported verbatim to `crates/desktop/ui/src/lib/path-complete.ts`
  (`pathTrigger`, UTF-16 offsets) with the same vectors in both test suites.
- `complete(cwd, input, cursor)`: one `read_dir` of the directory the head
  names (no recursion). Case-sensitive prefix on the last segment; hidden
  entries only when the segment starts with `.`; names with control chars or
  invalid UTF-8 skipped; symlinks resolved for `is_dir`; sorted directories
  first then by name; `LIMIT = 50` with `truncated`. Unquoted inserts escape
  spaces as `\ `. Missing/unreadable dir or unknown `$VAR` → empty.
- `~user` resolves to a sibling of `$HOME` (`/Users/<user>`, `/home/<user>`);
  `ponytail:` getpwnam if that ever misses.

## TUI (`crates/tui/src/pathac.rs`, `ui.rs`)

`PathState` on the model. Tab on a path token asks (`Command::PathComplete`
→ `UiEvent::PathComplete(generation, Option<PathCompletion>)`); the list sits
in a bordered box under the editor (`path_box`, 8 rows + `…`). ↑/↓ move, Tab
accepts and, for a directory, asks again; Esc closes; Enter closes and
submits as usual. Typing while the list is up re-asks after 120 ms
(`due` + `wake()`/`tick()` from the frame loop). Replies carry the generation
that asked; stale ones are inert; a failed call (`.ok()`) or empty listing
closes the list. The slash popup keeps Tab while it has matches. Works over
`rpc::api::Api`, so an attached TUI completes on the engine it is attached to.
`on_change` has no caret: the pre-edit caret is shifted by the edit length
(right for typing and Backspace; Delete lands one char short until the
re-ask).

## Composer (`Composer.tsx`, `lib/path-complete.ts`)

`onChange` → `pathChanged(text, caret)`: no trigger, a live slash command or
an open `@` picker closes the list; otherwise a 120 ms debounced
`api.path_complete(session, text, byteOffset)` (seq-guarded) opens
`PathList` — the `SlashList`-style popup anchored above the box; on a phone
(`useIsMobile`, < 768 px) a horizontal chip row, tap to accept. Keys in
`onKeyDown` after the slash block: ↑/↓ (and ⇧Tab) cycle, Tab / → at the end
of the draft accept (a directory keeps completing), Esc closes and remembers
the token start so it does not reopen until edited past. `accept()` replaces
`[start, end)` with `insert`. `pathOpen` re-validates the token against the
current draft so a send or recall leaves no stale rows.

## Verification

- `cargo test -p rpc pathac` (trigger vectors, expansion for lookup only,
  hidden filtering, dirs first, quoting/escaping, `~`/`$HOME` inserts keep
  the prefix, missing dir/no trigger/unknown var → empty, truncation);
  `cargo test -p tui pathac` (Tab open → stale reply inert → navigate → accept
  dir re-asks → file closes; debounce, Esc, Enter, empty reply, leaving the token).
- vitest `lib/path-complete.test.ts` (same trigger vectors; keys, cycling,
  accept inserts the remainder, dir keeps triggering); full suite 99 files /
  579 tests incl. `no-emoji` and `rpc/api.test.ts` (METHODS = methods.json).
- Manual, scratch engine `THEUTH_DATA_DIR=/tmp/69/pathac-data`: desktop UI
  (`bun run dev` + `VITE_THEUTH_URL`, driven in Chromium) `~/Dev` → `Develop/`;
  Tab → `~/Develop/`; `th` Tab → `~/Develop/theuth/`; `cr` Tab →
  `~/Develop/theuth/crates/` with the crate list; `$HOME/…` stays `$HOME/…`;
  Esc closes and stays closed for that token; `@~/Dev` opens the attach
  picker, not the path list; 390 px viewport shows the chip row and a tap
  accepts. TUI `t` attached to the same engine (pty + xterm-headless):
  `~/Dev` Tab → list `Develop/`; Tab → `~/Develop/` + its listing with `…`;
  ↓ moves; Tab accepts a directory and lists it; Esc closes.
