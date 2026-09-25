# 70 — as built: CHARTER-DOC (the agent charter is a document with a chat beneath it)

Sky's ruling (2026-09-25): the Charter step of Hire an agent and the Charter tab of an agent's detail are (1) a
BlockNote editor — a real document an agent can edit like any note (`doc_read`/`doc_insert`/`doc_replace`/`doc_delete`
over the `doc` id) — and (2) a full chat beneath it so the user writes the charter with theuth. Clarified mid-build:
the detail tab (`/agents/$name/charter`, an existing agent) is the same editor + chat, first-class, not a follow-up.
Branch `feat/charter-document-chat`.

## Commits

1. `feat(document,roster): agent charter as a document — Binding::AgentCharter, doc_ensure/doc_markdown RPCs, roster ⇄ document sync`
   - `crates/document/src/meta.rs`: `DocKind::AgentCharter` (`"agent_charter"`), `Binding::AgentCharter(String)`
     (key `{"agent_charter":"<name>"}`; `DocId::for_binding` = uuid v5 of that, so the wizard and the detail tab
     land on the same id), `derived` → `documents/<id>/charter.md` (body only; the roster splices it under the file's
     front matter — `agents/<name>.md` itself is never the derived file, the YAML header stays the roster's).
     `store.rs::rename` re-derives the path for charters like artifacts. Tests extended: `bound_ids_are_deterministic…`,
     `binding_key_and_derived_paths`.
   - `crates/document/src/tools.rs`: `Docs::resolve` accepts `agent-charter:<name>` (Lookup; a clear error when the
     Charter tab was never opened) beside `knowledge:`/`meeting:`.
   - `crates/rpc/src/api.rs`: `doc_resolve` creates an `agent_charter` binding on demand from `roster::GetAgent(name).body`
     (mirrors the knowledge arm; `theuth` → `None`). New `doc_ensure(binding, markdown)` (agent-charter only, `owner_creates`
     otherwise; the wizard's draft before the file exists) and `doc_markdown(doc)` (whole-document lossy markdown =
     what the export writes; Hire copies it). `crates/rpc/bindings/{Binding,DocKind}.ts` regenerated.
   - `crates/roster/src/lib.rs` (+ `document` dependency, cycle-free): the **sync rule**.
     - document → file: `impl Message<document::Exported> for Roster` (subscribed in `start`) → `charter_exported(name, path)`:
       reads the export, `trim`, rewrites `agents/<name>.md` through `write_text` with the same front matter and the new
       body. Ignored: unchanged text, an empty export (would make the file invalid), a name without a file (wizard
       draft), `theuth`. Timing is the existing `documents.export_quiet_secs` path.
     - file → document: `push_charter(old_body, file)` after `write_file` (`agent_put`, approve, …) and inside `reconcile`
       for every file the watcher saw change — `Lookup` then `Handle::import_markdown(doc, body, Principal::File)` (59
       §4.4 diff import; the sha check makes the document's own export a no-op, so no echo loop). Fire-and-forget; the
       roster never waits on the documents actor. Reuses the import path knowledge uses; no second watcher.
     - `RemoveAgent` trashes the charter document (`Handle::delete`).
     - Test `charter_export_replaces_the_body_and_keeps_the_front_matter`.
2. `feat(ui): charter is a BlockNote document with a chat beneath — …`
   - `components/agents/charter.ts`: `charterBinding(name)`, `charterTitle(name)` = `Charter · <name>`, `charterRef(id, who)`
     — the pinned `AttachRef` (`kind: 'note'`, the charter's DocId) whose `meta` carries the standing context: *the
     charter of agent ‹name› (‹title›, reports to ‹reports_to›; hand off to it when: ‹capabilities›). You are helping
     write it: edit it with the doc tools and keep the user's words*. One line, `[`/`]` folded to parentheses so the
     marker grammar holds. Tests: `charter.test.ts`.
   - `components/agents/CharterDoc.tsx`: `useCharterDoc(name, seed?)` (`doc_ensure` with a seed, else `doc_resolve`;
     disabled for a bad name and theuth) and `CharterDoc({who, seed?, hint, className})` — hint bar with a **Chat**
     toggle, `Live` (the notes' BlockNote over `useDocument({doc})`, agents' edits member-labelled via the reveal
     extension exactly as in notes), then `NoteChat` beneath: the note panel with its pointer-drag top edge, height /
     open / session remembered per DocId in `lib/note-chat` (localStorage `note-chat`), transcript + `Composer
     pinned={charterRef} sessionTitle="Charter · <name>"`. The session is created lazily by the Composer on the first
     message (`new_session` → `rename_session` → `send_input` with the `[attached note …]` marker leading). "Ask theuth…"
     from a block seeds the composer like the note page. Panel starts open.
   - `components/notes/NoteChat.tsx`: `about?: string` prop for the copy ("this note" → "this charter"); nothing else changed.
   - `components/agents/AgentDetail.tsx`: the Charter tab is `CharterDoc` (no Save/Discard — the document saves itself,
     the file follows on the quiet path). theuth keeps the read-only CodeMirror view with the `prompt.md` note.
   - `components/agents/HireWizard.tsx`: the Charter step is `CharterDoc who={draft} seed={starterCharter(draft)}`
     (binding `agent-charter:<name>` from the Charter step on, so names typed on Identity never mint documents). Next
     (from Charter) and Hire / Hire paused first copy the document's markdown into `draft.body` (`doc_markdown`), then
     validate and `agent_put`. Review shows that body. `wizard.ts::withCharter(d, doc, markdown)` is the pure rule;
     `wizard.test.ts` gains `hire copies the charter document into the body; an empty document fails the charter step`.
   - `rpc/api.ts`: `doc_ensure`, `doc_markdown`.
3. `chore: Cargo.lock (roster → document) and regenerated ts-rs bindings`.

## Decisions / deviations

- **Wizard binding**: the brief offered `agent-charter-draft:<name>` renamed on Hire, or copy-and-delete. Chosen:
  **neither rename nor delete — the same binding `agent-charter:<name>` from the start.** `doc_ensure` mints it from the
  starter template before the file exists; Hire copies its markdown into the file body; after `agent_put` the roster's
  `push_charter` imports the file body back (a no-op diff) and the same document is what the detail tab opens. No
  rebind step, nothing to garbage-collect. Cost: abandoning the wizard after the Charter step leaves a document bound
  to a never-hired name (trashed if that agent is later hired and removed; otherwise inert). Starting over with another
  name mints another. Acceptable for now — `ponytail:` sweep unhired charter documents if they pile up.
- **Context line**: sessions have no system-prompt/context field (`CreateSessionOptions`, `NewSession`), and Composer
  is off-limits. The ruling's sentence rides in the pinned attachment marker's `meta`, which the model reads on every
  message (66 §3: "You are helping write the charter for agent … edit it with the doc tools; keep the user's words").
  The document is "attached" the 66 way (`[attached note <id> "Charter · <name>" · …]`); `doc_read(<id>)` works.
- **Derived file**: `documents/<id>/charter.md`, not `agents/<name>.md`. The exporter emits YAML front matter only for
  knowledge and the frontmatter block would render a knowledge form in the editor; the roster owns the header and
  splices the body. External edits of `agents/<name>.md` still reach the document (reconcile → `push_charter`).
- **theuth**: `prompt.md` stays read-only CodeMirror, no chat. Making it a document would need a `prompt.md` export
  path and a read-only viewer role the UI does not derive from a prop; the ruling allowed skipping it.
- **Split layout**: the notes' pointer-drag divider and persisted height (`lib/note-chat`) rather than a new panel
  library — the same component, keyed by the charter's DocId.
- **Charter validation in the wizard** still runs on `draft.body`, which is the document's markdown as of the last
  Next/Hire click.
- Disclosed one-liners outside the owned paths: `crates/document/src/store.rs` (rename arm), `crates/document/src/tools.rs`
  (resolve spelling), `crates/rpc/src/api.rs` (two RPCs + one arm), `crates/roster/Cargo.toml`, `Cargo.lock`,
  `components/notes/NoteChat.tsx` (`about` prop), `rpc/api.ts`.

## Not built

- A read-only document + chat for theuth's `prompt.md`.
- Garbage collection of wizard charter documents for names never hired.

## Tests

- Rust: `document::meta` — `bound_ids_are_deterministic_and_notes_are_not` (charter ids deterministic per name, distinct
  from knowledge), `binding_key_and_derived_paths` (key, kind, derived). `roster` —
  `charter_export_replaces_the_body_and_keeps_the_front_matter` (sync rule: body replaced, front matter kept, read model
  and disk agree; unchanged / empty / unknown / theuth ignored).
- TS (vitest): `components/agents/charter.test.ts` (binding determinism, title, the pinned ref round-trips the marker
  grammar with its context), `wizard.test.ts` (hire copies the document body; empty document fails the charter step).

## Verification

See the report's "Verification" section (`/tmp/69/report-CHARTER-DOC.md`) for the exact command results.

DONE CHARTER-DOC
