# 103 — Decision packets v3 (PACKETS-V3): forwarding, bundles, chat relay card

Status: decided 2026-10-06 00:17 ("Yep, write it"); builds on 101 + Revision 2 (mini-boards). Source: the brief "make agent decisions land in Attention as packets" (another session, 2026-10-06), re-shaped for mini-boards. Workers: `feat/103-eng` (crates/tasks, crates/rpc) and `feat/103-ui` (crates/desktop/ui). Starts when 101b has landed (same files).

## 1. Why

On 2026-10-05 22:30–00:00 four Sky-only decisions were relayed by the theuth manager desk on behalf of SRE desks (autopay PR approval, autopay PR-CI side effects, a ten-item portal DNS bundle, DR plugin publish approvals). Each reached Sky as one `question` string: the manager re-typed the desk's structured report into prose, and the ten DNS items were one paragraph. The thread held the real packet.

## 2. Decisions

- **F1 Forwarding, not re-typing.** When an agent escalates a task that is already blocked on a `decision:<id>` raised by *another* actor (the manager hop), the new packet **inherits** the original packet's `question`, `summary`, `board`, `options`, `recommended`, `pointers` and `items` unless the caller passes its own; the caller adds `recommendation`. The thread block says `forwarded from decision:<old> by <manager>` and records the recommendation. The old decision stays as it is (the manager answers it with `decide` once the user has answered, as today).
- **F2 `Packet.recommendation: Option<Recommendation>`** — `{ text: String, option: Option<u32> }`: the forwarding manager's view in one or two sentences and the option it backs. Shown above the board on the Decision screen as a callout ("<manager> recommends: <label> — <text>"); when `option` is set the option card is pre-picked over `recommended`.
- **F3 Bundles.** `Packet.items: Vec<Item>` — `Item { id: String, title: String, description: Option<String>, options: Vec<PacketOption>, recommended: Option<u32> }` (≤ 20 items, ≤ 6 options each, caps as §3 of 101). A bundle is N related sub-decisions in one escalation (the DNS case). Decision screen: a numbered list; each item shows its title, description, option chips with the recommended one pre-picked; the footer offers **Approve all as recommended** (enabled when every item has a recommendation) and **Answer** (when any item was changed or has no recommendation, listing the per-item picks). The answer string is structured markdown: `1. <title>: <pick>` per line (plus the free text when typed) so the thread and the caller read it without parsing. Items anchor to the board: an item whose `id` matches a `###` heading slug under the board's Options section lights that section when hovered (cheap: scroll-into-view on click; no new anchor grammar).
- **F4 Chat relay card.** The `AskRequested` card a decision raises on the calling session (D4) renders as a compact card when the ask's id is a `decision:<id>` with a packet: title, the one-line abstract (`head(summary)`), the recommended option, **Open in Attention** (`/attention/<item id>`); the full question/options stay one tap away (expand). Channel (Slack/Signal/Telegram) cards are unchanged in this wave.
- **F5 Teach the agents.** Tool descriptions and the desk manual add: ≤ 120-word abstract answering "what is being decided and why now"; ≤ 4 options unless it is a bundle, one line each, exactly one recommended; forward a report's decision with `recommendation`, never by re-typing it into `question`; use `items` for N related decisions; the refusal text already names what to add.

## 3. Contract additions (ts-rs shapes)

```ts
export type Recommendation = { text: string, option: number | null, };
export type Item = { id: string, title: string, description: string | null, options: Array<PacketOption>, recommended: number | null, };
// Packet gains: recommendation: Recommendation | null, items: Array<Item>
```
`EscalateArgs` gains `recommendation: Option<Recommendation>`, `items: Vec<Item>`; inheritance happens in `ledger.escalate` when the task's blockers hold a `decision:<id>` whose packet `by != actor`.

**Revision 1 (106, 2026-10-06 — fix wave after the live proof, `docs/design/101-live-proof-2026-10-06.md` F-2/F-3/F-4).** `Packet` gains `to: Option<String>` (`"user"` | `"agent:<name>"`, serde default, ts `to: string | null`; rows written before 106 read `None`): `ledger.escalate` sets it to the resolved manager, `review_packet` to the effective reviewer. The inbox rule: among a blocked task's `decision:` blockers the Attention surface lists the **newest** one whose packet is addressed to the user (`to == "user"`, or no `to` / no packet for legacy rows) and none when every decision is addressed to an agent — a forward chain shows the forwarded packet with its recommendation, an agent→agent escalation shows nothing to the user. The forwarder wake: `answer_decision` wakes the packet's `by` agent (`answer`) when it is not the task owner, so a parked forwarder learns the user's answer and `decide`s the parent; the owner wake on an empty blocker list stays. The tell→ask rule: a message into `Tasks` whose reply is a `Result` is sent with `ask` and its `Err` logged, never `tell` (kameo stops an actor on an `Err` reply to a tell); `Tasks::on_panic` logs and continues as a backstop, and `answer_decision` dismisses the ask card raised for the decision so it cannot be answered twice.

## 4. Gates

ENG: `cargo test -p tasks -p roster -p rpc --no-fail-fast`, `cargo check --workspace`, `just ts`. UI: `just ts-desktop`, `bun run check`, `bunx vite build`, `CI=1 bun run test`. Merged tree: all, plus screenshots of a bundle and a forwarded decision (1280 and 390 px, sim data).
