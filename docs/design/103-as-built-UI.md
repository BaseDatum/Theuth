# 103 — PACKETS-V3 — as built (UI, `feat/103-ui`)

Base `8aeb9065` (main with 101b landed). Everything under `crates/desktop/ui/src/components/{attention,chat,settings}`;
Rust, `Page/Row/Screen/model`, `hud/**`, `ArtifactCard`, `Message/Transcript` untouched; no new routes (`routeTree.gen.ts`
unchanged after `bunx vite build`). Bindings are local only: `just ts-desktop` (477 files, exit 0, 642 s), then
`src/rpc/bindings/{Recommendation,Item}.ts` written as §3 spells them and `Packet.ts` given `recommendation: Recommendation |
null, items: Array<Item>` by hand (gitignored, not committed; integration regenerates from the ENG branch).

## What was built

Commits: `9625f637` (F2/F3 + PacketBoard polish), `860198c9` (F4), `daf39d45` (sim), then this note.

- **0 PacketBoard polish** — `components/attention/PacketBoard.css:25-32`: `.packet-board h1.board-prose { display: none }`
  (the board's `#` title block — `BoardReader.tsx:283` renders headings as `<h1 class="board-prose">` — no longer repeats the
  screen's question) and `.packet-board .board-chrome[data-anchors] .text-destructive { color: inherit }` (the live chip's
  `verify failed: …` takes the chrome row's muted colour; `BoardReader.tsx:178` is unchanged). Nothing in `board/**` changed.
- **1 Recommendation (F2)** — `DecisionScreen.tsx:44-46,67-73`: callout above the board/summary, `border-info-border
  bg-info-bg`: "**‹by› recommends:** ‹option label› — ‹text›", `by = packet.by` without `agent:`, the label only when
  `recommendation.option` resolves to an option. Pre-pick: `choice = picked ?? recommendation.option ?? recommended`
  (`:40`); the `recommended` badge on the cards stays `packet.recommended`.
- **2 Bundle (F3)** — `components/attention/Bundle.tsx` (new). `Bundle({ items, picks, onPick, anchored })`: `<ol>` with
  `1.`…`N.`, title, description (muted), option chips (`aria-pressed` toggle group in the Explorer chip style, `recommended`
  badge on `item.recommended`), "Type your own…" `Input` per item. State lives in `DecisionScreen` as `Pick[]`
  (`number | string | null`, `:35,39`): untouched = `items.map(i => i.recommended)`; a chip sets the index, typing sets the
  string (so the chip pick is gone), clearing the input sets `null`. Pure helpers at the end of `Bundle.tsx` (`:82-120`):
  `pickLabel`, `allAnswered`, `allRecommended(items, picks)` (every item has a recommendation and `picks[i] ===
  recommended`), `bundleAnswer(items, picks, top)` (`1. <title>: <pick or text>` per line, the top-level pick/text as the
  last line when non-blank, `\n`-joined), `head(text)` (first non-blank line, ≤ 140 chars), `slug(text)`.
  Screen (`DecisionScreen.tsx:106-112,151-160`): section title "‹N› decisions" → `Bundle` → "Overall" (the top-level option
  cards, title "Options" without a bundle) → "Type your own…" → **Approve all as recommended** (`disabled` unless
  `allRecommended && !typed`) · **Answer** (`disabled` unless `allAnswered`, i.e. `answer !== ''`). Both submit the same
  form; `send()` → `act.decide.mutateAsync({ decision, answer })` → `task_decide`, unchanged.
- **3 Anchor into the board (F3)** — `Bundle.tsx:23,73-77 jumpTo`: the title is a button when `anchored` (`!!packet.board`);
  it collects `.packet-board h3`, takes the one whose `slug(textContent) === item.id`, else the one whose slug equals
  `slug(item.title)`, and `scrollIntoView({ block: 'start' })` (smooth unless `prefers-reduced-motion`). The reader gives
  headings `id={block.id}` (`BoardReader.tsx:283`), not a slug, so the match is on text. A collapsed `##` section has no
  mounted `###` → nothing happens.
- **4 Chat relay card (F4)** — `components/chat/AskCard.tsx:146-149 decisionKey`, `:156`, `:180`, `:216-251 RelayCard`.
  The key is the **question's** id (`ask.questions[0].id` when there is exactly one and it starts with `decision:`), see
  Deviations. `usePacket(key)` (query disabled when `null`); with a packet and not expanded/sent the card is the compact
  relay inside the same `FRAME`: "‹by› asks for a decision" · `packet.question` · `head(summary)` (muted) · the backed option
  (`recommendation.option ?? recommended`) with the `recommended` badge and `recommendation.text` · **Open in Attention**
  (`Link to="/attention/$id"`, id = `decision:${packet.task}:${packet.key}`, the shape `roster/src/attention.rs:215`
  builds; `search={true}` like `Row.tsx` so `?sim=` survives in dev) · **Answer here** → `expanded` → today's full card
  (questions, options, Send answers / Dismiss) byte-for-byte. Without a packet (older engine, plain `ask_user`) the card is
  unchanged (`hooks` order unchanged; the early return sits after every hook).
- **5 Sim** — `components/settings/sandbox-queries.ts:184` new inbox row `decision:<task2>:decision:dns-9c21` ("Portal DNS
  cut-over", by `manager`), `:201-202 dns(...)` item builder, `PACKET_SIM['decision:dns-9c21']` (`:252-283`): `by:
  'agent:manager'`, a two-paragraph summary naming the forwarded `decision:dns-4b10`, two top-level options with
  `recommended: 0`, `recommendation: { text, option: 0 }`, ten items (apex A, www, api, TTLs, MX, SPF, DKIM, CAA, old LB,
  status) with 2–3 options each, every one with a `recommended` (two differ from option 0), `board: SIM_BOARD_DECISION`.
  The two 101 packets gained `recommendation: null, items: []`.
- **6 Tests** — `components/attention/Bundle.test.ts`: `bundleAnswer` (labels, typed text, top-level line, blank/invalid
  picks), `allAnswered`, `allRecommended` (changed, typed-over, item without recommendation, empty), `head` (blank lines,
  140 cut with `…`, exact 140), `slug`.

## Smoke (managed headless Chromium, `bunx vite --port 5183` against the local daemon ws://127.0.0.1:58311, `?sim=sandbox-ok&mock=1`)

- `/attention/decision:<task2>:decision:dns-9c21` 1280×900: callout rendered; `.packet-board h1.board-prose` present with
  `display: none`; the `verify failed` span's computed colour equals the chrome row's (`rgb(154,139,114)`). Ten items with
  the recommended chips `aria-pressed=true`; initial buttons: Approve all **enabled**, Answer **enabled**. Click "Switch
  Monday" on item 1 → Approve all disabled; click the recommended chip → enabled again. Type on item 5 → both MX chips
  `false`, Approve all disabled; Backspace to empty → Answer disabled too; click "Leave as is" → both enabled.
- Answer string: `WebSocket.prototype.send` patched in the page to capture (and not forward) `task_decide`; with item 3 →
  "Switch tonight", item 9 typed "after 14 days", top-level on the recommendation, **Answer** sent
  `["decision:dns-9c21", "1. portal.example.com A record: Switch tonight\n2. www CNAME: Switch tonight\n3. api CNAME: Switch
  tonight\n4. TTLs at 60 s: Already done — proceed\n5. MX records: Leave as is\n6. SPF TXT: Add tonight\n7. DKIM selector:
  Rotate next week\n8. CAA record: Add tonight\n9. Old LB decommission: after 14 days\n10. status CNAME: Switch tonight\nApply
  the bundle tonight"]`. No mutation reached the daemon.
- Anchor: the daemon's board has no `###` (`h3` count 0), so one `<h3 class="board-prose">API CNAME</h3>` was injected into
  the first section body; clicking the item title "api CNAME" moved it from `top: −6037 px` to `56 px` (just under the
  pane header) — the slug match and scroll work; against this board's real headings the click is a no-op.
- `/attention/…apr-7f3a` (the 101 decision, no bundle, no recommendation): one **Answer**, "Options" title, option 1
  pre-picked, no callout, no `<ol>` — unchanged.
- Relay card: `/c/sess-ops-desk`, two `AskRequested` pushed into `useAsks` (`import('/src/store/approvals.ts')`): one with
  question id `decision:dns-9c21` → the compact card (who asks, question, head, backed option + badge, recommendation text,
  Open in Attention, Answer here); one plain `ask_user` → today's card. **Answer here** → the full card with "Apply the
  bundle tonight" pre-pressed, Send answers / Dismiss. **Open in Attention** href
  `/attention/decision%3A0192a000-…0002%3Adecision%3Adns-9c21?sim=sandbox-ok&mock=1` → the decision screen (`h1` = the
  question).
- 390×844 (touch emulation): header, callout, board chrome row with Verify + Walk me through, bundle chips wrapping to two
  rows, inputs full width.

## Gates (run from `crates/desktop/ui`, 2026-10-06 01:2x–01:4x, on the committed tree)

```
just ts-desktop                       # exit 0, 477 files (642 s; first attempt hit a 300 s wrapper timeout, re-run under a log); stubs added by hand after
bun install && (cd ../../../packages/apps && bun install)   # 619 + 484 packages
bun run check 2>&1 | tail -3          # tsc -b --noEmit, exit 0
CI=1 bun run test 2>&1 | tail -8      # Test Files 142 passed (142); Tests 802 passed (802); exit 0
bunx vite build 2>&1 | tail -2        # exit 0; routeTree.gen.ts unchanged; tree clean
```

(101b's `mobile-zoom.test.ts` timeout did not recur; the machine was idle.)

## Screenshots (DPR 1.25 → 1600×1125 px; phone DPR 2 → 780×1688 px)

- `/tmp/mc-2026-10-05/ui103-decision.png` — the forwarded bundle decision at 1280×900: header, question, recommendation
  callout, board (no title heading, muted verify chip).
- `/tmp/mc-2026-10-05/ui103-bundle.png` — the same, scrolled to "10 decisions" (items 1–7).
- `/tmp/mc-2026-10-05/ui103-bundle-footer.png` — items 6–10, "Overall" option cards, Type your own…, **Approve all as
  recommended** · **Answer**.
- `/tmp/mc-2026-10-05/ui103-decision-phone.png`, `ui103-bundle-phone.png` — 390×844.
- `/tmp/mc-2026-10-05/ui103-relay-card.png` — `/c/sess-ops-desk` with the relay card above an unchanged plain ask card.

## Deviations from the brief / design

- **Relay key = the question id, not `ask.id`.** `AskRequested.id` is the ask actor's uuid (`tools/src/ask.rs:166`); the
  decision key is the single question's id (`tasks/src/lib.rs:767`). `decisionKey` reads `questions[0].id` when there is
  exactly one question starting with `decision:`. If ENG later also stamps `ask.id`, nothing here needs to change.
- **One submit row, under the top-level cards.** The brief places the Approve all / Answer footer "above the existing option
  cards"; that leaves either two Answer buttons or the top-level pick below the button that sends it. The bundle, the
  "Overall" cards and the free text share the one form, and **Approve all as recommended** · **Answer** close it (D8's single
  Answer stays single). Approve all is disabled while top-level text is typed (it would not be "as recommended").
- **Approve all sends the same string Answer would** (the picks *are* the recommendations when it is enabled); it exists as
  the one-tap affordance, not a different payload.
- **"Answer here"**, not a toggle: the compact card expands once; the full card has no collapse (nothing to go back to once
  a draft is started).
- **Anchor match on heading text**, not an id — the reader has no slug ids. Sim item ids are the slugs of the titles the DNS
  board would carry (`portal-apex-a-record`, …); the daemon's sim board is the 101b RCA board (no `###`), so in the sim the
  click is a no-op (mechanism verified by injection, above). Hover lighting skipped (design: "cheap: scroll-into-view").
- **Helpers live in `Bundle.tsx`** (brief: `Bundle.tsx` or `bundle.ts`): a `bundle.ts` beside `Bundle.tsx` collides on the
  case-insensitive macOS filesystem (`tsc` TS1149). `head` (F4) is there too rather than in `queries.ts`.
- **No chat sim ask**: nothing seeds `useAsks` under `?sim=`; the relay card was exercised by pushing into the store from
  the dev console (above). Adding a chat fixture would touch `Transcript`/store code outside the card.
- **Section title "‹N› decisions" / "Overall"** for the bundle and the top-level cards (brief names neither).

## Not verified

- Against real `Packet.recommendation` / `Packet.items` from the ENG branch (stubbed bindings; shapes follow §3 verbatim).
- `task_decide` end to end with a bundle answer (captured on the wire, not sent); the mutation is 101's `act.decide`.
- An expanded relay card's Send answers against a live decision ask (the full card is 101's code, unchanged).
- The anchor against a real decision board with `###` option headings (none on this daemon).
- Tauri `openUrl`, iOS safe area (dev ran in a browser).
