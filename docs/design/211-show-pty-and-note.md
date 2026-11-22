# 211 — show_pty and show_note: open a terminal or a note in a tile

Status: contract for the build (2026-11-22). Siblings of `show_web` (design 205). Owner request: "like we have the show_web, I also want show_pty, show_note so the agent can open those in a tile for the user as well."

This file is the fixed contract between the backend builder and the UI builder. Do not change the names, topics, fields or receipt lines below without the orchestrator.

## 1. Tools (crate `artifact`, new file `crates/artifact/src/tileview.rs`)

Both: `RISK = Risk::ReadOnly`, `DIRECT = true`, `RETURNS = "dict"`, registered next to `ShowWebTool` in `crates/artifact/src/lib.rs` (~L877), re-exported like `ShowWebArgs`/`ShowWebTool`/`WebViewOpened` (lib.rs ~L62).

### show_pty{job, title?}
- `job: String` — a PTY job id (`j3`) or the `name` given to `start` (same resolution rule as `send`/`screen`: exact id, then a running job with that name, then any job with that name — `Jobs::pty`, `crates/tools/src/pty.rs` ~L1237).
- `title: Option<String>` — tile title; default the job's name, else its command (flatten to one line, ≤120 chars, reuse `clean_title` from webview.rs).
- Validation: only jobs of the calling session (`cx.session`). Use `ListTerminals { sessions: vec![cx.session] }` on the Jobs actor (`JOBS_ACTOR`, `crates/tools/src/pty.rs` ~L1112 / handler ~L1765) — it lists retained PTY terminals (running or finished) and already excludes cell kernels. Resolve name→id on those rows. Errors (ToolOutput::err, prefix `show_pty: `): unknown job/name; job is not a PTY (started with `shell`, not `start`) — say "start it with start() to get a terminal"; job evicted. A finished job is accepted (its final screen is still readable); say so in the receipt.
- Event `PtyViewOpened`, topic `artifact.pty-view-opened`:
  `{ type:"PtyViewOpened", session: SessionId, call_id: String, job: String /* resolved jN */, name: Option<String>, title: String, running: bool }`
- Receipt, first line EXACT (the UI parses it, the transcript keeps no structured result):
  `pty view opened · {title} · tile pty:{session}/{job}`
  Following lines: if `!crate::preview::feed_live()` the same "no app is connected" note show_web uses; then: "The user can type into this terminal and the tile resizes it; you keep using screen()/send() as before." If finished: "The job has finished; the tile shows its final screen."
- `structured: {session, job, name, title, running}`.

### show_note{doc, block?, title?}
- `doc: String` — the same grammar as the `doc_*` tools: a DocId, `knowledge:<concept>` (or a bare `a/b` concept id), `meeting:<id>` (its note). Resolve with the document crate's resolver (`crates/document/src/tools.rs` ~L97-L215, `Docs::resolve` is private — expose a `pub` helper in the `document` crate rather than duplicating it). Then fetch `DocMeta` with `MetaOf{doc, by: Principal::Agent{session}}` (actor.rs ~L1744) for kind/title and the admission check.
- Accepted kinds: `Note`, `Board`, `Knowledge`, `MeetingNote`. Others (`Notebook`, `Source`, `Artifact`, `AgentCharter`, `TaskThread`) → `show_note: <kind> documents do not open in a note tile` + a hint (notebooks/sources: their pages; artifacts: `show`).
- `block: Option<String>` — a block id to scroll to (optional; UI may ignore if unsupported, but carry it).
- `title: Option<String>` — default the document's title.
- Event `NoteViewOpened`, topic `artifact.note-view-opened`:
  `{ type:"NoteViewOpened", session: SessionId, call_id: String, doc: String /* resolved 26-char DocId */, kind: String /* "note"|"board"|"knowledge"|"meeting_note" — use DocKind's serde name */, title: String, block: Option<String> }`
- Receipt first line EXACT:
  `note view opened · {title} · tile note:{doc}`
  then `theuth://notes/{doc}` on line 2 (append `#{block}` when given), then the no-app note when applicable, then: "It updates live as you edit it with doc_insert/doc_replace."
- `structured: {session, doc, kind, title, block}`.

### Prompt
Add ONE paragraph to `PROMPT` in `crates/artifact/src/tools.rs` right after the "Two browsers, two jobs." paragraph (keep PROMPT a pure constant):
"`show_pty{job, title?}` and `show_note{doc, block?, title?}` open a tile beside the chat the same way: a terminal you started with `start()` (by id or name) that the user should watch or type into — a dev server's log, a long build, an interactive prompt only they can answer — or a note, board or knowledge page you want them to read or edit alongside the chat. The same job or document again reuses its tile. Use them when the user should see it now; a link or a summary is enough otherwise."
Also one sentence in each tool description saying what it is for and that `show_web` is for web pages, `show` for files.

