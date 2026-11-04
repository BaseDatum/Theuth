# 171F4-P — artifacts in private chats never render (notes)

Bug: in a private chat (design 51) every artifact body read was `get_attachment(path)` with no session. The engine
serves a private tree's files only when the owning session is named (51 §4.2, `crates/rpc/src/api.rs` `get_attachment`:
no session → root `data_dir` → a scratch path is `-32000 "<path>: not an attachment"`). Every text kind (html, markdown,
text, code, csv, mermaid) fell to `''` → "Unable to load" / "could not load"; images, svg, audio and pdf failed the same
way, and on the local daemon `asset://` (scope `data_dir` only) could not reach the scratch dir either.

## Decisions
- **UI only; the engine rule stays.** `lib/files.ts`: `resolveFile`, `readBytes`, `resolveDocument`, `readText`,
  `readTextOnline` take an optional `session`. Set → `get_attachment(path, session)` and never `convertFileSrc`;
  unset → the one-argument durable call (unchanged).
- **Explicit session where the caller has one.** `privateOwner(session)` (`lib/private.ts`) is the session when it is in
  the owned tree, else `undefined`, so a durable artifact never names a session (`authorize` fails closed on unknown or
  deleted ids). Callers: `ArtifactCard` (`useText`, preview `Img`, Copy, Download), `Viewer` (text kinds, image/svg,
  pdf frame and pdf.js pages, audio), `AppHost` (source fallback), the explorer `List` thumbnails, and `Attachments`
  (thumbnails, lightbox image/pdf, from its `session` prop). `useText`, `useFileUrl`, `useDocumentUrl` and `Img` take
  the optional `session`.
- **Fallback for renderers without a session**: the composer's held strip, queued chips (`QueuedChips.tsx`, worker N's
  file, not edited) and the attach picker. In `files.ts`, a path under `/theuth-private-<32 hex>/` with no explicit
  session names the owned tree's root (`usePrivate.getState().current?.root`). The root is the tree's owner, so the
  engine authorizes it the same as a child.
- **Archive listing** needed nothing: `get_artifact_listing` reads by `call_id` through the private-aware index; its
  Download goes through `saveAs` (now session-aware remotely; `export_artifact` locally, already private-aware).
- **Caches go with the tree.** Paths fetched under a private session are tracked; `forgetPrivate()` revokes their
  blob URLs and drops them from the `lru` and `texts` caches. `discard()` (every release path: leave, engine switch,
  `SessionClosing`, failed resume) calls it.
- **No pop-out for private artifacts.** `PopoutButton` renders nothing when the artifact's session is in the owned
  tree: a second window is a different peer and the engine would refuse it. Covers the card, the panel, the explorer
  viewer and the explorer list. Save/Download stay.
- `files.ts` ↔ `private.ts` import each other; both uses are call-time only (no module-level reads), so the cycle is
  harmless.

## Follow-up: artifact RPCs get the `get_attachment` rule (engine, `crates/rpc` + `crates/artifact`)
- What was actually open at `2a4bb91c`: `export_artifact` (any store by id, no peer) copied a private blob for any
  peer; `get_app` (the app plugin's `GetRecord`, any store) served a private app record; `publish_app` wrote into any
  session, private ones included. `get_artifact` and `get_artifact_listing` searched every store but `State::get`
  checks the owner against the supervisor's durable session list, so a private record always came back "owning
  session no longer exists" — to everyone, its owner included (the panel's `openShow` fallback never worked in a
  private chat).
- Rule: `get_artifact`, `get_artifact_listing`, `export_artifact`, `get_app` take an optional trailing `session`,
  authorized with `authorize_opt` (the same helper `list_artifacts` and `get_attachment` use). The artifact actor's
  `search::Get` / `Export` carry it and look up `Stores::visible(session)`: the durable store, plus the session's
  private store when it is a tree member. No session → durable only. Another peer naming the session →
  `-32000 unauthorized` from `authorize`; no session → `-32000 "<call_id>: no such artifact"` (the durable miss).
- `get_app` checks visibility with `search::Get` in the RPC handler before asking the app plugin, so `crates/app`
  is unchanged; `-32001` (apps disabled) still comes first. `publish_app` authorizes its `session`.
- `artifact_search_match` reads the durable store only (`ReadSearchMatch`): a private tree is never indexed.
- `State::get` skips the alive-session check for a private store (no `owner_lock`): its rows live exactly as long
  as the tree.
- Engine-internal lookups (`GetRecord`, invocations) keep `Stores::all()`: the app plugin's runtime is peer-bound
  (`app_call_tool` carries the peer).
- Callers: UI `openShow` (`get_artifact`), `saveAs` (`export_artifact`), `ArchiveView` (`get_artifact_listing`),
  `AppHost` (`get_app`) pass `privateOwner(...)`; the explorer, attention and notebook callers stay session-less
  (durable; navigating there leaves a private tree). TUI: `/artifacts` export names the owned tree's session, a
  search hit none.
