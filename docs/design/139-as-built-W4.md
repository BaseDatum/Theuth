# 139 W4 — RPC/UI (D11, D18), as built

Branch `feat/139-w4`, cut from C's `1afe827c`. Owns `crates/rpc/src/api.rs`, `crates/desktop/ui/src/**`, `crates/tui/src/**`; nothing else touched.

## What changed where

### RPC (`crates/rpc/src/api.rs`)
- `resume_turn` (`:6028-6057`): `forward::owner_peer` → `forward::route(ext, session, "resume_turn", (session,))` → local `OpenTurn` + `ResumeTurn`. Revision 1 Q15: when the owner was foreign and `route` returned `None` (the `Write` take landed), the message is `ResumeTurn{cause: Some(Takeover), from: Some(<old owner>)}`; on the owner (local or `Forwarded`) `cause: None` = boot rule. Idempotent against W2's resume-on-take (`Resume` answers `Ok(false)` "in flight").
- `answer_ask` (`:3674`) / `resolve_approval` (`:3646`): `route_question(ext, &id.0, method, params)` first (`:2226-2255`): plugin gate + `Forwarded` short-circuit, `question_get(id)` → `store.owner(row.session)` (`question_owner`, `:2200-2223`) → foreign → `forward::to_peer` (forward, never take; `Route::Take` is not consulted); not connected → `-32010` via `owner_unreachable` (`:2258-2276`, mesh `last_seen` as `forward::unreachable` does). No row / own session / lone engine → local as before.
- `-32011` mapping (`session_draining`, `:2171-2197`): `send_input` and `steer` map a handler error whose message starts `SessionDraining:<reason>[:<next_owner>]` to `SESSION_DRAINING` with `data = {"SessionDraining": {session, reason, next_owner}}` and the §6.1 message (`"<s> is moving to <taker> — retry in a moment"` / `"<s> is pausing for <reason> — resumes on the next start"`). **Contract for W2/closer**: `human_input`'s `String` error for the D16 refusal must use that `Display` prefix (the `DocError::NotHub` convention, `doc_send`); nothing in `crates/agent` emits it yet.
- Tests: `durability_wire_shapes` (`:6761`) extended — `ResumeState.owner/here/cause` on the wire and the pre-139 default (`here: true`), `TurnResumed.cause/from` + old-engine default (`crash`, `None`), the `-32011` payload/messages/pass-through. New `routed_methods_dispatch_on_the_owner_with_the_forwarders_params` (`:6886`): the real `Server` dispatched as `forward::serve` does (internal peer + `Forwarded`) with the exact positional params the forwarder sends for the three methods — reaches the actor lookup, never `INVALID_PARAMS`, never a second hop. New `answers_follow_the_questions_row_to_its_owner_or_fail_unreachable` (`:6931`): two-engine store (`them` writes the question row, `me` reads it) → `question_owner` names `them`; own/unknown → local; no transport → `-32010` `{"OwnerUnreachable": {"owner": "them", "last_seen": null}}`.

