# 70 — as built: SEND-NEVER-LOSES (a send never destroys typed text)

Branch `fix/send-never-loses-draft`, 2026-09-26. Defect (user): "I was on mobile, I had composed a long message, I sent
it and I guess the connection was dropped, I lost my composed message and had to write it again."

Rule: text a user typed is never destroyed by a send. The composer clears only when the engine has acked the input;
until then the text stays in the draft store. On any failure (transport error, timeout, relay reconnect, engine restart
mid-send, iOS backgrounding mid-request) it is back in the composer with a visible "Not sent" banner, one-tap Retry and
Discard; a retry never double-sends.

## Root cause (traced)

`components/chat/Composer.tsx` `submit()` cleared the draft store *before* awaiting the RPC:

- `reset()` (`setText('')` + `setPastes(NONE)` on `lib/drafts.ts`, persisted to `localStorage` at once) ran at
  Composer.tsx:512, then `await api.steer(...)` :519 / `await api.send_input(...)` :523 / :553.
- The `catch` (:557-560) only toasted the error (or, for `OwnerUnreachable`, kept `{body, files}` in component state for
  the "Continue here" card — lost on unmount). Nothing put the text back.
- On a paired device the request goes through `RpcClient.call` (`rpc/client.ts:96`), whose pending call is rejected
  with `RpcError(-32300, 'connection closed')` the moment the transport reports `reconnecting` (`#failPending`, :218).
  `LinkTransport` emits `reconnecting` from `#lost` (`link/transport.ts:95`, relay socket closed) and on PRESENCE 0
  (:158, engine offline). So the failure *was* surfaced — as a toast, with the text already gone. The remaining
  transport gap: a socket the OS never closed (iOS suspend, a Wi-Fi drop while foregrounded) looks `open`;
  `LinkTransport.send` (:60-66) and `LocalTransport.send` (:71) drop silently and the call hung with no per-call
  timeout, so `busy` never cleared either (only the foreground `info` probe, :166-186, reset it).
- Engine side there was no idempotency key for `send_input` (`rpc/src/api.rs:162`: session, text, files, delivery);
  `steer` carried a client `InputId` but nothing deduped on it (`loop_.rs:1085` pushes unconditionally). A "retry"
  after an ambiguous `connection closed` (the request may have reached the engine) would have been a second turn.
- The RPC ack is enqueue-only: `Agents::route` (`supervisor.rs:879`) `tell`s the session actor and replies before
  `Turn::persist` (`loop_.rs:2559`) writes the row; `human_input` also swallowed routing failures (rejected attachment
  path, closed mailbox) and answered success while publishing `AgentEvent::Error`.
- TUI: `ui.rs:545` `std::mem::take` emptied the editor on Enter; a failed `Command::Send`/`Steer` only appended
  `Entry::Error` (`model.rs:4874`), text gone.

## What changed

### Desktop (`crates/desktop/ui`)

- `lib/drafts.ts`: `Draft` gains `unsent?: string` — the client input id of a send not yet acked (in flight or failed),
  persisted with the draft. `sending(key, id)` flags it; `acked(key, sentText)` removes exactly what was sent (text typed
  during the flight stays: prefix strip); a failure changes nothing. `set` drops the flag once the box is empty (nothing
  left to retry). POPOUT last-writer-wins sync carries the flag like any other field.
- `components/chat/Composer.tsx` `submit()`: `clientId = unsent ?? crypto.randomUUID()` — Enter, the Retry button and
  the "Continue here" card all reuse a failed send's id. The box keeps its text (`sending`) through the await; the
  optimistic row (`markSent`) / queued chip (`InputQueued`) still appear at once. Success → `acked` clears the box,
  then a new chat navigates. Failure → the optimistic row is removed (`useSessions.unmarkSent`, new) or the chip
  withdrawn (`InputWithdrawn`), the draft stays flagged, and the banner renders above the box: "Not sent — your message
  is still here." with **Discard** (clears draft + flag) and **Retry** (`submit()`). A new chat whose session was created
  but whose first send failed moves the flagged draft to that session's key and navigates there, so Retry lands in it.
  Every send is raced against `SEND_TIMEOUT_MS = 15 s` (`deadline`); a `SendTimeout` fails the send (banner) and calls
  `RpcClient.probe()` so a silently dead socket is reset and reconnects. A voice note with an empty box parks its
  `[voice note …]` line in the box first so a failure has something to restore.
- `rpc/client.ts`: the foreground `info` probe is now public `probe()` (same body: 5 s, any reply proves the socket,
  else `transport.reset()` → `reconnecting` → pending calls rejected). In-flight rejection on `reconnecting` was already
  there and is now pinned by a test.
- `rpc/api.ts`: `send_input(session, text, files, delivery?, id?)`.
- `store/sessions.ts`: `unmarkSent(id, text)` removes the last unstamped own row with that text and its echo skip.
- `components/chat/OwnerOfflineCard.tsx` (68 §5): no longer holds `{body, files}`; Continue = `session_take` then
  `onResend()` = the composer's retry, so a second failure also keeps the text.

