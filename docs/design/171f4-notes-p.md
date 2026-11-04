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

## For theuth (engine, not changed)
- `get_artifact` and `get_artifact_listing` (`crates/rpc/src/api.rs` ~8717-8749) take no extensions: any peer that
  knows a private artifact's `call_id` gets its record and an archive's listing, unlike `get_attachment`, which
  authorizes the owner. Worth a look against 51 §4.2.