## 2. Wire plumbing (backend builder)
- `rt.register_wire_event::<PtyViewOpened>()` and `::<NoteViewOpened>()` in `declare_wire` (artifact lib.rs ~L783).
- Add both to `republish!` in `crates/rpc-compose/src/client_mirror.rs` (~L85).
- Add `("PtyViewOpened","artifact.pty-view-opened")`, `("NoteViewOpened","artifact.note-view-opened")` to `WIRE_RELAYED` in `crates/sessionsync/src/events.rs` (~L123) and to its pin test (~L1360).
- Golden: `family::<artifact::PtyViewOpened>(…, "artifact.pty-view-opened")` and the note one in `crates/rpc-compose/tests/wire_golden.rs` (~L115) and `wire_parity_inc5.rs` (~L55); fixtures `crates/rpc-compose/tests/fixtures/wire/artifact.pty-view-opened.json` and `…note-view-opened.json` copied from `artifact.web-view-opened.json`'s shape.
- Name lists: both names in the two `"output"` category lists in `crates/tools/src/builtins.rs` (~L217 and ~L467); Slack `is_deliverable` in `crates/slack/src/bubble.rs` (~L631); `crates/theuth/harness.md` tool list (~L182).
- Schema pin `crates/theuth/tests/fixtures/wire/openrpc-v1.json` and TS bindings: the ORCHESTRATOR regenerates these (`just ts-desktop`, `just wire-schema-accept`). Builders do not.

## 3. UI (UI = `crates/desktop/ui/src`)
- Listener in `App.tsx` next to the `artifact.web-view-opened@1` one (~L583): `on("artifact.pty-view-opened@1", 'PtyViewOpened', e => ptyViewOpened(e, visible(e.session)))` and the note equivalent.
- New `lib/tile-arrival.ts`: `ptyViewOpened(e, visible)` / `noteViewOpened(e, visible)`. If `visible && tileable(e.session)` → open the tile via `openTile` (so selected-tile takeover, placement and dedupe come from `lib/tiles.ts`). Otherwise do nothing (phone / hidden chat keep the transcript card).
- PTY tile: reuse kind `job-terminal` (`components/tiles/kinds/terminals.tsx`), key `${session}/${job}`, props `{session, job, label: title}`. Generalise `openJobTerminalTile` in `components/tiles/open.ts` to accept a `JobRef` + label (keep the existing callers working).
- Note tile: new kind `note` (`components/tiles/kinds/note.tsx`, registered in `components/tiles/registry.tsx`, opener `openNoteTile(host, {doc, block?, title?})` in `open.ts`, key = doc). Render `TileFrame` around `<Capability required={panelRequirements}>` + `<NoteView id={doc} embedded />` (`components/notes/NotePage.tsx` ~L106; meeting `Note.tsx` already embeds it). Avoid a double header. Pop-out: `openPopout({kind:'note', doc})`. Live updates come from `useDocument` already. Scroll to `block` if cheap.
- Transcript cards: `components/chat/PtyViewCard.tsx` and `NoteViewCard.tsx`, branches in `MessageTool.tsx` (~L95, next to `show_web → WebViewCard`), both names in `DELIVERABLE_TOOLS` (`ToolRunCard.tsx` ~L27). Each parses the receipt's first line (regexes below), shows title + an Open button (re-opens/places the tile; on phone or non-tileable: PTY → `openPopout({kind:'pty',session,job})` when `canPopout()` else the existing terminal sheet/route `/pty/$session/$job`; note → navigate `/notes/$id`) and a pop-out button when `canPopout()`.
  - PTY: `/^pty view opened · (.*) · tile pty:([^/\s]+)\/(\S+)$/m`
  - Note: `/^note view opened · (.*) · tile note:([0-9a-z]{26})$/m`
- Until the orchestrator regenerates bindings, the UI builder hand-writes the two generated binding files (git-ignored) at `UI/rpc/bindings/` named like the existing `event.artifact_2eweb_2dview_2dopened.output.ts` (`event.artifact_2epty_2dview_2dopened.output.ts`, `event.artifact_2enote_2dview_2dopened.output.ts`) and adds whatever index/map entry `on()` typing needs, mirroring the web-view one exactly. The regenerated ones will replace them.
- Not in scope: Inbox items, header globe, a `web_views`-style state tool, phones beyond the card.

## 4. Tests
- Rust (inline `#[cfg(test)]` in tileview.rs, modelled on webview.rs tests): receipt first-line pins for both tools, event wire shape (type + keys), name→id resolution, refusals (unknown job, non-PTY job, wrong doc kind).
- UI: `lib/tile-arrival.test.ts` (visible→tile opened with right kind/key; same event twice → one tile; not visible → no tile), receipt-parser tests for both cards, opener tests in `lib/tiles.test.ts` if touched.