### Desktop (`crates/desktop/ui/src`)
- `store/sessions.ts:156-176`: `ResumeBanner` + `owner?`, `here` (required), `cause?`, `from?`.
- `store/reduce.ts`: `resumeFromLoaded` (`:477-500`) copies `owner/here/cause` (`here ?? true` for pre-139 rows; same turn → the row's `owner/here` win); `apply()` `TurnResumed` (`:677`) copies `cause/from`; `TurnSuspended{reason:"release"}` → `cause: "transfer"`; new `ResumePending` arm (takeover, not resumed, `from`); new `SessionOwnerChanged` arm patches `here/owner` on a live banner. `whose()` (`:505`): relayed owner events (the `events.rs` follower re-publishes them on a replica) inherit the banner's `here/owner` instead of asserting "here".
- `components/chat/ResumeBanner.tsx`: `resumeText(r, label)` = the §9.3 table, first match wins; `offersResume()` + a Resume button (`useAttentionActions().resume` → `resume_turn`) for `here ∧ takeover ∧ !resumed`; owner ids through `engineLabel` with the mesh roster.
- `components/chat/OwnerOfflineCard.tsx`: body gains `ownerMidTurnText` and the button reads *Continue here and resume* when `resume && !resume.here`; click unchanged (`session_take(.., 'user')` + resend).
- `lib/owner.ts`: `sessionDraining()` (keys on `data.SessionDraining`, never on the code alone), `ownerMidTurnText()`.
- `components/chat/Composer.tsx:777-787`: a `SessionDraining` failure → `toast.message("moving to <B> — retrying…")` and one automatic `submit` retry after 2 s (`retried` ref, reset on success); no card.
- Tests: `ResumeBanner.test.ts` (every §9.3 line + boot causes + `offersResume`), `reduce.test.ts` (`139 §9.3` case: Loaded/TurnResumed/relayed TurnResumed/TurnSuspended{release}/SessionOwnerChanged/ResumePending; six 109 literals gained `here: true`), `lib/owner.test.ts` (`sessionDraining` vs `NotHub`, mid-turn sentence).

### TUI (`crates/tui/src`)
- `model.rs:6412-6446`: `TurnResumed` divider `resumed after transfer|takeover from <chip>` + note `taken from <chip> at hh:mm — resumed`; boot causes keep 109's strings. `ResumePending` (`:6399-6407`): divider `taken from <chip>`, note `… — not resumed`. `owner_turn` field (`:3506`) filled from `Loaded.resume` with `here = false` (`:3767-3771`); `UiEvent::Error` (`:5485-5490`) appends `owner_turn_suffix` (`:11453-11458`, *" — a turn was running on <chip>; /take resumes it here"*) to an owner-offline line.
- Tests: `resume_divider_and_note_name_the_previous_owner`, `owner_offline_line_names_the_running_turn` (`model.rs:16380-16501`); `ui.rs` `resume_divider_takes_the_compacted_shape` loops over the four labels.

## Deviations from §11 / the brief, and why
1. `-32011` is already `NOT_HUB` (`api.rs:2360`, 69 Y); C's `SESSION_DRAINING = -32011` collides. Both sides disambiguate by `data` (`SessionDraining` vs `NotHub`); the desktop keys on the payload only. Closer: pick a free code (e.g. `-32012`) in `forward.rs` + `owner.test.ts`.
2. The "Forwarded marker" test runs the real `Server` dispatch with the marker in extensions rather than a loopback `serve` pair: the transport leg is C's and already covered by `forward.rs` tests; `rpc` exports no loopback `Transport`.
3. `owner_unreachable` duplicates `forward::unreachable`'s mesh lookup (private in C's file). Closer: `pub(crate)` it and delete `api.rs:2258-2276`.
4. §9.3 has no row for `here ∧ cause transfer ∧ !resumed` (A's clients during the release drain; B under `resume: ask`): rendered *"Paused for a transfer at hh:mm — resumes on the new owner"* instead of 109's "Paused for an update". `!here ∧ resumed` renders *Running on A* (a relayed `TurnResumed`).
5. `ResumeState` has no `from`; after a reload the owner-side banner says *Taken from another engine* (live `TurnResumed.from` keeps the name).
6. Q10's client-peer flag is **not** implemented: `TakeSession{id, reason}` has no flag and `forward::route` (C) sends `Write`; a pre-take with `User` in `send_input` would also skip 68's `unseen_long_enough` gate, which is not Q10's intent. Closer/W2: flag on `TakeSession` set by `route` from `ext`.
7. `BUILD_LABEL=139` and `139 W4:` commit prefixes (the brief's verbatim block says 138; the prompt says 139).
8. Composer's "inline" retry line is a 2 s `toast.message`, not a new composer element.

## Gates
- `bun run check`: clean. `bunx vitest run src/store src/components/chat src/lib/owner.test.ts`: 55 files, 578 tests passed.
- `cargo test -p rpc -p tui --lib`: rpc 53 passed (51 + 2 new), tui 146 passed, 0 failed. `cargo clippy -p rpc -p tui --tests -- -D warnings`: clean. `cargo fmt -p rpc -p tui` applied to W4's hunks only (main is not fmt-clean in `api.rs:1489/5406`, `tui/lib.rs`, `model.rs:24-40/3904/18232-18294` — reverted, as C did).
- Build: the warm `./target` clone had copied 10 G of `release` and 28 M of 59 G `debug` after 20 min (four worktrees cloning in parallel); built cold into `./target-w4` (`CARGO_TARGET_DIR`) so the still-running `cp` could not clash with cargo. 10 m 42 s cold for the two test binaries.

## Open items for the closer
- W2 must emit `SessionDraining:<reason>[:<next_owner>]` (deviation 1 contract) or the `-32011` path is dead.
- Deviations 1, 3, 6 above.
- Banner default `here: true` when no banner exists and a relayed owner event arrives (replica client that loaded the session before the owner's turn opened): a known ceiling; fix needs `SessionSummary.here` in the slice.