### Engine (`crates/agent`, `crates/rpc`)

- `SendInput.id: Option<InputId>` (`#[serde(default)]`); RPC `send_input` gains a trailing `id: Option<InputId>`
  (positional, so older callers are unchanged); forwarded to the owner engine with the other params.
- `Agents::human_input` (the single funnel for `SendInput` and `Steer`) dedupes on the id: `seen: HashMap<InputId,
  (SessionId, Instant)>`, TTL `SEEN_TTL = 600 s`, swept on each identified input. A hit returns the remembered session
  and routes nothing (idempotent success). It runs on the owner engine after `forward::route`, so a retry through a
  different front door still dedupes. `// ponytail: process-local, lost on restart; persist beside the row if retries
  must survive engine restarts.`
- Routing failures now reject the RPC: `human_input` returns `Result<SessionId, String>`; `SendInput`'s reply is
  `Result<SessionId, String>`, `Steer`'s `Result<(), String>` (`rpc::api::ask` maps `Display` errors to JSON-RPC
  errors). A failed routing is not remembered, so the retry routes. Callers in meeting/roster/schedule/signal/slack add
  `id: None`; their error handling already covered `SendError`.
- Side effect, intended: a `send_input` that carries an id now gets `InputQueued`/`InputInjected` events like a steer
  when the engine is busy after all (the desktop showed those invisibly before, 70 COMPACT-QUEUE).

### TUI (`crates/tui`)

- New `UiEvent::Unsent { id: Option<InputId>, text }`, sent by `execute` when `Command::Send` or `Command::Steer`
  (including its interject cancel) fails, before the existing `UiEvent::Error`. The model removes the optimistic
  `Entry::User { turn: None }` (Send) or the `QueuedInput` (Steer), clears `sent`/`awaiting_session`, and sets
  `recall`, which the editor drains on the next render — above anything typed since, not over it (`ui.rs:203`).
- Not done for the TUI: a banner and an idempotency id on `send_input` (passes `None`). The TUI mostly talks to an
  in-process engine (`Engine::Local`), where a failure is not ambiguous; the `Entry::Error` line plus the restored text
  is its "not sent" state. Noted, not fixed: `/edit` and `/skill args` also clear before ack.

## New-chat options before the first message (2026-11-16)

The draft's model/effort and pending per-chat choices share `composer-drafts` in localStorage:
tools/MCP hides, messaging, approvals, memory mode, project, summary model/effort, view budget and root scope/alias.
The header's session menu, Tools and Messaging sheets, memory controls and slash commands edit these without
creating a chat. Private retention and project/memory switches below the composer persist with the draft too.
Global tool gates remain global.

`openDraftSession` sends creation fields through `create_session`, then awaits approvals, messaging, tool hides,
summary model before its effort, view budget and root scope/alias before allowing a send, Talk or skill.
Settings clear only after all setters succeed. On failure, the error names the setting; text, held attachments,
settings and the created chat remain. A retry reapplies settings to that same chat rather than creating another.
The retained opening and target survive reload. Session setters route by owner, including chats created elsewhere.
The Model picker's Advisor choice is carried by `CreateSessionOptions.advisor`: the existing `set_advisor`
does not forward, so creation installs it on the owning machine before returning. Advisors require durable native
chats; choosing Private clears that incompatible pick. Retention and Advisor choices lock while a failed apply
retains the created chat, since they are creation-only choices on a remote draft.

## Tests

- `lib/drafts.test.ts`: send keeps the text until acked, ack drops exactly the sent text (typed-during-flight stays),
  flag persisted for a reload; failure leaves the flagged draft through edits, emptying the box drops the flag.
- `rpc/client.test.ts` (mocked `Transport`): a send in flight when the socket drops is rejected at once with
  `-32300 connection closed`, a retry while down fails fast; `probe()` on a silent socket resets it after 5 s and the
  parked call fails.
- `store/reduce.test.ts`: `unmarkSent` drops the optimistic row and its echo skip; a history row with the same text
  stays; the retry's `TurnStarted` is echoed.
- `agent`: `retried_input_id_routes_once` — same id twice routes once and answers the same session; a fresh id routes.
  Existing `closed id` test now asserts the refusal reaches the caller.
- `tui`: `unsent_text_leaves_the_transcript_and_returns_to_the_editor` (Send and Steer variants).

## Verification

See `/tmp/69/report-SEND-NEVER-LOSES.md` for the run log: `just ts-desktop`, `bunx tsc --noEmit` 0, `bunx vite build`
0, `CI=1 bunx vitest run` 85 files / 513 tests, `cargo check --workspace --tests`, `cargo test -p agent|rpc|tui`,
no-emoji lints (vitest + `cargo test -p tui --test no_emoji`), and the browser smoke against a scratch engine killed
mid-session (screenshots `/tmp/69/sendlose-*.png`).

DONE SEND-NEVER-LOSES
