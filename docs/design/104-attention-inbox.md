# 104 — Attention inbox for everything (ATTENTION-INBOX)

Status: decided 2026-10-06 00:17 (Sky). Workers: `feat/104-eng` (crates/roster, crates/rpc; reads crates/tools and the artifact events) and `feat/104-ui` (crates/desktop/ui). Runs beside 101b with disjoint files (see seams).

## 1. The ask (Sky, verbatim)

> I want you to implement that from any session any question or approval or anything that needs my attention actually
> gets put into the attention area and has high quality UI/UX for it. That include show as well. The agent showing me
> something qualifies as needing my attention. All of it should link to the session that generated it easily as well.
> Once again, reduce cognitive load.

## 2. Today

`attention_list` derives approvals, task decisions/blocked/review, hires, budgets, unreachable nodes, desktop hand-offs and secret requests (`crates/roster/src/attention.rs`). An `ask_user` question from a chat session is NOT an item: it is a one-shot `AskRequested` broadcast rendered as a card in that session, which a phone that connects later never sees (facts/wth-ask-user-relay-no-replay). A `show` is not an item either. `AttentionItem.session` is set only for approvals; the rows link to the task, the agent or the session depending on kind.

## 3. Decisions

- **A1 Two new kinds.** `AttentionKind::Ask` (`ask:<AskId>`) — every pending `ask_user` from any session (`tools::PendingAsks` on the ask actor, which already exists for reconnecting UIs). `AttentionKind::Show` (`show:<call_id>`) — every artifact an agent showed (`show`) since this build: the attention actor subscribes to the `ArtifactShown` runtime event the way it subscribes to `ApprovalRequested`, and records `attention_shows(call_id, session, title, kind, mime, bytes, version, at_ms)` (roster migration 0005; engine-local like `attention_state`). No backfill: only shows after the upgrade become items.
- **A2 Clearing.** An ask item leaves when answered or dismissed (dismiss = `answer_ask(id, null)`, which ends the asking turn, exactly like the chat card's Dismiss). A show item leaves when the user **sees** it: opening `/attention/show:<id>` or `/artifacts/<id>`, the chat `ArtifactCard` becoming visible in the open session, or Dismiss — all call `attention_dismiss("show:<call_id>")`. Snooze works on both.
- **A3 Session on every item.** `AttentionItem.session` is filled for every kind where a session exists: approvals (as today), asks, shows, hand-offs, secret requests, decision/review/blocked tasks (`task.checkout.session`, else the newest `task_events.session`), hires (the proposing session when the roster knows it). `AttentionItem.agent` is the desk name when the session is a desk or under one (`SessionSummary.desk` on the session or its root). `AttentionItem` gains `ask: Option<tools::AskRequested>` (the questions, so the screen renders at once) and `turn: Option<u32>` when the source knows the turn (approvals and shows carry their call's turn when available; else null).
- **A4 Inbox grouped by kind.** The rail groups rows under headers with counts, in this order: Decisions · Reviews · Questions · Approvals · Shown to you · Needs a hand (hand-offs, secrets) · Agents (hires, budgets) · Blocked · Mesh. Empty groups are omitted; each group is collapsible (state kept in `localStorage`). Within a group newest first. The badge stays the total.
- **A5 Every row: who · what · age.** Kind chip, title, then `from <session title> · <agent>` and the age. The session title comes from `list_sessions` (`title()` helper of the broadcast picker is the precedent: a subagent's peer name, else `chatName`).
- **A6 Every screen opens with the source.** A header line **From ‹session title› · ‹agent› · ‹age›** with **Open session** (→ `/c/$session`, phones navigate, desktop opens in the current pane) and, when the item has a task, the task title as a second link. Then the kind's body. Then the actions. The screens: **Ask** — the question blocks of `components/chat/AskCard.tsx` (export `QuestionBlock`) with Send / Dismiss; **Show** — the artifact rendered inline with `components/artifacts/Viewer.tsx ArtifactView`, its name · kind · size · version, Open in Library, and "Mark seen" (= Dismiss); **Approval** — tool name, the arguments pretty-printed (a `command` as a code block, other args as a key list), Allow / Deny; **Hand-off** — reason, Open desktop (when a VM), Done / Cancel; **Secret** — the existing `SecretRequestCard`; **Hire** — title, reports-to, budget, the charter's first paragraph, Hire / Decline; **Budget**, **Node**, **Blocked** — the row's facts and button, larger. Decision and Review keep 101/101b's screens (not touched here).
- **A7 Context line.** For Ask, Show and Approval screens: the agent's last assistant text before the item (≤ 600 chars, "…" cut) under the header, fetched from the session's history through the RPC the chat uses to open a session; shown collapsed to three lines with "more". Skipped silently when the history is not available (an older engine or a remote node).
- **A8 HUD home.** The home inbox section keeps its rows and inline buttons; every title links to `/attention/$id`.
- **A9 Phones.** Group headers and rows are 44 px; screens are the same components; the Ask screen's Send sits in the fixed footer.
- **A10 Events.** The UI invalidates the inbox on `AskRequested`, the ask-answered event, and `ArtifactShown` (add to `ATTENTION_EVENTS`); the engine needs no new RPC beyond what exists (`attention_list/dismiss/snooze`, `answer_ask`, `get_artifact`, `resolve_approval`).

## 4. Seams (beside 101b, which is in flight)

104-ENG owns `crates/roster/src/attention.rs`, the roster migration, `crates/roster/src/lib.rs` (actor wiring), `crates/rpc/src/api.rs` only if a method is unavoidable. It must not touch `crates/tasks/**`, `crates/document/**` (101b-ENG). 104-UI owns `components/attention/{Page,Row,Screen,model,shared-header}.tsx`, new screen files, `components/hud/Hud.tsx` links, `components/chat/ArtifactCard.tsx` (the seen hook), `components/chat/AskCard.tsx` (export only), `components/hud/queries.ts` (events). It must not touch `DecisionScreen.tsx`, `ReviewScreen.tsx`, `PacketMarkdown.tsx`, `Pointers.tsx`, `queries.ts` (attention), `shared.tsx` (101b-UI) — add a new `source.tsx` for the header instead.

## 5. Gates

ENG: `cargo test -p roster -p rpc --no-fail-fast`, `cargo check --workspace`, `just ts` (bindings: `AttentionKind` gains `ask` | `show`, `AttentionItem` gains `ask`, `turn`). UI: `just ts-desktop`, `bun run check`, `bunx vite build`, `CI=1 bun run test`, screenshots of the grouped rail, an Ask screen, a Show screen, an Approval screen at 1280 and 390 px (sim data).
