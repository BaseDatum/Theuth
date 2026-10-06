# 104 — Attention inbox for everything — as built (UI, `feat/104-ui`)

Base `d0f0f4be`; everything under `crates/desktop/ui/src/` plus this note. Rust untouched; `routeTree.gen.ts` unchanged
(no new routes). Bindings stubbed locally per the brief's contract (gitignored, not committed): `AttentionKind.ts` adds
`"ask" | "show"`, `AttentionItem.ts` adds `ask: AskRequested | null` and `turn: number | null`.

## What was built

Grouped rail (A4, A5)
- `components/attention/model.ts:11 GROUPS`, `:26 groupItems(items)` — Decisions · Reviews · Questions · Approvals ·
  Shown to you · Needs a hand (`handoff`, `secret`) · Agents (`hire`, `budget`) · Blocked · Mesh; empty groups omitted;
  `at_ms` descending inside each.
- `components/attention/Page.tsx` — one `<section aria-label>` per group: a 44 px (`h-11`) sticky header button
  (chevron · label · count, `aria-expanded`) above the rows; collapsed keys in `localStorage['attention.groups']`
  (`:17 GROUPS_KEY`, `:19 readCollapsed`). The header count stays the total.
- `components/attention/Row.tsx` — kind chip · title (`line-clamp-2`) · `from <session title> · <agent> · <age>`,
  unknown parts dropped; `min-h-11`. The session title comes from `title(item.session, sessions, …)` of
  `components/chat/broadcast.ts` (imported, not edited); `sessions` is passed down from the page's one `list_sessions`
  query (`source.tsx:18 useSessionList`).

Source header, context line, action bar — `components/attention/source.tsx` (new; `shared.tsx` left to 101b)
- `:24 Source({ item })` — chip · **From ‹session› · ‹agent› · ‹age›** · **Open session** (`Link to="/c/$session"`) ·
  the task title as a `Link to="/tasks/$id"` when `item.task` (`useTask`, "Open task" until it loads).
- `:61 useContextLine(session, beforeMs)` — `api.load_session(session, null, 60)` (the chat's own transcript RPC), the
  last `assistant` row stored at or before `beforeMs` (`model.ts:48 lastAssistantBefore`; an undated row counts as
  before), cut to 600 chars on a word boundary with "…" (`model.ts:36 clampText`). `retry: false`, 5 min stale;
  `enabled` only with a session. `:78 ContextLine` — three clamped lines with "more"/"less"; renders nothing on
  `null` (RPC failure, no session, no assistant text).
- `:95 ActionBar` — the review screen's fixed bottom bar (`pb-[calc(0.75rem+var(--safe-bottom))]`); `:104 SnoozeButton`.

Screens (A6) — `components/attention/Screen.tsx` maps every kind to a component (`SCREEN`), keyed by `item.id` so drafts
reset per item; phone header titles: Decision · Review · Question · Shown to you · Approval, else the kind label.
- `AskScreen.tsx` — Source · ContextLine · "The agent has N questions" · `QuestionBlock` per question with the
  recommended option pre-picked (`draftsFor`, `allAnswered`, `toAnswer`, `QuestionBlock`, `Draft` now exported from
  `components/chat/AskCard.tsx:17–62`; `AskCard` itself uses the two helpers, same behaviour) · fixed bar: **Send**
  (`useAttentionActions().ask` → `api.answer_ask(id, answers)`, disabled until every question is answered or skipped),
  **Dismiss (ends the question)** (`answer_ask(id, null)`), Snooze. An item without `ask` (older engine) shows the title
  and a link into the session instead of question blocks.
- `ShowScreen.tsx` — Source · ContextLine · the artifact's title · `metaLine(a) · vN` (name · KIND · size, from
  `ArtifactCard.tsx`) · `ArtifactView` of `get_artifact(call_id)` capped at `60vh` (query key `queryKeys.artifact`,
  shared with the Library); an error shows as one line with a "try the Library" link · fixed bar: **Open in Library**
  (`/artifacts/$callId`), **Mark seen** (`attention_dismiss(item.id)`), Snooze. `useMarkShown(callId, 2000)` dismisses
  the item after 2 s on screen.
- `ApprovalScreen.tsx` — Source · ContextLine · the tool name as a mono heading · "wants to run · ‹risk›" · **Command**
  code block · **Arguments** (`describe()` lines of every non-`command` argument) · fixed bar: **Allow** / **Deny**
  (`size="default"`), then `shared.tsx`'s `Footer` (Open task · Snooze · Dismiss). The full `args`/`risk`/`tool` come
  from `useApprovals` (the engine's pending-prompts snapshot this client holds, every session); without that request the
  tool is parsed from the title, `detail` is the code block, and a one-line note says only the command reached the inbox.
- `PlainScreens.tsx` — `HandoffScreen` (reason from `useHandoffs`, "Raised on ‹vm (VM)›", the page URL), `SecretScreen`
  (the `SecretRequestCard` exactly as `AttentionCard` mounts it: replicated row, else local `SecretRequested`),
  `HireScreen` (agent title · name · reports to · budget · the charter's first paragraph from `useAgents`),
  `BudgetScreen` (spent / cap bar, the HUD's colours), `NodeScreen` (last seen from `useMesh`, addresses), `BlockedScreen`
  (the task's `blockers` as chips). Each: Source · heading · facts · `KindActions size="default"` in the `ActionBar`
  (Snooze / Dismiss are its last two buttons, as today).

Seen hook (A2) — `components/hud/queries.ts:50 useMarkShown(callId, delayMs = 0)`: observes the inbox cache with
`useQuery({ queryKey: ATTENTION_KEY, enabled: false })` (no fetch of its own), and when it holds `show:<callId>` calls
`attention_dismiss` once (ref guard) after `delayMs`, then invalidates `ATTENTION_KEY`. Off under `?sim=`. Mounted in
`components/chat/ArtifactCard.tsx:406` (once the card has its `artifact`), `routes/artifacts.$callId.tsx` (a
`LibraryArtifact` wrapper around `Viewer`), and `ShowScreen`. `routes/attention.$id.tsx` keeps the last matching item in a
ref so a screen whose item left the list (a show marked seen by being looked at) stays up until the user navigates.

HUD (A8) — `components/hud/Hud.tsx:232` every `AttentionCard` title is `Link to="/attention/$id"` (was decision/review
only; the hand-off VM title button is gone — **Open desktop** in `KindActions` still opens the pop-out). `KindActions`
(`:249`) accepts `size: 'xs' | 'sm' | 'default'` and gains `ask` → **Answer** and `show` → **Open** as links to
`/attention/$id`. `KIND_LABEL` adds `ask: 'question'`, `show: 'shown'`; `ATTENTION_EMPTY` names questions and shows.

Events and actions (A10) — `components/hud/queries.ts:24 ATTENTION_EVENTS` adds `ArtifactShown`, `AskRequested`,
`AskResolved`; `:105 useAttentionActions().ask` (`answer_ask`; `false` → toast "That question was already answered").

Resolution (101 D11) — every Send / Dismiss / Allow / Deny / Mark seen / Snooze and every `KindActions` resolving button
goes through `useResolved(item)` from `shared.tsx` (desktop → `nextAfter` or `/attention`; phone → `/attention`).

Sim — `components/settings/sandbox-queries.ts:189 ATTENTION_SIM`: new `ask:ask-sim-01` (two questions: three options
with `recommended: 0`, one free text), `approval:apr-sim-01` (`detail` = the command), `show:toolu_015AZQL17aTdoAMD49A3uZkD`;
every row carries `session` (the ask/approval/show on a desk session, decision/review on an ops session), `ask`, `turn`.
`:236 ARTIFACT_SIM` — the `Artifact` record the Show screen serves under `?sim=` (see deviations).

Phones (A9) — headers `h-11`, rows `min-h-11`; the Ask screen's Send/Dismiss in the fixed bar; `QuestionBlock`'s inputs
already carry `text-base md:text-sm`; the back chevron is the 101 screen header.

## Tests

- `components/attention/model.test.ts` — `groupItems` (order, omission, newest first, hand-off + secret in one group,
  empty → `[]`), `clampText` (verbatim when short, word-boundary cut ≤ 601 incl. "…", hard cut when no space),
  `lastAssistantBefore` (newest at-or-before, undated counts, user/blank rows ignored, empty → `null`).
- `hud/model.test.ts` fixture gained `ask: null, turn: null` (type only).

## Gates (run from `crates/desktop/ui`, 2026-10-06 00:4x–00:5x)

```
export PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$HOME/.bun/bin:/opt/homebrew/bin:$PATH
just ts-desktop                      # exit 0 (second run; the first was killed at 900 s mid `cargo` — the log is /tmp/mc-2026-10-05/ui104-ts.log); 477 files, then the two stubs hand-edited
bun install && (cd ../../../packages/apps && bun install)   # 619 + 484 packages
bun run check 2>&1 | tail -3         # tsc -b --noEmit, exit 0
CI=1 bun run test 2>&1 | tail -8     # Test Files 141 passed (141); Tests 800 passed (800); exit 0  (/tmp/mc-2026-10-05/ui104-test.log)
bunx vite build 2>&1 | tail -2       # ✓ built in 1m 44s, exit 0; routeTree.gen.ts unchanged      (/tmp/mc-2026-10-05/ui104-build.log)
```

## Screenshots

Dev server `bunx vite --port 5181` with `VITE_THEUTH_URL=ws://127.0.0.1:58311` / `VITE_THEUTH_TOKEN` from `~/.theuth`
(the local daemon, so `list_sessions`, `load_session` and `get_attachment` are real), `?sim=sandbox-ok&mock=1`, the
managed headless Chromium at 1280×900 (DPR 1.25 → 1600×1125) and 390×844 (DPR 2 → 780×1688):

- `/tmp/mc-2026-10-05/ui104-list.png` — `/attention`: the grouped rail (Decisions · Reviews · Questions · Approvals ·
  Shown to you · Needs a hand), the newest item (the ask) in the pane
- `/tmp/mc-2026-10-05/ui104-ask.png` — `/attention/ask%3Aask-sim-01`
- `/tmp/mc-2026-10-05/ui104-show.png` — `/attention/show%3Atoolu_015AZQL17aTdoAMD49A3uZkD` (the markdown renders)
- `/tmp/mc-2026-10-05/ui104-approval.png` — `/attention/approval%3Aapr-sim-01`
- `/tmp/mc-2026-10-05/ui104-list-phone.png`, `ui104-ask-phone.png`, `ui104-show-phone.png`, `ui104-approval-phone.png` —
  the same at 390×844

Also exercised in the browser (not saved): collapsing Reviews and Decisions → `localStorage['attention.groups'] =
["reviews","decisions"]`, rows 15 → 12, state kept across a reload, re-expanding → `[]`; the context line's "more" →
"less" (the real session's last assistant text showed under the header); typing into the ask's free-text question
enables **Send**; the hand-off and secret plain screens; HUD home rows — every title and the ask's **Answer** / show's
**Open** link to `/attention/$id`; `/attention` without `?sim=` lists the live inbox grouped (Decisions 8 · Reviews 22 ·
Blocked 4, 40 rows). No mutation was clicked against the live daemon.

## Deviations from the brief / design

- **Session title fallback = the session's first 8 chars**, not omitted, when `list_sessions` lacks the session (the
  roster's own `who` convention in `attention.rs:338`; the sim's `sess-lab-…` rows would otherwise show nothing). An item
  with no `session` shows no "from" part.
- **The action bar and Snooze button live in `source.tsx`** (`ActionBar`, `SnoozeButton`) since `shared.tsx` is 101b's;
  the approval screen reuses `shared.tsx`'s `Footer` for Open task · Snooze · Dismiss.
- **Approval arguments** come from the client's `useApprovals` store (the engine's `PendingPrompts` snapshot) rather
  than the item; the item only carries `detail` (the command). Without the request the screen shows `detail` and says so.
- **`useMarkShown` is shared** by the Show screen, `ArtifactCard` and the Library route (one hook in `hud/queries.ts`)
  instead of three inline effects; it is a no-op under `?sim=` so the sim never calls `attention_dismiss` on the daemon.
- **`routes/attention.$id.tsx` edited** ("only if needed"): without holding the last item, the Show screen would flip to
  "This item is no longer in the inbox" two seconds after opening.
- **`ARTIFACT_SIM`'s `path`** is a real blob of this machine's daemon (`~/.theuth/artifacts/96b213f0572a8e19.md`); on
  another machine the Show screen's body shows `ArtifactView`'s "could not load" line while the chrome still demonstrates.
- **The HUD hand-off title** no longer opens the VM pop-out directly (A8: every title → the screen); **Open desktop** is
  still the first button on the card and on the hand-off screen.
- **Context line is raw text** (markdown markers visible in the three clamped lines), not rendered markdown.
- **`KindActions` on the plain screens** is `size="default"` (new size option) rather than a bespoke large button row.

## Not verified

- Against a real `ask` / `show` item from the ENG branch (the bindings are hand-written stubs; `AttentionItem.ask` is
  assumed to be the `AskRequested` shape with `type`, `id`, `session`, `questions`).
- Send / Dismiss on an ask, Allow / Deny, Mark seen, the 2 s seen timer and the `ArtifactCard` / Library dismiss end to
  end (not clicked against the live daemon, which has no `show:`/`ask:` items yet); the paths are the existing
  `answer_ask` / `resolve_approval` / `attention_dismiss` calls and the D11 navigation is `[INFERENCE]` from
  `mutateAsync().then(onResolved)`.
- `AskResolved` / `ArtifactShown` arriving over the wire and invalidating the list (the `on(...)` subscription is the
  same as the other events).
- iOS safe-area padding on a device.
