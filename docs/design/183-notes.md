# 183 — build notes

## 183 follow-ups, review round 1 (CF-R1)

On fd4de033, 17 items. Evidence `/tmp/rs183f/evidence.txt` (section "CF-R1"), shots `shots/r1-*.png`.

1. **Rebinding a hold checks the export's definition, not only its name.** `Staged::unchanged_exports` (a2a `config.rs`): exports granted in both epochs with the same `exports`/`agents` definition and the same contact cap. `reload_as` asks `RebindHeld` for every contact whose cap or grant changed, right after `CommitPeerConfig`; holds of exports not in that list fail at once with `HOLD_CHANGED` ("the shared chat or agent changed while this waited; send it again"), so a re-pointed chat, changed agent tools or a changed cap never runs an old first message under the new authority. Live: hi-there re-pointed to another chat with a message waiting → B's task failed with that text, nothing ran in the new chat, C's receipt failed with it.
2. **The held line is bounded.** `held_line` clips machine and export to 64 characters, drops control characters, at most 512 in all; the export comes from the alias as written, and for a plain `peer:<contact>` the holder's export is named only when it is a valid export name.
3. **The card route is admitted like any contact request** (`Live::admit_contact`: rate and concurrency limits before the card or the notice). Offer refreshes coalesce per contact (one in flight, notices collapse into one follow-up), errors are clipped to 200 characters, and `A2aChanged` fires only when the offers changed. The card's alg/kid in a verify error are clipped.
4. **A held message on the sender at the sender's restart fails honestly.** Re-arming the follower is not possible (the store keeps no export per exchange for the follow route), so boot accounting fails outbound held/consumed contact rows with `HELD_AT_RESTART` ("the contact's owner may still allow and deliver it, and its answer would not reach this session; check with them before sending it again").
5. **The sender keeps following a held task.** While the remote task is `AUTH_REQUIRED` the follow deadline slides by `FOLLOW_MAX`, capped at `HOLD_TTL + FOLLOW_MAX`, so an Allow late in the 24 h still reports back.
6. **Non-await sends ask for contacts only.** `peers.rs` asks `IsContact` per `peer:` receipt and fetches first answers for contacts only, concurrently.
7. **Directory order and caps.** Three groups: own tree, then contacts with at most 8 export rows each (the contact's row then reads `+N more: discover_peers(kind="contact", query="<contact>")`), then the rest. New filter `kind: contact` (`DirectoryKind`) lists every contact row uncapped.
8. **Tests that fail on the base.** The supervisor F11 test now sends an allowed message into the same chat while one is held (on v2.19.0 it queued behind the held one) and then Allow once; the network test's Allow-once half is unconditional. The hold timer is tested on a paused clock (`held_wait`). F2 is a real ungrant and regrant: **decision** — an explicit "Ask first" is now stored as `ask` (it was the absence of an entry), so a regrant keeps the owner's last explicit choice, and an export never answered starts on Always allow. Also: per-reader card (a third contact sees no skills), the offers TTL, and an upgrade from a pre-0055 store with an `auth_required` task (opens with `held = 0`, boot fails it).
9. **Touch targets.** The three answers are `pointer-coarse:min-h-11` with `gap-2` (strip, Communication, Contacts rows: all `FirstButtons`).
10. **One answer per row.** The buttons stay disabled after success until the list drops the row; a second click would only have set the standing default.
11. **Unknown Attention kinds are listed.** A runtime catch-all group "Other" (last) and a plain screen for a kind a newer engine sends.
12. **Remote strings wrap.** `break-words` on the waiting line and on "Shared with you".
13. **Receipt lines stay one line.** `\r`, `\n`, U+2028 and U+2029 become spaces, in the address too.
14. **The contact's `tasks/cancel` on a held task cancels it at once** (as a never-started task: no `CANCEL_WAIT`); its retire drops the hold, so a later Allow runs nothing.
15. **Every dropped hold refreshes the surfaces.** `NetworkState::retire` publishes `ContactsHeldChanged` (via `HeldGone`) when it removes a hold.
16. **Offer titles** are cut to the card's 64 characters, without control characters, names deduplicated.
17. **Counts and `held`.** The strip and rows say "· N messages" when more than one waits; the boot UPDATE clears `held`.

## 183 follow-ups (F1–F11), after Sky's live test of 2026-11-10

Branch `fix/183-contacts-followup` from v2.19.0 (e2f3608e, Sky's version bump on 4a2b2fd3). F6–F11 came from the orchestrator after the first brief (`.contactfix-addenda.md` and the queued messages); F11 as revised (a strip, the held task waits aside). Evidence `/tmp/rs183f/evidence.txt`, shots `/tmp/rs183f/shots/`, rig `/tmp/rs183f/rig/`.

### F4 root cause: the Attention rail had no group for `contact_first`

- **Cause.** `crates/desktop/ui/src/components/attention/model.ts:11-25` (at e2f3608e): `GROUPS` lists every kind but `contact_first`, and `groupItems` keeps only kinds in the table, so the rail never lists the item. `routes/attention.index.tsx:7-10` shows `items[0]` in the pane, which is the only way the item was ever seen (Z3's P3-3 shot had it as the only item). On 333, with any other item ranked first, it was on no screen. Every build since Phase 3 landed (70a84bc1, so v2.19.0) has it.
- **Proven, on a rig like Sky's** (A hub by lease, B the non-hub sharer, C the contact; B inviter, C invitee, names `sky`/`sky-moore`): the engine derives `contact_first:sky-moore:hi-there` on B in every case tried — B's own chat, a chat owned by the other member (W-40), the hub as sharer, after a restart of B as a mesh member. `attention_list` has it; the UI header says "Attention 1"; the rail `<aside>` is empty. Not the lease, not W-40, not `lookup_when`, not a derive or alert step.
- **Sky's own run agrees** (the laptop's store, read only): the first message (02:14:02) stayed `accepted` until 02:20:32 `replied`, right after the Settings switch to "Always allow" — that path releases only a hold `ContactsHeld` lists, so 333 held it (and derived the item) the whole time, and the UI Sky used was attached to 333.
- **Fix.** The rail's kind→group mapping is a `Record<AttentionKind, …>` (tsc fails when a kind lacks a group); `contact_first` is the "First contact" group after Approvals. vitest: a `contact_first` item among others is listed.
- **Lead 1 (the app on the other member):** Attention is per engine by design; there is no mesh merge of Attention items to hook into, and Sky's UI was on the holding engine (above). Not built; see "Not covered".
- **Lead 2, found on the base rig:** any `contact_update` that changes a contact's grant (sharing a second export, a cap change) rotates its grant epoch and silently failed every held first message ("peer route revoked"), taking its item. Fixed: before the old epoch is cancelled, the reload asks the agent `RebindHeld{contact, exports, route}`: held tasks for exports still granted move to the new epoch (their watch too); the others fail with the old one. Always (Attention, strip, Contacts row, the Settings select with a message waiting) releases that export's held messages like Allow once and clears every surface.
- **Live updates.** The agent publishes `ContactsHeldChanged` whenever a hold begins or ends (held, released, failed, revoked, expired); the a2a actor nudges Attention and publishes `A2aChanged`. Before, only the non-streaming send path nudged, so a hold that began behind a busy chat, or ended by revoke, changed nothing on screen until another event.

### Calls made

1. **F1 card.** A contact `GET /.well-known/agent-card.json` over `theuth/a2a/1` gets this engine's signed card with, as skills, the exports granted to it (`id` = export name, `description` = its description, tags `text`, `chat`|`agent`, `allowed`|`ask_first`|`unavailable`; a paused chat reads `unavailable`). Served in `reach::serve_http` before the router, for gate-admitted contacts only; the public card (Nearby, ticket, ceremony) lists none. The receiver verifies the JWS against the pin; an old engine answers that path with its first export's unsigned card, which fails verification and keeps the last list.
2. **F1 notice.** Every reload compares, per contact, what it may reach (`offers::granted`: grants, descriptions, kinds, first-contact answers) and posts `POST /theuth/offers-changed` to each contact whose list changed (so `contact_update`, `a2a_export_put`, `a2a_export_remove`, a first-contact change and a hand edit are all covered by one hook); archiving or unarchiving a shared chat posts it too. Best effort, logged at debug.
3. **F1 receiver.** Memory, per endpoint id (`Ctx.offers`), pushed to the agent by alias (`ContactOffers`) after each refresh and each reload (renames, removals, blocks). Refreshed at pairing (retried at 1, 3 and 10 s: the other side may not have pinned yet), at boot, on the notice, and when `discover_peers` (or `list_peers`) runs and the last ask is older than 60 s (`ContactOffersStale`, published at most once a minute; that call answers from the cache). A failed refresh keeps the last list and logs once per distinct error. Not persisted: the boot refetch covers a restart.
4. **F1 rows.** `peer:<contact>/<export>` rows, name `<contact>/<export>`, title = description (else the name), tags `["contact", "chat"|"agent", "allowed"|"ask_first"|"unavailable"]` (the state is the third tag; `PeerState` is unchanged), send and reply true. Only roots that see the contact row see them, so a contact's inbox still sees no contact (Z3-R2).
5. **F2.** `contact_update` writes `first_contact[x] = allow` for each newly granted export with no stored answer; a stored answer (an explicit Ask first is the absent entry, a Block) is kept. `a2a_export_put` carries no contact, so the picker's put-then-grant gets the default from the grant. No migration.
6. **F3.** RPC `contact_first_waiting() -> FirstWaiting[]` from `Ctx::first_waiting`, the same function the Attention source uses; the UI refetches it under `['a2a']` on `A2aChanged`.
7. **F5.** A held task's watch fails it with "the owner did not answer" after `HOLD_TTL` (24 h). With F11 it never blocked the chat, so there is no queue to restart.
8. **F6.** `discover_peers` lists A2A contact rows first (kind `a2a` and first tag `contact`, so a title-model tag cannot jump a session ahead); `active_only` keeps them; new `kind` filter (`native`|`acp`|`a2a`|`node`|`parent`) on `ListPeers` and the tool.
9. **F7.** `PeerMessageState::Held` (`"held"`): the sender recognises the holder's `waiting: <machine> has not allowed messages to <export> yet` status on a contact route (`ContactRefusal::waiting`, beside the text it parses) and stores `held` with `error` = "held: <contact> (<machine>) has not allowed messages to <export> yet; it will be delivered when they press Allow — do not resend". No peer message. An awaiting `send_message` returns at once with it; a non-awaiting one waits up to 5 s for the remote's first answer (ponytail: a 100 ms store poll). Held → `consumed` (error cleared) when the remote task starts, → `replied` on its answer, → `failed` on Block, revoke or expiry.
10. **F8.** The receipt's `to` is `peer:` + the address as written (`peer:sky/hi-there`); a reply's and a question's `from` is rewritten the same way before the session sees it; `peer_message`/`list_peer_messages` show `peer:<alias>` for live bindings (the store keeps no export per exchange, so there it is the contact's name only). The store still keys exchanges by binding id.
    - **send_message text grammar** (for the transcript's send card, 03:47 addendum): one physical line per receipt, `<address> <state> id=<uuid>[ task=<…>][ error=<…>]` (line breaks in remote `task=`/`error=` text become spaces), every receipt line first, then a `reply id=<uuid>:` header and the reply text per replied receipt; `structured.receipts` unchanged. Test: `receipts_output_is_one_row_per_recipient` (a three-way awaited fan-out, a forged line inside a reply, multi-line error and task id).
11. **F10.** The sharer's export row reads "<contact> reaches this as …/<export>": the engine does not know the name the contact gave us. The receiver's contact row reads "Shared with you: …" from `Contact.shared`.
    - **peers.yaml's `exports` direction** (decided, no rename): every UI rewrite adds one comment line after the owner's leading block (`store::EXPORTS_NOTE`): "peers.<contact>.exports: what THIS machine grants that contact …, not what the contact shares with us." It survives later rewrites (E3 rule) and is added once. Test: `rewrite_keeps_the_header_and_the_entries`. Not built (decided): a `contacts()` runtime tool.
12. **F11.** `a2a_tasks.held` (migration `0055_a2a_held.sql`, local table): a held task is stored `auth_required` (what the contact reads) with `held = 1`; `active_a2a_task` and the store's one-active-task rule skip it; any later transition clears it. Every first message to an Ask-first export is held at once (`hold_on_start` is gone). The strip (`ContactAllowStrip`) sits in the composer's `strip` slot beside `KernelStrip`.
13. **Same-chat traffic while one waits** is the owner's turns and peer or session messages; another export or contact cannot reach the same chat (one export per session, one contact per chat export), so "another contact's allowed message" is proven on an agent export and, at the engine level, with the same contact's allowed task in the same session.

### Tests

- a2a: `a_granted_export_is_a_discover_peers_row_on_the_contacts_side_until_revoked` (F1 rows appear after the grant and go after the revoke's notice, title and tags, F6 order with `active_only`, `Contact.shared`, F2 default and a kept Ask first, public card empty); `a_grant_change_keeps_a_waiting_first_message_and_always_releases_it` (F4 lead 2 + Always releases).
- agent: `first_contact_holds_until_the_owner_answers` (reworked for F11: held at once behind a busy session, an allowed task starts, Allow once queues, Block fails); `a_held_first_message_expires_unanswered` (F5); `a_held_send_reads_held_and_names_the_contact` (F7, F8); `a_grant_change_keeps_held_first_messages_of_exports_still_granted` (F4 lead 2 at the agent); `a_waiting_first_message_leaves_the_chat_working` (F11: owner turn and a session's peer message run, the hold still waits).
- vitest: attention `groupItems` lists `contact_first` (F4); `ContactAllowStrip.test.ts` (three answers, several waiting, hidden for other chats); `Contacts.test.ts` (F2 default and Ask first display, F3 Contacts row and buttons, F10 both texts).
- Removed: the source-text assertion in `Composer.test.ts` (171F B7) that pinned the route's exact `strip={<KernelStrip … />}` line; the strip slot now holds the contact strip too.

### Gates and live proof

- check `--tests` and clippy `--tests --no-deps -- -D warnings` on `-p a2a -p agent -p tools -p rpc -p mesh -p roster -p secrets -p theuth -p web -p document -p cells -p link` 0/0, and on `-p slack -p telegram -p signal` (one-line `ListPeers.kind`) 0/0.
- Rust suites twice on the final code with a fresh `t` as both workers: 1911 passed, 2 failed each run — `repl::tests::capping_binds_out_name` both (failing before Z3), `sensor::tests::monty_budgets_and_allowlist` (load) once, web's `connection_reset_then_200_is_retried` once (web untouched; 3/3 alone). a2a 47, agent 830, tools 222, rpc 88, mesh 70, roster 100, theuth 12.
- Bindings regenerated (`FirstWaiting`, `ContactOffer`, `OfferKind`, `OfferState`, `PeerMessageState.held`); tsc 0; vitest 323 files / 2183 tests, 0 failed.
- Live (scratch A hub, B non-hub sharer, C contact; loopback, fake model): F4 before (rail empty, `f4-before-b-rail-empty.png`) and after (`f4-after-b-rail-first-contact.png`); F2 grant → `allow`; F1 rows appear, carry `allowed`/`ask_first`, go on revoke, come back after a restart; F7/F8 the model's result `peer:sky/hi-there held … error=held: sky (b) …`; F3 both surfaces (`f3-chat-communication-waiting.png`, `f3-contacts-row-waiting.png`); F11 strip (`f11-strip-b-shared-chat.png`), an owner turn and a session message ran while a message waited, Allow once on the strip delivered it and cleared Attention; F4 lead 2 a grant change kept the waiting message and Always on the Contacts row released it; F6 `discover_peers {kind: a2a, active_only: true}` lists the contact and its two rows. F5 is proven by its test only (24 h).

### Not covered / follow-ups

- **The item on another member's screen** (lead 1): a contact's hold shows only on the engine that holds the contact. A mesh-wide view would forward `contact_first_waiting` and the answer to the holder over `theuth/rpc/1`.
- A message held when the **sender** restarts stays `held` on the sender (its follower is memory; the owner side fails it at its own restart). Pre-existing for every followed outbound task.
- A held message keeps the cap and allowlist of its admission; a contact **renamed** while one waits loses it (holds are keyed by alias).
- `peer_message` shows `peer:<contact>`, not `/<export>`.
- The full `peer:<their name for us>/<export>` on the sharer's row needs the contact's alias for us, which is not exchanged.
- mDNS on en0 not captured (no root for tcpdump); the rig used the dev directories and the ticket only (see evidence).
- Migration `0055` may clash with another branch's number at merge.

## Phase 3 closer 2, round R1 (Z4-R1)

- **MUST (UI copy): the readers see everything of their kind.** `knowledge_search`, `knowledge_get` (including `id@commit` history), `knowledge_list`, `doc_read`, `nb_search` and `artifact_read` are not scoped to the export. Add tool now shows under each "Sees ALL your <knowledge | notes and documents | notebooks | artifacts>, not just what you share here", and under an `mcp:` tool "Read-only as the server declares it; for a server you don't control, pin its risk in mcp/config.yaml". The line above the export rows (`toolsLine`) names what granted readers let contacts read ("… let them read ALL your knowledge, notebooks and notes and documents, not just what you share.") and drops knowledge from the hidden list when a knowledge reader is granted. vitest: the `183 Z3-R2 tools` test (both notes) and `183 Z4-R1 tools line`.
- **SHOULD: web tools are bounded per call for contacts**, in the registry's `Invoke` when the session is a contact session (`contact_scoped`, where the contact limits live): `crawl` limit ≤ 20 and `max_depth` ≤ 2, `map_site` limit ≤ 200, `search_web` limit ≤ 8 and `read` ≤ 5 (its defaults). Set where absent, since the owner's `scrape.*` defaults may be larger; a non-integer value takes the bound. Test `a_contact_crawl_is_bounded_per_call` (the owner's own call passes as sent).
- Gates: clippy on the P3 set 0; tsc 0; vitest 322 files / 2170 tests, 0 failed (an earlier run under the clippy load failed `link/pairing.test.ts` once, 7/7 alone and in the clean rerun); Rust suites with a fresh `t`: tools 222, the rest green but agent's `capping_binds_out_name` (failing before Z3) and `monty_budgets_and_allowlist` (load), and web's `connect_timeout_fires` (a 1 ns loopback connect race, web untouched: 2 of 3 direct reruns pass).

## Phase 3 closer 2 (Z4): the grantable allowlist

Security re-check of e815e520 (`/tmp/rs183/p3/final/review-security-r4.md`, CHANGES): the spawn refusal holds, but the deny-list (`escalates`) let an owner grant tools that reach past the contact's capped session and that the read_only cap does not stop (`knowledge_put`, `remind_me_in`, `routine_draft`, `brief_put`, `computer_*`, `browser_*`, …).

- **The rule is now an engine-owned allowlist.** `tools::CONTACT_GRANTABLE` (in `tools`, where the registry lives; `a2a` and the RPC use it): `search_web`, `read_url`, `scrape_url`, `crawl`, `map_site`, `knowledge_search`, `knowledge_get`, `knowledge_list`, `doc_read`, `nb_search`, `artifact_read`. An MCP tool is grantable as `mcp:<server>/<tool>` (the server's own tool name) only while its effective risk (`spec().risk`: gates, else annotations) is `read_only`; its bare name never admits it, so an MCP server cannot register a grantable native name to slip in. Nothing else, ever: `escalates` and the UI copy of it are gone. The loop's own tools (`python`, `todo`, …) are not grantable either.
- **Enforced in three places.** At load (`config::contact_tools` in `stage`: strip, one warn per tool per load); at `a2a_export_put` (refused, `tools: a contact may not be granted knowledge_put, computer_screenshot`; the export is left as it was); and in the registry for every `SetAllowOnly` — the one path both `set_turn_limits` and `install_contact_limits` take, so a paired engine's `origin.tools` is intersected there: non-grantable names are dropped at install (warned) and `allowlist_admits` admits a tool under any allowlist only by its `grant_key` (fail closed; an MCP tool that turns mutating later drops out at once).
- Choice: the put checks the key's shape only for `mcp:` names (the a2a actor has no registry at hand in its tests and an MCP server may be offline); a mutating or unknown MCP key is accepted into the file and never admitted. The editor offers only what the registry reports.
- **The editor's choices come from the engine.** RPC `a2a_grantable_tools() -> [GrantableTool {name, risk, mcp_server?}]` (registry `ContactGrantable`: registered grantable natives and read-only MCP tools by key, sorted); `ExportTools` lists only those (query key `grantable`, replacing `list_tools`).
- **Tests.** a2a `a_put_without_tools_keeps_the_exports_allowlist` (a put with `knowledge_put`, `computer_screenshot`, `remind_me_in`, `shell` is refused naming the four; an MCP key and a reader are accepted), `ungrantable_tools_are_stripped_when_the_file_loads`; tools `an_allowlist_from_a_peer_holds_only_grantable_tools` (`browser_open` intersected away at install, inbox and turn; MCP by key only, read-only only; `ContactGrantable`), `contact_sessions_see_only_their_allowlist` (reworked: a listed `send_message` or mutating tool stays hidden); agent `a_contact_turn_in_a_chat_spawns_nothing_at_the_registry` (the live registry: a turn-scoped `SetAllowOnly` on a chat, then `SpawnChild{parent: chat}` is refused; cleared, it spawns); vitest `183 Z3-R2 tools` (Add tool = `a2a_grantable_tools` less the list).
- **Merge.** `origin/main` (ba6be48c, 747abd99: UI only) merged with no conflicts; bindings regenerated.
- Gates: check and clippy (`--tests --no-deps -- -D warnings`) on the P3 set plus `web`, `document`, `cells`, `link` 0/0; those suites twice with a fresh `t` (agent `--lib` included): a2a 45, tools 221, agent 825/2 failed then 826/1 (`capping_binds_out_name` both, failing before Z3; `monty_budgets_and_allowlist` run 1, load), the rest green; bindings regenerated (`GrantableTool`, `a2a_grantable_tools`), tsc 0, vitest 322 files / 2169 tests, 0 failed.
- Live (`evidence.txt`, "Z4 live"): P3-1, P3-7, P3-4 and P3-9 PASS.

## Phase 3 closer round R4 (Z3-R4)

- **A fresh grant is admitted at once.** Root cause: not the grant, the reload or the address debounce — the grant, the put, the cap and the allowlist already reload synchronously inside `Ctx::edit` (the 2 s hot reload and the 1-minute address write only serve hand edits and address hints). The 404 came from `Live::open`, which asks the communication policy whether the exported chat is enabled, and A's policy did not know B's freshly made chat: a replicated row is mirrored into the policy only when something touches it, which `a2a_exports` (`ChatPaused` → `mirror`) did — hence "works a minute later" after the next `a2a_exports`. Fix: `ValidateExport` (run for every chat export on each reload, so on every put and grant) mirrors the session (`Agents::validate_export`). Test `an_exported_chat_from_another_engine_is_open_as_soon_as_it_is_exported` (a replicated row unknown to the policy is enabled after the validation). Live (`evidence.txt`, "Z3-R4"): C's send 90 ms after the grant returned was admitted and answered.
- **main's `tasks/home.test.ts` "offline home" is no longer date-bound**: the test pins the clock (`vi.useFakeTimers({ toFake: ['Date'] })`, `vi.setSystemTime` to the same day as its "last seen") and restores it on finish; the component is unchanged.
- **MUST (security re-check): a contact session spawns nothing.** With the turn's allowlist on the turn's own session only (R3 d), a child spawned in a contact turn was no contact session: it kept every tool up to the inherited cap and could spawn `agent:` uncapped — reachable once an owner put `spawn_subagent` in an export's tools. Both fixes: `SpawnChild` refuses every spawn, native or external, when the spawner is in a contact session (`under_inbox`, or `tools::ContactScoped` for a contact turn), `subagents are not available to a contact`; and an allowlist never holds a tool that escalates (`a2a::config::escalates`: `spawn_subagent`, `send_message`, `discover_peers`, `wait_peer`, `schedule`, `task_*`, `agent_*`, `secret_*`), stripped at staging and at `a2a_export_put` with a warning per tool, never offered by the editor (`Contacts.tsx` `escalates`; its line says so). `child_cap` is back to (requested, ceiling, external). Tests `a_contact_session_spawns_nothing` (an inbox whose allowlist names `spawn_subagent`, and a session under it, are refused; an owner root spawns), the put leg of `a_put_without_tools_keeps_the_exports_allowlist` (`send_message`, `spawn_subagent`, `task_create`, `secret_list`, `schedule` stripped), the registry's `ContactScoped` on a contact turn (`contact_sessions_see…`, `bg` leg), vitest "Z3-R2 tools" (the escalating tools are not offered).
- **Engine nit: a retry leaves no context.** `admit_send` mints a context before `accept_a2a_request` dedups, so each retry of one messageId left an orphan row. When the acceptance answers from an earlier task, the minted context is dropped (`Store::drop_unused_a2a_context`, only if no task names it). Test `retries_of_one_message_leave_one_context` (5 sends of one messageId: 1 task, 1 new context).
- Gates: check and clippy on the R3b set 0/0; with a fresh `t`: a2a 44, tools 220, agent 824/2 failed (`capping_binds_out_name`, failing before Z3; `monty_budgets_and_allowlist`, load); tsc 0; vitest 322 files / 2163 tests, 0 failed (no UI change after that run).

## Phase 3 closer round R3b (Z3-R3b): merge with design 184

- **Merge.** `origin/main` at 2add9997 (design 184) merged at 71f9d0ba. One conflict, `crates/mesh/src/net.rs` `Net`'s fields and their initialisers: both kept (P3's `contacts`, `a2a_conns`, `a2a_stream_budgets`; 184's `docs`, `dev_drop`). `net::Msg` gained only 184's `DocsSchema` (P3 adds no `Msg` variant), so the tag order is main's. No migration number clash (agent 0054 is P3's only one). Bindings regenerated, tsc 0.
- Gates: check and clippy on the P3 set plus `web`, `document`, `cells`, `link` 0/0; those suites once with a fresh `t` (agent `--lib` included): all green but agent's `capping_binds_out_name` (failing before Z3) and `monty_budgets_and_allowlist` (load). vitest 322 files / 2163 tests: 2162 passed, 1 failed — main's `tasks/home.test.ts` "offline home …" (design 182) hardcodes 2026-11-08 21:40 as "last seen" and expects `since 21:40`; on 2026-11-09 the line reads `since Nov 8, 21:40`. The test is date-bound, outside P3 (not run on main itself), left as is.
- Live (`evidence.txt`, "Z3-R3b live"): P3-1 (Nearby pairing, capped inbox), P3-7 (every probe "not available"), P3-4 (B's chat exported on A: contact turn with no tools and no owner details, only its answer back, owner rows unseen, stop sharing refuses) PASS. Seen again: a send within seconds of a fresh grant is refused "grants no export" until the next reload reaches the gate.

## Phase 3 closer round R3 (Z3-R3)

- **Merge.** `feat/183-p3-ui2` (U3b, 0b75827b) merged with no conflicts; the allowlist editor now uses the real `Export` binding (`tools?: string[]`), its local type and the `// 183 Z3-R2: tools` marker are gone.
- **A put keeps the allowlist** (R2 residual 3). `contacts::Export.tools` is `Option<Vec<String>>`: `a2a_exports` always fills it; `a2a_export_put` without it keeps the export's list (a new export: `search_web`, `read_url`), so a re-grant or an older client never resets it. The UI sends `tools` only from the editor. Test `a_put_without_tools_keeps_the_exports_allowlist`. Stop sharing with the last contact still removes the export (U3's rule), so sharing it again later starts on the default.
- **A resumed contact turn still answers** (R2 residual 1). A resumed exported-chat contact turn takes its opener's exchange id as its scope (`consumed_exchanges` at the opener's seq); at its end the owner rebuilds the envelope from that exchange (`contact:<holder>:<contact>`) when a restart lost it, and the answer goes back as before (the holder only needs the message id it waits on). Test: the restart leg of `an_exported_chat_elsewhere_runs_one_capped_turn_and_answers_only_it` (envelope forgotten, the turn's text still completes bob's task). A holder restart fails the contact's task at boot as before (`fail_interrupted`), a clear outcome.
- **A resumed inbox speaks as the exported agent** (R2 residual 2). `a2a_contexts.inbox_identity` (added to the unreleased migration 0054) keeps the agent's persona of the last admission; `identity_of` falls back to it when nothing is cached. Found live: the roster's identity source joins after the boot resume, so asking the source again at the actor start was not enough. Test `a_restarted_inbox_runs_the_exported_agents_persona_again` (no source after the restart).

Security re-check of 2d49e40a (CHANGES), in the same round:

- **MUST: a resumed contact turn is known from its journal.** `open_contact_turn` (the last user row) is gone: the loop writes user rows inside a turn (harness notes, budget notes, the notebook message, surface and cell correction rounds, the compaction head), so a provoked correction made a restart resume uncapped. The opener's consumption now journals `input_accepted` with source `contact` (and the exchange id), and `Resume` reads it (`Store::contact_turn_input`); a turn whose journal has no `input_accepted`, or a failed read, counts as a contact's (fail closed). The scope is the journaled exchange id, so the answer still goes back (item 3). Test `a_contact_turn_is_known_from_its_journal_whatever_rows_follow` (a note, a budget note, a correction and a compaction head after the opener: still a contact turn, bounded; an owner turn is not; no `input_accepted` is). Not tested end to end through a live `Resume` actor: the install on resume is the same `set_turn_limits` as at turn start.
- **a)** `set_turn_limits` and `install_contact_limits` return their errors: a failed `SetAllowOnly`/`SetTurnCap`/`SetCap` ask or registry lookup stops the contact turn (its token is cancelled before it runs; the contact reads the turn was stopped), refuses the inbox admission (`internal error`), or refuses the inbox's actor start; a failed clear at the turn's end leaves the limits on. A process with no registry registered (unit tests) runs no tool at all, so that is not an error.
- **b)** Migration 0054 backfills `inbox = 1` for contexts whose session carries the E3 `<contact_inbox …/>` first row.
- **c)** A contact session (inbox, or a chat while a contact's turn runs in it: `tools::ContactScoped`) gets `frame::contact_baseline`: the exported agent's persona and charter, the contact rule and the date; no machine, working directory, standing instructions (`prompt.md`), skills index or bodies, peer line or plugin blocks. The chat's own history stays (the owner shared it). Test `a_contact_prompt_carries_nothing_of_the_owners_machine`.
- **d)** A contact turn's allowlist (and `ContactScoped`) apply to the turn's own session only; the chat's existing subagents keep the owner's tools (an inbox's allowlist still covers its subagents). Registry test, `bg` leg.
- **e)** `web::fetch::ssrf::is_blocked_ipv6` blocks 6to4 (2002::/16) and Teredo (2001::/32). Test in `fetch_ssrf.rs`.
- **f)** After a fail-closed resume the contact's task ends: an exported chat owned elsewhere answers through the journaled exchange (item 3); an inbox's or a holder-owned chat's A2A task is failed `interrupted_by_restart` at boot (`fail_interrupted`); an opener the journal does not name leaves the holder's task to its request deadline.

Engine re-check of 2d49e40a (M1, M2 closed), in the same round:

- **MUST (engine M3): no context bound.** `CONTACT_CONTEXTS` and `a2a_context_count` are gone: they counted every context ever made for (contact, export), and our own sender never reuses one (`network_send` passes no `context_id`), so the 65th message was refused for good. The waiting bound (`CONTACT_QUEUED` = 8) limits a contact's pending work. Test `a_contact_never_runs_out_of_contexts` (70 sequential messages, each a fresh context, all admitted).
- **MUST (regression): an external harness is refused for a contact only.** `child_cap(requested, ceiling, external, contact)`: in a contact session (the supervisor's own record of the spawner or an ancestor being an inbox, `Agents::under_inbox`, or `tools::ContactScoped`: a contact's turn in the spawner's own session, which also hides `spawn_subagent`) an `agent:` spawn is refused; under an owner desk's or a capped subagent's ceiling it spawns as before with the cap it asked for (the ceiling rides in its brief). Native children get min(requested, ceiling) everywhere. Test `a_child_takes_the_spawners_ceiling_and_external_harnesses_are_refused_for_contacts` (a contact session refused; an owner desk at `read_only` allowed). Found by the suite: refusing whenever the registry could not answer refused the ACP tests' owner spawns, so an unanswered registry no longer counts as a contact (the inbox check does not depend on it).
- **The owner's running children keep their tools during a contact turn.** Neither the turn's allowlist nor its cap (`SetTurnCap`, now applied by `cap_for` to the turn's own session only) reaches the chat's existing subagents: they list and run the owner's tools. Registry test, `bg` leg (allowlist, `EffectiveCap` none, full `ListTools`).

- Gates (R3, final): check and clippy (`--tests --no-deps -- -D warnings`) on the set plus `web` 0/0; `mbx test -p a2a -p agent -p tools -p mesh -p roster -p rpc -p theuth -p web --no-fail-fast` twice with a fresh `t`: a2a 44, agent 821/2 failed then 822/1 (`capping_binds_out_name` both, failing before Z3; `monty_budgets_and_allowlist` run 1, load), tools 220, mesh 62, roster 100, rpc 83, theuth 12, web green; bindings regenerated, tsc 0, vitest 317 files / 2110 tests (no TS or UI change after that run).
- Live (`evidence.txt`, "Z3-R3 live" and "Z3-R3 security items"): the allowlist editor round-trips (add `crawl`, remove `read_url`, an RPC put without tools, a reload and a UI re-grant all keep `[search_web, crawl]`; shots `r3-c-allowlist-*.webp`); P3-7 on the merged build and again on the final one: the inbox is offered only its allowlist, every other probe (shell, read_file on a decoy, search, session_read, spawn_subagent, python, todo, send_message) is "not available in this session"; the inbox's system prompt carries no machine or working directory; a kill -9 mid inbox turn resumes as Research with no tools and the contact reads `interrupted_by_restart`.

## Phase 3 closer round R2 (Z3-R2): the cap and security round

Reviews: `/tmp/rs183/p3/final/review-engine.md`, `review-security.md` (both CHANGES at 284b4bb1).

### The trust rule: contact sessions are default-deny

A **contact session** is (a) a contact's inbox (an `a2a_contexts` session marked `inbox`, migration `0054_contact_inbox.sql`, local table, `SESSIONS_WIRE_SCHEMA` unchanged) or (b) a contact's turn in an exported chat, from its `<contact_message>` turn start to its end. In a contact session only these exist:
- the A2A reply itself (the turn's final text; it needs no tool);
- the export's allowlist `ExportEntry.tools` (engine-local in `peers.yaml`, serde default `["search_web", "read_url"]`; `read_url` already refuses private, loopback and link-local targets, `web::fetch::ssrf`). It rides to the agent as `ContactOrigin.tools` (absent from an older holder = nothing).

Everything else is **hidden** (absent from `ListTools`, `unknown tool` on a call), for the session and its subagents: `send_message`, `spawn_subagent`, `discover_peers`, `session_read`, `search`, `knowledge_*`, `doc_*`, `artifact_read`, `nb_*`, `task_*`, `mesh_*`, `secret_*`, `read_file`/`grep`/`list_dir`/`write`/`edit`/`shell`, every MCP tool not listed. The risk cap still applies on top (a `read_only` cap drops a mutating allowlisted tool), and `CAP_EXEMPT` (`send_message`) never applies in a contact session.

The loop's own tools (`python`, `todo`, `zoom`/`recall`/`date`, `consult`, `advise`) obey the same allowlist: `rebase` asks `tools::Allowlisted` and drops each one not listed; without `python` the runtime index, the bridge and the `runtime_only` floor are off, so admitted tools are offered direct (found live: the inbox was still offered `python` and `todo`).

Built as an allowlist, not `SetHidden` (a denylist cannot hide tools registered later): `tools::SetAllowOnly{session, tools, turn}` — the session's own entry for an inbox (`install_contact_limits`, on every admission, every actor start and every reload), the turn-scoped entry beside `SetTurnCap` for a chat's contact turn (`set_turn_limits` in `start_turn`, cleared on every exit of the turn: finish, cancel, fail, and the Take/Move suspend). The UI worker adds the allowlist editor (`Export.tools`, TS).

Residuals (not fixed in R2; the resumed answer, the resumed persona and the re-put are fixed in Z3-R3 above): after a restart the resumed contact turn of an exported chat finishes capped but its answer does not go back (the holder's wait died; the contact's message stays accepted), and a resumed inbox turn runs with the engine's persona (the agent persona came with the admission). Contact framing text still reads "Tools above read_only are refused" (true, but not the whole rule). `a2a_export_put` without `tools` (today's UI) writes the default allowlist, so a hand-edited list is reset by a UI re-put until the UI worker's editor sends it.

### MUST (fail-before tests)

| Item | Fix | Test |
|---|---|---|
| M1 caps survive a restart (engine M1, sec M5) | Every actor start of an inbox (`Agents::session` → `reinstall_contact_limits`) re-installs its last admission's cap and allowlist, else fails closed (`read_only`, nothing but the reply; a store error counts as an inbox). A resumed contact turn in an exported chat is recognised from its opener row (`open_contact_turn`) and **resumes capped, fail-closed** (`read_only`, empty allowlist) — chosen over closing it as interrupted because the turn's work survives and the contact still gets an answer, while its origin (cap, allowlist) died with the process. A Take/Move suspend clears the turn limits on the old owner; the new owner resumes it fail-closed the same way. A child result waking an inbox goes through the same actor start. | `an_inbox_is_capped_and_default_deny_at_every_actor_start`, `a_resumed_contact_turn_is_recognised_from_its_opener` |
| M2 no relay (engine M2, sec M4) | No `CAP_EXEMPT` in a contact session; an inbox lists no contact in `discover_peers` and may not send to one (`granted = (contact && !inbox) \|\| …`) | `contact_sessions_see_only_their_allowlist_and_send_message_has_no_cap_exemption` (a chat contact turn's `send_message` refused), `an_inbox_neither_sees_nor_reaches_another_contact` |
| M3 first contact cannot be skipped (sec M1) | A message naming an `AUTH_REQUIRED` task is refused (`waiting: the owner has not allowed this task yet`) until the owner releases it | `first_contact_holds_until_the_owner_answers` (the `m-skip` leg) |
| M4 spawn_subagent (sec M3) | Hidden in contact sessions (the rule). Any native child gets min(requested, the spawner's ceiling now — `tools::EffectiveCap`, turn cap included) as its own cap; an external harness is refused under any ceiling (`child_cap`) | `a_child_takes_the_spawners_ceiling_and_external_harnesses_are_refused_under_one`; `EffectiveCap` in the registry test |

### SHOULD

- **Pairing**: one inviter slot taken atomically at `admit_request`; failures per remote id (3 refuse that id, 10 close the window); a ticketless request only from an id seen in this window's sightings. Test `one_handshake_at_a_time_failures_count_per_endpoint_and_ticketless_needs_a_sighting`.
- **Deadlines/bounds**: `serve_http` has a `TokioTimer`, a 30 s header read and a 60 s whole-request deadline (streaming is not carried over `theuth/a2a/1` yet, so nothing long-lived is cut); at most 4 inbound connections and 16 concurrent streams per contact. Per contact and export at most 8 waiting tasks (`CONTACT_QUEUED`); the 64-context bound of R2 is gone (Z3-R3). No tests for the caps/deadlines (timing).
- **SSRF**: a Nearby card URL is always built from the sighting's own address (an absolute TXT URL keeps only its path); card fetches resolve once, refuse private/link-local/loopback unless the address came from a sighting in this window, and pin the connection; a ticket's private `addrs` and a private IP-literal `relay` are dropped unless sighted. Residual: a relay *hostname* resolving to a private address is not resolved. Test `private_card_urls_and_ticket_addresses_are_refused_without_a_sighting`.
- **Discovery**: the card listener binds only the advert's LAN IPs (no 0.0.0.0; nothing advertised if none bind), at most 8 connections with deadlines; TXT is `card`, `proto`, `v`, `eid`, `pair` and `port` (the iroh UDP port, needed because the sighting's port is the card listener's); no `iroh` IPs, no login name (instance = mesh name).
- **Card strings**: name and machine cut to 64 chars wherever stored or framed; a new key under an existing contact's name pins as `bob-new-key` and the ceremony reads `bob (new key — not your existing bob)`. Tests `card_names_and_machines_are_cut_to_64_characters`, `a_same_name_new_key_is_flagged_in_the_ceremony_and_the_alias`.
- **Wrappers and owner input**: the inbox frame neutralises `<contact_…` in the body; owner input typed during a contact turn waits for the owner's next turn (it no longer joins; supersedes §13 row 50 and E3 call 10). Test `a_contact_turn_is_the_contacts_alone`.
- **Address churn**: `a2a_direct_addrs` ≤ 8; at most one write a minute per contact, only on a change; a failing write warns once per distinct error. In-entry comments still do not survive a write (serde_yaml has no comment model; the leading-block rule stands).
- **reload() coupling**: stamp taken before the read; unchanged HTTP peers whose last probe succeeded are not re-probed (the rest run concurrently); a failing HTTP peer no longer rolls back a contact edit (`reload_as(keep_http)` retries with the last good HTTP peers, the error stays in the status). Test `a_broken_http_peer_does_not_block_a_contact_edit`.
- **Defaults**: with `agent.communication` unset, contacts count beside paired machines (re-evaluated when contacts change). Test `absent_communication_counts_contacts_on_a_lone_engine`.
- **Small**: contact refusal texts name no mesh machine (`the chat's machine is offline`, `… needs an update`, an owner refusal reads `the chat is not taking messages right now`; the log keeps the detail). A cap or allowlist change reaches live inboxes at once (`ContactLimits` pushed after every reload; test `a_cap_or_allowlist_change_reaches_a_live_inbox_at_once`); a running exported-chat contact turn keeps its start limits until it ends. `peers.yaml` mode is checked on read (warn once if not 0600, unix).

- Gates (R2): `check --tests` and `clippy --tests --no-deps -- -D warnings` on `-p a2a -p agent -p tools -p rpc -p mesh -p roster -p secrets -p theuth` 0/0; `mbx test -p a2a -p agent -p tools -p mesh -p roster -p rpc -p theuth --no-fail-fast` twice with a fresh `t` as both workers, `/opt/homebrew/bin` off PATH: a2a 43, agent 818/2 failed and 819/1 failed (`repl::tests::capping_binds_out_name` both, failing before Z3; `sensor::tests::monty_budgets_and_allowlist` run 1 only, load), tools 220, mesh 62, roster 100, rpc 83, theuth 12, rest green. Bindings regenerated (`Export.tools?`, `ContactOrigin.tools`), tsc 0, vitest 317 files / 2107 tests.
- Live (`evidence.txt`, "Z3-R2 live"): P3-1 Nearby pair + capped inbox PASS; P3-7 shell, read_file (decoy path), search, session_read, spawn_subagent, python, todo each "not available in this session" PASS; P3-5 inbox `send_message` not available PASS; P3-4 exported chat contact turn with no tools, only the answer back, stop sharing refuses PASS; P3-8 a kill -9 mid contact turn (B's chat, C's inbox) resumes with no tools PASS (residuals above).

## Phase 3 closer round R1 (Z3-R1)

- **Merge.** `origin/main` with design 185 (eb2a2b3a) merged with no conflicts; bindings regenerated (1021 files), `tsc` 0.
- **Communication shows contacts as built** (HANDOFF 3). Settings › Communication › Network (A2A) lists each contact from `contact_list` as `bob [contact]` with `<machine> · theuth/a2a/1 over iroh · pinned <fingerprint> · <cap> · may reach …`, and leaves it out of the HTTP peers (no "local roots may send"); a chat export granted to a contact reads "reached by the contact over theuth/a2a/1, one capped turn per message" without its HTTP URL or Copy URL; the disabled line says contacts do not need `a2a.enabled`. The chat's design 50 "Exported: … approvals still ask you" banner now names HTTP-peer exports only (`useCommunication` drops exports granted to a contact; the header's `Shared with bob · read only` chip covers those). vitest: "Settings › Communication shows a contact as built …", "the chat banner names HTTP-peer exports only …".
- **Contacts keep their last direct addresses** (HANDOFF 2). `PeerEntry.addrs` (engine-local, skipped when empty): every hot-reload tick (2 s) `Ctx::remember_addrs` writes a contact's direct addresses from its live `theuth/a2a/1` connection (`Net::a2a_direct_addrs`: the selected and other non-relay paths, either direction) when they differ; a reload stages them (`ContactFacts.addrs`) and `Net::a2a_stream` dials them beside the caller's hints (`ContactAlpn::hints`). No live connection writes nothing, so a stale address is only replaced, never cleared. Test `a_contacts_last_direct_addresses_are_kept_and_offered_to_the_dial`; live (`evidence.txt`, Z3-R1): after a ticket pairing both files held the other's `127.0.0.1:<mesh port>`, both engines restarted with relay none and discovery off, and A's next `peer:` send reached C.
- **P3-2 stays NOT-RUN** (no new relay dependency); reason in the Z3 table and HANDOFF 1.
- Gates (R1): `check --tests` and `clippy --tests --no-deps -- -D warnings` on the set 0/0; tsc 0; vitest full 317 files, 2107 tests passed; `mbx test -p a2a -p agent -p mesh -p roster -p rpc -p theuth` once with a fresh `t` as worker (agent `--lib` included): a2a 38/38, agent 812 passed, 2 failed (`capping_binds_out_name`, failing before Z3; `sensor::tests::monty_budgets_and_allowlist`, its 5 s bound under load, passed alone), mesh 62, roster 100, rpc 83 + 4, theuth 12.

## Phase 3 as built + live proof (Z3)

Merge: `feat/183-p3-ui` (875dd184) at 41557f9e, `origin/main` (06ccf506, docs only) at 4f83b2df; no conflicts. Bindings regenerated (`export_bindings` 8 passed, 1021 files) and copied into the UI; `tsc -b --noEmit` 0. E3 gaps and reconcile fixes in 114ef04a, live-proof fixes in ac28eff5, these notes at the closing commit. Evidence `/tmp/rs183/p3/evidence.txt`, screenshots `/tmp/rs183/p3/shots/`, rig `/tmp/rs183/rig/{setup3,start3,stop3}.sh`, `fake3.py`, `ev2.ts`, `r3`.

### Reconcile: U3's assumptions against E3 and the doc

| U3 assumed | Doc / engine | Settled |
|---|---|---|
| Cap per contact, not per export | §9.4 `PeerEntry.cap`, §14.4 `Export` has no cap, §7.6 "contact.cap" | Kept per contact; no shape change |
| `contact_first_answer` with nothing waiting just saves the default | E3 answered `ask` with no write and `block` blocked the whole contact | Engine: with no task waiting for (contact, export) the answer is that export's standing default (§9.4): `ask` drops the entry, `allow` and `block` write it. With a task waiting, the Attention semantics stand (Allow once releases, Always writes `allow`, Block blocks the contact, §7.6) |
| Sharing reuses or creates the export (`a2a_export_put`), the last ungrant removes it | `ExportPut`, `ContactUpdate.exports`, `ExportRemove` as built | Unchanged; proven live (P3-4) |
| Agent exports are `<agent>@<this machine>` | §7.6 | The inbox strips `@<engine>` to find the roster agent (below) |
| Nearby: the engine fetches and verifies the card on Add | §7.1 "shown unverified and cannot start the ceremony" | UI: a Nearby row whose Add came back `unverified card` / `the card does not match the advert` is marked **unverified** and its Add is disabled for the window |
| "The codes differ" = Reject; subtitle `os · fingerprint` | §12.7 "name, machine and fingerprint"; `PairCeremony` has no machine | `contact_answer(false)` sends `Reject` (the other side reads rejected). A contact ceremony's `PairCeremony.addr` now carries the contact's machine (from its card; `""` before): the subtitle reads `c · macOS · 5NCW RWFL 7CFI 5MIW PL2R`. The UI's `fingerprint()` encodes a 64-hex endpoint id as base32 like the engine's `Contact.fingerprint` (it showed hex) |
| Composer chip not built | §12.10 | Built: `read only · bob's turn` above the composer while a turn opened by a `<contact_message>` runs (`SessionSlice.contactNote` from `TurnStarted`, cleared at `TurnFinished`; after a reload mid-turn, the last opener in the transcript) |

### E3 gaps (114ef04a)

1. **Exported agent's persona (W-25, MUST).** `IdentitySource::inbox(agent) → InboxPersona{identity, model, tools}` (roster implements it: `desk::inbox_identity` = who it is, its charter and capabilities, never its journal or desk manual; theuth keeps the default persona). A new inbox starts on the agent's `model:` (the default if that fails), its system prompt leads with that identity (recorded per inbox on every resolve, so after a restart too), and it runs under the stricter of the contact's cap and the agent's `tools:`. Test `an_agent_export_inbox_runs_the_exported_agents_persona_model_and_ceiling`.
2. **Addressing.** §7.7/§11 name no per-export form, so `peer:<contact>` keeps reaching the first export granted (name order) and `peer:<contact>/<export>` reaches that export: the contact's route, with the export riding on the request (`OutboundPeerMessage.export` → `IrohTransport.export` → `POST /agents/<export>/rpc`; the follower polls the same export). Only a contact takes it; the export must be `[a-z0-9_-]`. The `send_message` description names the form. Tests `a_contact_export_address_names_the_contacts_route` (agent), the `zeta` leg of the a2a e2e first-contact test.
3. **`A2aChanged` on archive and unarchive.** `archive_session` (local or forwarded to the owner) asks the a2a actor `ChatArchived{session}`, which publishes `A2aChanged` when that chat is exported here. Test `archiving_an_exported_chat_fires_a2a_changed`. Not covered: an archive made on the owning machine's own UI for an export held elsewhere (the holder refetches on its next `a2a_exports`).
4. **`peers.yaml` comments.** §7.8 says only that the RPCs write the file atomically; it is silent on comments. E3's rule (the leading comment block survives, comments inside entries do not) stands.

### Defects found live and fixed (ac28eff5)

| Defect | Fix |
|---|---|
| A contact could not reach the side that dialed: either side reuses a live `theuth/a2a/1` connection, but the dialer never accepted streams on its own connection (C→A timed out "no answer from the contact in time") | `mesh::net::serve_a2a` serves every contact connection, dialed or accepted, with the per-stream admit. Test `the_side_that_dialed_is_reached_on_its_own_connection` |
| A remounted Add contact dialog (React StrictMode, or a fast close/reopen) left no window: the engine served `contact_window` calls concurrently and a close overtook the still-opening window, dropping its advert | Engine: `ContactWindowOpen` runs in the a2a mailbox (call order). UI: `contact_window` calls go through one promise queue |
| A contact inbox was retitled from its first row (`<contact_inbox …`) | `handle_inbox` marks the session titled after setting "bob · research" (as a rename does) |
| No way to prove Nearby agents without mDNS on en0 (swarm-discovery always multicasts on the default interface; `with_multicast_interfaces_v4` only adds interfaces) | Debug builds with `THEUTH_DEV_CONTACTS_DIR`: adverts as files, the signed card served on `127.0.0.1` only (`discovery::dev::DirLan`, as the mesh's `THEUTH_DEV_NEARBY_DIR`) |

### Calls made (Z3)

1. Settings' first-contact select vs the Attention item share `contact_first_answer`; "is a task waiting for (contact, export)" (`ContactsHeld`) decides which meaning applies.
2. `PairCeremony.addr` carries the contact's machine for contact ceremonies (doc comment updated; no new field).
3. The inbox applies the agent's `tools:` ceiling too (an exported read-only agent stays read-only under a `mutating` contact).
4. E3's e2e first-contact test now presses the Attention Block on a task that waits (a fresh export, `zulu`): a first-contact task queued behind the inbox's running turn is not an Attention item yet, so Block there is the Settings default for that export.

### Gates (Z3)

- `check --tests` and `clippy --tests --no-deps -- -D warnings` on `-p a2a -p agent -p tools -p rpc -p mesh -p roster -p secrets -p theuth`: 0 errors, 0 warnings (after the last test change too).
- `mbx test -p a2a -p agent -p mesh -p roster -p rpc -p theuth --no-fail-fast` twice, fresh `target/debug/t` as `THEUTH_MONTY_WORKER`/`THEUTH_WORKER`, `/opt/homebrew/bin` off PATH (the full agent `--lib` included): a2a 37/37, agent 813 passed, 1 failed (`repl::tests::capping_binds_out_name`, failing before Z3 as in E3's runs), 4 ignored, mesh 62, roster 100, rpc 81 + 4, theuth 12, notebooks 2 + 7; identical both runs.
- Bindings: `export_bindings` 8 passed, 1021 files copied; `tsc -b --noEmit` 0 errors; vitest full: 317 files, 2104 tests passed.

### Live proof (two meshes: A+B, C alone; loopback, fake model on 18750)

| Proof | Result |
|---|---|
| P3-1 contacts over LAN | PASS (Nearby via the dev directory, codes matched, pinned, `peer:bob/research` → capped inbox on C running Research's persona and model, shell refused with approvals off) |
| P3-2 contacts over the relay | NOT-RUN (relay leg): `crates/relay` is the phone-link WebSocket relay (design 31), not an iroh relay; with it as `mesh.relay` both engines report `connected: false`, and an iroh relay (`iroh-relay` `server`) would be a new dependency. The invite-ticket ceremony and messaging PASS over the ticket's direct address (path `lan`) |
| P3-3 first contact | PASS (waits with an Attention item on the receiver; Allow once; Always; Block writes `blocked` and closes the contact's connections — the gate's own refusal text was not observable live, see HANDOFF 2) |
| P3-4 exported chat | PASS (B-owned chat exported on A, `contact_deliver` to B, ContactMessageCard, capped turn, C got only that turn's answer, owner's next turn invisible, Stop sharing ends it) |
| P3-5 isolation | PASS (inbox `send_message(session:<owner chat>)` → `unknown peer`) |
| P3-6 single engine | PASS |

### HANDOFF

1. **An iroh relay for P3-2 (NOT-RUN).** Proving contacts over a relay needs a loopback iroh relay (`iroh-relay` with its `server` feature, not in `Cargo.lock`) or a test-utils relay in a test; `theuth-relay` cannot serve `mesh.relay`. Left NOT-RUN by ruling: no new relay dependency.
2. ~~Contacts keep no address hints~~ — done in Z3-R1 (below).
3. ~~Settings › Communication shows contacts as HTTP peers; stale exported-chat banner~~ — done in Z3-R1 (below).
4. **Streaming over `theuth/a2a/1`** (E3 call 5) is still polled.

## Phase 3 engine E3 (contacts over A2A, exported chats)

On `feat/183-p3` from edc227d6. Every `// 183 P3: E` marker is built and gone; the `// 183 P3: UI` ones are U3's.

### Phase 3 contract changes after C3

No generated TypeScript changes: every shape U3 imports (`Contact`, `ContactPatch`, `ContactTarget`, `ContactWindow`, `NearbyAgent`, `Export`, `ExportTarget`, `FirstContact`, `ContactEvent`, `ContactEnvelope`, `ContactOrigin`, `AttentionKind`, `PairCeremony`) is as C3 left it, and the 12 methods keep their names and params. What changed:

1. **The RPCs answer for real** (§10.1): `contact_window(true)` opens a window and returns its ticket (`contact_window(false)` closes it, `null`); `contact_pair` returns the ceremony at its code (`state: "waiting"`), then `ContactEvent::Pair` events carry `accepted` and the end (`paired` · `rejected` · `expired`); `contact_answer`, `contact_list`, `contact_update`, `contact_remove`, `a2a_exports` (agent and chat exports, `paused` set), `a2a_export_put`, `a2a_export_remove`, `contact_first_answer` act; `contact_deliver` serves W-40. The `off` answers (only a process without the a2a plugin) say `contacts are unavailable: the a2a plugin is not running` (`a2a::contacts::OFF` replaces `NOT_BUILT`).
2. **The A2A actor always runs** (W-26): `a2a_status` now comes from it with `enabled: false` (and `listening: false`) when `a2a.enabled` is off; HTTP peers stay dormant then, contacts work.
3. **`PeerEntry.machine: Option<String>`** (skipped when unset): the contact's machine from its card, so `Contact.machine` survives a restart. Not a TS type.
4. **`PairCeremony.addr`** is `""` for contact ceremonies.
5. Engine-internal: `A2aPrincipal.ask_first`; agent messages `ContactInbox`, `ContactRelease`, `ContactsHeld` (`HeldContact`), `ChatPaused`; constants `UNKNOWN_SESSION`, `PAUSED_ARCHIVED`, `PAUSED_MESSAGING`, `CONTACT_NOT_OWNER`; `loop_::ContactFrame`, `PeerInput.contact`, `PeerMeta.cap`; `IrohSender` removed (a contact's route is the a2a `PeerClient` over an iroh link); `mesh::net::ContactAlpn`, `Net::{set_contacts, a2a_stream, a2a_path, close_a2a, a2a_reach, sign}`, `mesh::net()`; `config::stage(…, contact_cap)`, `Staged.{agents, contacts}`, `Peer.eid`; `communication::mesh::{contact_forward, ContactFailure}`; `agent::spawn_engine_agents` (test-support).

### Calls made (E3)

1. **The ALPN is always registered**; the gate refuses everyone while the engine has no contact and no open window, which is what "the listener exists only then" (§7.2) observably means. Family peers are refused in `mesh::net` before the a2a gate runs.
2. **One inbox per (contact, agent export), reused** (§7.6 "later contexts … reuse it" over W-25's "fresh"), found through `a2a_contexts`. Its cap is installed on every resolve (after a restart too; a lowered cap applies at once).
3. **The inbox runs the default persona**: the exported agent's name titles the inbox and the log, but its roster persona is not loaded (that needs the roster's desk-opening block). Deviation; the cap, the scope and the wrapper hold.
4. **`peer:<contact>` addresses the contact, not one export**: it posts to `/rpc`, which the gate maps to the first export granted to that contact (name order). Contacts reach every local root (no `sessions` grant) and list in `discover_peers` tagged `contact`; the contact's own exports are not known here, so they are not tags.
5. **Streaming is not carried over `theuth/a2a/1` yet**: the iroh transport refuses `SendStreamingMessage`/`SubscribeToTask`, and the follower polls `GetTask` (2–30 s backoff). §7.2 "Streaming" is the follow-up.
6. **Comments**: a rewrite keeps the file's leading comment block; a comment inside an entry does not survive a UI write (serde has no comment model). Hot reload polls the file's mtime and size every 2 s.
7. **First contact**: the waiting text names this engine's mesh name; `contact_first` items come from a roster attention `Source` the plugin registers, and the plugin nudges a re-derive (the inbox's `AlertDue`) when a hold starts or ends. Allow once moves the task back to `submitted`.
8. **One principal per exported chat** still (HTTP peer or contact): a chat export is granted to one contact.
9. **W-40 answer path**: the owner sends the contact turn's final text back as a `contact_deliver` with `reply_to` = the message id the holder waits on (the C3 RPC reused, no new method); the holder completes the A2A task with it. A stopped or failed turn answers a one-line reason. A moved chat (`-32013`) is retried once on the new owner; an offline or old owner fails the task with the §11.4 texts.
10. **The turn cap is set and cleared by the session** around a contact's turn (before its first tool call, at its end, cancel included), not by the supervisor: race-free with the same `SetTurnCap`. The owner's own input steers into a contact's turn under its cap (§13 row 50); another contact's message never joins it.
11. **A deleted exported chat is dropped** from `peers.yaml` at reload and when `a2a_exports` runs, for HTTP session exports as well (they were a config error before).
12. **The card**: name = the login user (fallback the mesh name), machine = the mesh name, `os_name` from `platform::process::machine()`. The real mDNS advert serves the signed card on a LAN port only while a window is open; its TXT carries the theuth hint `iroh` (direct addresses) beside `eid` and `pair=1`.
13. **Revoke** on Remove is best effort; a received `Revoke` is logged, the entry is kept (the owner removes it).

### Tests (E3)

`a2a`: `contacts::store::tests::rewrite_keeps_the_header_and_the_entries`; `contacts::tests::{jcs_sorts_by_utf16_and_escapes_as_ecmascript, card_jws_vector_verifies_and_refuses_tampering}`; `config::tests::contacts_and_agent_exports_stage`; `server::tests::the_contact_gate_admits_a_pinned_contact_and_refuses_strangers_and_blocked_ones` (three real loopback endpoints); `contacts::e2e_tests::{the_advert_and_the_browse_live_only_while_a_window_is_open, a_ticket_ceremony_pins_both_sides_after_both_accept, reject_reuse_and_expiry_are_refused_and_three_failures_close_the_window, a_broken_commitment_or_a_forged_card_is_rejected, a_contact_reaches_its_export_first_contact_waits_and_block_refuses_at_the_gate}` (two engines with live agents, fake LAN). `tools`: `registry::tests::turn_cap_refuses_above_it_and_never_replaces_the_session_cap`. `agent`: `communication::network::tests::{first_contact_holds_until_the_owner_answers, a_contact_inbox_is_one_capped_root_reused_per_contact_and_export}`, `supervisor::tests::an_exported_chat_elsewhere_runs_one_capped_turn_and_answers_only_it`, `loop_::tests::{a_contact_turn_is_its_own_and_only_the_owner_joins_it, contact_message_wraps_and_neutralises_the_body}`.

### Gates (E3)

- `check` and `clippy --tests --no-deps -- -D warnings` on `-p a2a -p agent -p mesh -p rpc -p roster -p secrets -p theuth -p tools`: 0 errors, 0 warnings.
- The whole set twice (`mbx test` on that set, fresh `target/debug/t` as `THEUTH_MONTY_WORKER`/`THEUTH_WORKER`): a2a 35/35 both runs; agent 810 passed, 2 failed, 4 ignored each run (`capping_binds_out_name`, plus `sensor::tests::monty_budgets_and_allowlist` in run 1 and `supervisor::tests::human_input_releases_parked_items_as_steers` in run 2, both passing alone); mesh 62, roster 100, rpc 81 + 4, secrets 40, theuth 12 + 9 all pass; tools 217 and 216 of 219 + 1 (load-sensitive `jobs::tests::{cancel_kills_group, close_sessions_kills_reaps_and_removes_logs}` in run 1, three `shell::tests` probe tests in run 2, all passing alone).
- Bindings: `export_bindings` 8 passed, 1021 files (unchanged set) copied; `tsc -b --noEmit` 0 errors.

## Phase 3 contract (C3)

Skeleton only, on `feat/183-p3` from b48578c2: every shape of §14.4 and §14.6, the invite ticket, the ceremony, the signed card and the Phase 3 RPCs of §10.1 compile; old wire and today's `peers.yaml` decode; behaviour is unchanged. Each place behaviour goes carries a one-line marker: `// 183 P3: E` (engine) or `// 183 P3: UI`. `rg -n "183 P3"` lists them.

### Migrations

None. §9 names no Phase 3 table or column: contact state lives in `<data_dir>/a2a/peers.yaml` (W-24, §9.4), and the existing local tables carry the rest: `peer_bindings` (`endpoint` = `iroh:<endpoint id>`, `revoked_at` for Remove/Block) and `a2a_contexts` (the inbox per (principal, export)) from `crates/agent/migrations/0011_communication.sql:47-64`. The next free numbers stay agent `0054`, mesh `0008`, secrets `0004`; `SESSIONS_WIRE_SCHEMA` is unchanged.

### Shapes (file:line at this commit)

| Area | Shape | Where |
|---|---|---|
| Config §9.3 | `A2aConfig.mdns_types: Vec<String>` (`MDNS_TYPES` = `_a2a._tcp`, `_agent._tcp`, `_lad-a2a._tcp`), `.contact_window_secs: u64` (600), `.contact_cap: tools::Risk` (`read_only`): read, not acted on | `crates/a2a/src/config.rs:28`, `:38`, `:46` |
| peers.yaml §9.4 | `FirstContact{Ask, Allow, Block}` (lowercase, default `ask`) | `crates/a2a/src/config.rs:124` |
| | `PeerEntry` + `endpoint`, `name`, `cap: Option<Risk>`, `first_contact: BTreeMap<String, FirstContact>`, `blocked`, `paired_ms` (serde default, skipped when unset); `PeerEntry::is_contact`; `PeersFile`/`PeerEntry`/`ExportEntry` gain `Serialize` for the atomic rewrite (§7.8) | `crates/a2a/src/config.rs:138`, `:173` |
| | `ExportEntry.session: Option<SessionId>`, `.agent: Option<String>` (exactly one) | `crates/a2a/src/config.rs:187` |
| Contacts §14.4 | `ALPN_A2A` (= `mesh::net::ALPN_A2A`, `crates/mesh/src/net.rs:45`), `BINDING_IROH`, `IROH_URL_PREFIX`, `TICKET_PREFIX`, `CODE_DOMAIN`, `CEREMONY_SECS`, `FAILED_MAX`, `NEARBY_MAX`, LAD limits, `JWS_ALG`/`JWS_TYP`, `EXT_MACHINE`, `NOT_BUILT` | `crates/a2a/src/contacts.rs:23-52` |
| | `Contact`, `ContactPatch`, `ContactTarget` (tag `kind`: `nearby` · `ticket` · `card_url`), `ContactWindow`, `NearbyAgent` | `crates/a2a/src/contacts.rs:61`, `:89`, `:104`, `:125`, `:136` |
| | `ExportTarget` (tag `kind`: `agent{agent}` · `chat{session}`), `Export{name, target, description, paused}` | `crates/a2a/src/contacts.rs:155`, `:171` |
| Events §10.4 | `ContactEvent::Pair(mesh::PairCeremony)` / `Nearby(Vec<NearbyAgent>)`, wire `{"type": "contact_pair" \| "contact_nearby", "data": …}`; forwarded by the bridge (`crates/rpc/src/bridge.rs:173`); nothing publishes it. `A2aChanged` unchanged | `crates/a2a/src/contacts.rs:188` |
| Ticket §7.4 | `InviteTicket{eid, name, relay, addrs, nonce, exp}` | `crates/a2a/src/contacts.rs:204` |
| Ceremony §7.4 | `PairLine{pair}` (first line `{"pair": …}`), `PairFrame::{Request{name, machine, commit, ticket}, Challenge{nonce, card}, Reveal{nonce, card}, Accept{exports}, Reject{reason}, Revoke{}}` | `crates/a2a/src/contacts.rs:224`, `:234` |
| | `Role{Inviter, Invitee}`, `CodeInputs{id_a, id_b, nonce_a, nonce_b}` (a = inviter on both engines), `Ceremony{id, role, peer, name, machine, nonce, commit, their_nonce, card, accepted, their_accept, expires_ms}` | `crates/a2a/src/contacts.rs:280`, `:291`, `:304` |
| Card §7.0, §7.3 | `CardFacts{name, description, machine, os_name, endpoint, public_url, skills}`, `JwsProtected{alg, kid, typ}` (envelope = SDK `AgentCardSignature`), `signed_card(facts, sign)`, `verify_card(card, eid)` | `crates/a2a/src/contacts.rs:342`, `:363`, `:374`, `:384` |
| Actor | `ContactWindowOpen(bool)`, `ContactsNearby`, `ContactPair(ContactTarget)`, `ContactAnswer{ceremony, accept}`, `ContactsList`, `ContactUpdate{name, patch}`, `ContactRemove{name, block}`, `ExportsList`, `ExportPut(Export)`, `ExportRemove{name}`, `ContactFirstAnswer{contact, export, answer}` and their `A2aActor` handlers | `crates/a2a/src/contacts.rs:395-461`, `:463-600` |
| ALPN §7.2 | `IrohGate::{Contact{binding, alias}, Pair, Refuse}`, `iroh_gate(live, remote, window_open)` (always `Refuse`) | `crates/a2a/src/server.rs:708`, `:726` |
| §14.6 | `ContactOrigin{contact, machine, export, cap, holder}`, `ContactEnvelope{id, to, text, origin, reply_to, request_id}` (TS) | `crates/agent/src/communication.rs:625`, `:642` |
| | `ContactDeliver{envelope, forwarder}` (`ask` → `Result<PeerReceipt, String>`), handler stub | `crates/agent/src/communication.rs:663`, `crates/agent/src/communication/network.rs:2316` |
| Refusals §11.4, §7.10 | `ContactRefusal::{NotAllowed{contact, export}, NotAContact{contact}, Paused{reason}, NoAgentMessages, OwnerOffline{machine}, OwnerOld{machine}}`, texts as `Display` (after `waiting: ` / `failed: `) | `crates/agent/src/communication.rs:970` |
| Addressing | `A2aPrincipal.contact: Option<ContactOrigin>` (`None` = HTTP peer), `IROH_PREFIX`, `contact_endpoint(eid)`, `contact_eid(endpoint)`, `IrohSender{eid}` (`PeerSender`, refuses; never registered) | `crates/agent/src/communication/network.rs:89`, `:213-241` |
| Turn cap §14.6 | `tools::SetTurnCap{session, cap: Option<Risk>}`, no-op handler | `crates/tools/src/registry.rs:530`, `:537` |
| Attention §10.4 | `AttentionKind::ContactFirst` (`contact_first`), alert class needs-you | `crates/roster/src/attention.rs:82`, `crates/roster/src/alert.rs:60` |
| RPC §10.1 | `contact_deliver(envelope) → PeerReceipt` (`Forwarded` only) | `crates/rpc/src/api.rs:251` (impl `:4077`) |
| | `contact_window(open)`, `contact_nearby()`, `contact_pair(target)`, `contact_answer(ceremony, accept)`, `contact_list()`, `contact_update(name, patch)`, `contact_remove(name, block)`, `a2a_exports()`, `a2a_export_put(export)`, `a2a_export_remove(name)`, `contact_first_answer(contact, export, answer)` | `crates/rpc/src/api.rs:2615-2656` (helper `contacts` `:3779`, impls `:9018-9088`) |

What the stubs answer today, with or without the A2A actor: `contact_list`, `contact_nearby` `[]`; `contact_window(false)` `null`; `a2a_exports` the configured session exports as `{target: {kind: "chat"}}` (`[]` without the actor); every other contact RPC `-32000 contacts are not built yet (183 phase 3)`. `contact_deliver` is `-32002` without `Forwarded`, else `-32000 contact_deliver: exported chats are not built yet (183 phase 3)`. No ALPN is registered, no frame is sent, no window or advert opens, `SetTurnCap` changes nothing, and no `contact_first` item is derived.

### Module split (E3)

| Module | Holds | Called from (markers) |
|---|---|---|
| `crates/a2a/src/contacts.rs` | store helpers over `peers.yaml`, the window and ticket, the ceremony, the card, the actor handlers | `crates/rpc/src/api.rs` `contacts`; `crates/a2a/src/lib.rs` plugin start (W-26) |
| `crates/a2a/src/config.rs` | staging of contacts and agent exports (both refused now) | `stage` |
| `crates/a2a/src/server.rs` | `iroh_gate`, the iroh stream into `app` with the gate's binding | `dispatch` (bearer step), `Handler::admit` (`contact`) |
| `crates/agent/src/communication/network.rs` | addressing (`contact_endpoint`/`contact_eid`), `IrohSender`, contact admission (first contact, pause, inbox), `ContactDeliver` | `admit_principal`, the queued-task re-admission |
| `crates/agent/src/communication.rs` | `Policy::remote` without `a2a.enabled` for contacts (W-26) | `remote` |
| `crates/agent/src/supervisor.rs` | chat delivery (W-39/W-40), `discover_peers` rows | `PeerDeliver`, `remote_owner`, `ListPeers` |
| `crates/tools/src/registry.rs` | `SetTurnCap` storage and `cap_for` | `SetTurnCap` handler |
| `crates/mesh/src/net.rs` | the `theuth/a2a/1` handler, the network join with contacts | `bind` (ALPN list, join) |
| `crates/roster/src/attention.rs` | `contact_first` items | `AttentionKind::ContactFirst` |

UI (U3) markers: `components/settings/Network.tsx` (the Contacts placeholder), `Communication.tsx` (the "Edit the file" line), `components/chat/Header.tsx` (shared-chat chip), `SessionMenu.tsx` (Stop sharing), `HarnessCards.tsx` (the two cards), `lib/injected.ts` (kinds), `components/attention/Screen.tsx` and `components/hud/Hud.tsx` (`contact_first`), `rpc/events.ts` (`ContactEvent`), `rpc/api.ts` (the 12 methods).

### Deviations from §14, and calls made

1. **No migration** (above).
2. **Commit and reveal in the ceremony.** §7.4 has `PairRequest{name, nonce_b, ticket}` then `PairChallenge{name, nonce_a, card}`: the inviter sees `nonce_b` before it picks `nonce_a` and could grind the code. The frames follow the mesh's Nearby fix instead (`Request.commit` = base32 `SHA256(nonce_b)`, `Reveal{nonce, card}`); both cards travel, so each side verifies the other's JWS. `Reject{reason}` is added (§7.4 names a Reject, §10.2 lists four frames).
3. **`contact_pair` returns `mesh::PairCeremony`** (reused, §14.5 lists no contact ceremony type): `peer` is the contact's endpoint id, `os_name` comes from its card.
4. **`ContactEvent` wire types are `contact_pair` / `contact_nearby`**: the UI dispatches every event on `type`, and `MeshEvent` owns `pair` and `nearby`.
5. **`ContactTarget` and `ExportTarget` are tagged `kind`** (snake_case: `card_url`); §14.4 gives `ExportTarget`'s tag only.
6. **`Export.name` is the `peers.yaml` key.** §14.4's `Export` has no card title; E3 writes `ExportEntry.name` from it on put.
7. **`contact_first` items carry contact and export in the id** `contact_first:<contact>:<export>` (both `[a-z0-9_-]`): no new `AttentionItem` field. Allow once answers `ask` (let this task through, keep asking), Always `allow`, Block `block`.
8. **Additions not in §14**: `A2aPrincipal.contact` (the inbox cap and the turn cap need the origin at admission), `ContactDeliver`, `ContactRefusal` (§11.4 plus the §7.10/§13 texts), `CardFacts`/`JwsProtected`/`signed_card`/`verify_card`, `IrohGate`/`iroh_gate`, `IrohSender`, `contact_endpoint`/`contact_eid`, `PeerEntry::is_contact`, `MDNS_TYPES`, the ceremony constants.
9. **The machine in the card** is the extension `EXT_MACHINE` with `params{machine, os_name}`: an A2A card has no machine field.
10. **`ALPN_A2A` is defined in `mesh::net`** and re-exported by `a2a::contacts`: mesh cannot depend on a2a and the handler lives in `net.rs`. `a2a` gains direct `tools` and `mesh` dependencies (already in its graph through `agent`).
11. **Staging refuses Phase 3 entries until E3**: a contact entry is `peer <n>: contacts are not built yet`, an agent export `export <n>: agent exports are not built yet`, both or neither `set exactly one of session and agent`. Today's build failed the same files on unknown fields, so A2A stays off with a config error either way (§13 row 23). Test `config::tests::phase_3_fields_decode_and_are_refused_until_built`.
12. **No pause or turn-cap RPC**: `Export.paused` is derived (W-41), and the turn cap is the `SetTurnCap` tools message.
13. **`A2aChanged` is unchanged** (no payload); §10.4 only widens when it fires.
14. **The config template** (`crates/theuth/src/main.rs`) does not list the three keys yet; E3 documents them with the behaviour.

### Bindings the UI worker imports

New: `Contact.ts`, `ContactPatch.ts`, `ContactTarget.ts`, `ContactWindow.ts`, `NearbyAgent.ts`, `Export.ts`, `ExportTarget.ts`, `FirstContact.ts`, `ContactEvent.ts`, `ContactEnvelope.ts`, `ContactOrigin.ts` (1010 → 1021 files). Changed: `AttentionKind.ts` (`contact_first`), `methods.json` (+12). Reused: `PairCeremony.ts`, `PeerPath.ts`, `Risk.ts`. Hand-kept `rpc/api.ts`: the 12 methods and `METHODS`. UI edits forced by the types (each `// 183 P3: UI`): `attention/Screen.tsx` maps `contact_first` to the plain screen, `hud/Hud.tsx` labels it `contact`; `rpc/events.ts` adds `ContactEvent` to the union.

### Gates

- `check` and `clippy --tests --no-deps -- -D warnings` on `-p a2a -p agent -p tools -p rpc -p mesh -p roster -p theuth`: 0 errors, 0 warnings (`Finished`, no diagnostic lines).
- `mbx test -p a2a -p agent -p rpc -p tools -p roster --no-fail-fast` (full suites, once): a2a 26 (with `contacts::tests::contact_shapes_keep_their_wire_names`, `config::tests::phase_3_fields_decode_and_are_refused_until_built`), roster 100, rpc 81 + 4 integration, tools 218 + 1, agent 803 passed, 4 failed: `sensor::tests::{fs_exists_through_monty, monty_budgets_and_allowlist, pr_watch_type_checks}` and `loop_::runtime_bridge_tests::guessed_direct_calls_are_bridged_or_refused`, all "monty worker crashed: unsupported protocol version 6" (the same environment failure C2 recorded; none in code C3 touched).
- Bindings: `export_bindings` 8 passed, 1021 files copied to the UI; `tsc -b --noEmit` 0 errors; vitest `src/rpc src/components/{hud,attention,settings,chat} src/lib/injected`: 93 files, 828 tests passed.

## Phase 2 follow-ups P2-F1 (final engine review should-fixes)

1. **Dial lock.** `close_conns` closes the control and inbound connections first, then waits for `outbound` (held across a dial, up to 30 s) and re-checks `is_paired`: an engine re-paired meanwhile keeps its fresh dials. Test `closing_never_waits_for_a_dial_and_spares_a_re_pair`.
2. **Remove ordering.** Remove is the unpair alone; `remove_peer` runs tombstone → `refresh_paired` → close connections → clear the quarantine entry last, so the machine is never paired in between, a failed removal leaves it listed, and a failure clearing the entry leaves no stale paired set. Test `removing_a_quarantined_machine_never_pairs_it` (fails on the old `remove_peer`).
3. **`refresh_paired` serialised** (`Net::refreshing`, store read → set write, `peers_gen` bump inside); connections close after the lock. No deterministic fail-before: the stale interleaving needs a hook between the read and the write.
4. **Tick without a vault.** The 5 s tick starts at once and runs the paired-set refresh vault or not; the vault (and `keys::start`) joins in the same task when `secrets::handle` answers.
5. **`connected()` filters by `is_paired`**, so `recompute`, `Transport::peers`, availability and status never count an engine that just left the set. Test `an_engine_out_of_the_paired_set_is_no_peer_and_no_gossip_source`.
6. **Gossip.** A heartbeat or `LeaseRelease` counts only when the neighbour that delivered it (`delivered_from`) and the engine it names are both paired (`Net::gossip_admits`). Not "the same engine": plumtree forwards frames, so a paired relay is normal. Residual: a paired engine that has not heard of the unpair yet can still forward a forged frame naming a paired id; gossip carries only lease liveness (no data, no keys), and the lease re-elects from the replicated `engine_stats`.
7. The device-key reset warning is logged once per change of the error (`Vault::reset_warned`), not every tick.
8. UI: the keep-paired confirm is described by the "was added by …" line and the vault warning (`aria-describedby="keep-added-by keep-vault-warning"`); vitest asserts both texts.
- Fail-before: the tests for 1, 2 and 5/6 fail with the old `close_conns`, `remove_peer` and `connected()` (all three failed in one mutation run). Gates: check and clippy on the R1 set 0/0; mesh/secrets/rpc/roster/document/link twice green (mesh 61); tsc 0; vitest settings 17 files, 110 tests.

## Phase 2 closer round R5 (Z2-R5): UI re-review items

- **Dial race.** `usePairFlow` gives each dial a token (`dial`); Cancel or a newer dial makes it stale, and a stale reply's ceremony is refused (`seen`, answered false), never shown. `dialing` counts pending dials so an unknown waiting ceremony still cannot open meanwhile. vitest "a cancelled dial's late reply never lands on the next dialog".
- **Unpair preview error.** Retry (refetch) next to the error; after a failed retry (`errorUpdateCount ≥ 2`) "Unpair without the list…" opens an inline confirm saying the list could not be read, then "Unpair without the list" sends `mesh_remove`. The main confirm stays disabled without a preview. vitest ×2.
- **Quarantine row.** Remove is the primary (the real unpair, R4); "Keep paired…" is secondary and opens an AlertDialog (Cancel focused) with the name, short id, who added it and the Pair vault warning (`VaultWarning`, shared with the pair dialog; `aria-describedby`). Row text: "not paired with this machine until you remove or keep it". Engine: `Store::peers` leaves quarantined rows out, so the row is no peer here (status list, lease, paired set) while the replicated row stays live (found live: before, it also showed as a normal peer).
- **a11y tests.** The pair dialog is described by the vault warning (`aria-describedby` → its text); the code is `role="group"` labelled "Pairing code"; the keep confirm is described by its warning.
- Live (`/tmp/rs183/p2/evidence-r5.txt`, shots `r5-a-quarantine-row.png`, `r5-a-keep-paired-confirm.png`): a machine seeded as introduced by C right after C's unpair was quarantined by A's own tick; the row and the confirm render as above; Keep paired moved it into the peers list; B's replicated row was never tombstoned.
- Gates: tsc 0; vitest full; mesh-set twice green after `Store::peers`; clippy/check 0/0.

## Phase 2 closer round R4 (Z2-R4): a quarantine never unpairs the mesh

- **Pairing records no origin.** `learn()` merges with `None`: the partner hands a fresh engine the whole mesh, so retiring the partner right after (pair the new laptop, retire the old one) quarantined every machine. Origins come only from `Msg::Peers` (the accomplice case).
- **Local-only quarantine.** `quarantine()` only marks `peer_origin.quarantined`; `refresh_paired` leaves those ids out of this engine's paired set. The replicated `peers` row stays live, so one engine's suspicion never tombstones a machine for everyone. Safe: the suspect still gets no connection, stream, key grant or rotation membership from this engine; on the others it is exactly as paired as they decided, and the only way to unpair it mesh-wide is a human. Keep paired clears the mark; Remove (`mesh_confirm_peer(id,false)`) clears it and runs the real unpair (tombstone, push, rotation).
- Reviewer minors: the sweep hold is persisted in the rotation record (`RotationState.sweep_hold_until_ms`, ts-skipped; an ack's release is not, so after a restart the hold runs to its deadline); `maintain` logs a failed device-key reset and still sweeps and retires; a `mesh/device_key` row that opens under no held key is an error, so the reset stays pending (logged by `maintain`).
- Tests: `keys::tests::retiring_the_pairing_partner_unpairs_nobody` (A, B, N pair through E; A retires E: A and B stay paired on N, A on B, nothing quarantined; fails with the old `learn` origin); `a_machine_introduced_by_a_removed_one_is_quarantined` now asserts the row stays live; `rotation_holds_the_first_sweep` holds across a restart; `a_crash_mid_sweep_resumes…` runs past the persisted hold.
- Residual: a machine another engine heard of only through E's `Peers` frame within `QUARANTINE_SKEW_MS` of E's unpair (the new laptop N, on A and B) is indistinguishable from an accomplice; it is quarantined on those engines only, with Keep paired / Remove, and nobody is unpaired mesh-wide.
- Gates: check and clippy on the R1 set 0/0; full mesh-set twice green (mesh 58, secrets 40); bindings unchanged; vitest full.

## Phase 2 closer round R3 (Z2-R3): nothing reaches a removed engine

- **Every connection, every ALPN.** `Net::close_conns` (run by `refresh_paired` for every id that left the paired set: a local unpair, a merged tombstone, a grant's tombstones, one the replica wrote, picked up by the 5 s tick, which now refreshes before its vault check) closes the control connection, every cached outbound dial (`outbound`: records, sessions, docs, acp, rpc, …) and every inbound connection the engine opened (`inbound_conns`, tracked by the stream-ALPN and gossip handlers).
- **Per stream.** The stream-ALPN handler re-checks `is_paired` before it hands out each accepted stream.
- **Per write.** Every stream `Net` hands out (`Counted`, both directions) carries its peer and the shared paired set; once the peer leaves, each read and write fails at once, so no shipper's in-flight batch (records replica, sessionsync, docs relay, acp, rpc) reaches it or is read from it. One chokepoint instead of a check in each shipper crate.
- Gossip carries only lease heartbeats (`GossipMsg`); its inbound connections from the engine are closed too, a gossip connection this engine's gossip actor dialed is left to iroh-gossip (no data rides it).
- Test: `net::tests::an_unpair_closes_every_stream_to_the_removed_engine` (open records and sessions streams both ways, unpair: the in-flight write fails, C reads nothing more, nothing from C is read, new dials on docs fail both ways).
- Live (`/tmp/rs183/p2/evidence-r3.txt`): A, B, C; after C's unpair A wrote a secret, a session and messages: C's counts stayed `vault=2 sessions=1 messages=2 docs=7` (0 rows applied after the tombstone; no "caught up" line), B got them all.
- Gates: check and clippy on the R1 set 0/0; full mesh-set twice green (mesh 57); vitest full.

## Phase 2 closer round R2 (Z2-R2)

1. **No engine ids in the rotation UI.** `RotationState.removed_names` (TS; in `removed` order) is filled by the mesh from the removed machines' tombstoned rows right after the mint (and on every tick); the vault keeps a known name once written. The dialog and the banner read "After unpairing c"; with no name, "a removed machine". Rotation peers read by their recorded name; an unknown id is "another machine". Tests: mesh `unpair_rotates_for_the_remaining_machines_only` (the record names `c`), vitest "rotation progress …" (the record's name, the fallback, no 8-hex run). Re-shot: `shots/q5-a-rotation-progress-names.png`, `q7-a-rotation-progress-names.png`, `q7-a-rotation-done-names.png`.
2. **Peer-message titles.** A chat whose first input is a peer message seeds `Message from <sender>: <first body line>` (`attach::peer_message_title`; the sender is its desk's agent name when the `from` session has one, a `peer:<name>`'s name, else "another agent"), never the wrapper. `session_title` and the UI's `chatName` (sidebar, tabs, header) and ⌘K's session rows (`cleanTitle`) rewrite a title seeded from the wrapper before R2. Tests: `attach::tests::a_peer_message_names_its_chat_without_the_wrapper` (the `peer_wrapper` shape after `<time/>`), vitest "183 R2: a chat seeded from a peer message …". Live: `shots/r2-a-sidebar-peer-message-title.png`.
3. **A machine we unpaired that never heard of it.** A Nearby dial whose request it drops (it still lists us as paired) reads "<name> was unpaired from this mesh; on <name>, remove this mesh first (Settings › Network › Other meshes)" when we hold its tombstone (`nearby::tests::a_dial_to_a_machine_we_unpaired_says_why`). Call: the automatic "you were removed" was not done. Letting the removed engine tombstone the remover in its own `peers` table would replicate that tombstone to any member still stale about it, which would then unpair the remover: a partition. A local-only marker plus its clearing on re-pair is not cheap, so the message alone ships (the brief's fallback). A `PairRequest` from an engine we hold a tombstone for is still accepted: it can only arrive once that engine dropped us, and then a fresh code ceremony is the way back.

Gates (R2): `check` and `clippy --tests --no-deps -- -D warnings` on the R1 set: 0/0. Full mesh-set once (all green but the R2 assertion on the adopter's record, removed: an adopter's `removed` is the member diff and is empty when its legacy key never named the removed machine), then `keys:: nearby:: vault::` twice; agent `attach:: title::` twice; vitest full; bindings regenerated.

## Phase 2 closer round R1 (Z2-R1): the review of V

Review `/tmp/rs183/p2/review-engine-v.md`. One commit; evidence `/tmp/rs183/p2/evidence-r1.txt` (rig script `/tmp/rs183/p2/r1-live.sh`).

| Item | Fix | Test (fail-before) |
|---|---|---|
| V-M1 stale paired set | `Net::refresh_paired` recomputes from the store on every merge (the early return is gone), on every `keys::tick` (5 s: rows the replica wrote), in `receive_grant` before anything is stored, and for `KeyRequest`, `push_grants` and `rotate_now`'s members (they use its returned live set). It closes the connections of an engine that left the set; `peers_gen` moves only when the set changed. | `keys::tests::a_replica_first_tombstone_still_refuses_the_removed_machine` |
| S1 accomplice | Local table `peer_origin` (migration 0007, not replicated): the engine whose peer set first carried a row. A live row whose introducer is tombstoned, introduced after (or within 5 min before) that tombstone, is tombstoned here (it replicates) and listed as `MeshStatus.quarantined`; Settings › Network shows "Added by <m> after it was unpaired — not paired until you confirm" with **Keep paired** / **Remove** (`mesh_confirm_peer(id, keep)`; keep = a fresh `added_ms` outranks the quarantine tombstone). | `a_machine_introduced_by_a_removed_one_is_quarantined`; vitest "quarantined …" |
| S2 single-copy window | `Vault::rotate` holds the sweep `SWEEP_HOLD_MS` (10 s); the first `KeyAck` for the new key calls `release_sweep`; `rotate_now` keeps calling `maintain` until the sweep ran or the hold passed. An old build never acks, so the hold always ends after 10 s. | `vault::tests::rotation_holds_the_first_sweep` |
| S3 retire | The per-key "no row names it / no `''` row opens with it" check and the `meta` write are one `BEGIN IMMEDIATE` on one connection (`secure_delete` on it). Trial-decrypt throttle: keys a `''` row opened with are kept without a new trial for 60 s (`LEGACY_CHECK_MS`); a key not known to open one is always tried inside the transaction before it can go, so the cache only ever keeps a key; a sweep that re-sealed `''` rows clears it. | `the_legacy_trial_is_throttled`, `a_legacy_row_opens_by_trial_and_holds_the_legacy_key` |
| S4 epoch overflow | `rotate`: `max(held, floor).checked_add(1)`, "vault-key epoch exhausted" at `u32::MAX`; `adopt_key` refuses a key more than 1000 epochs above ours (logged). Pairing (`adopt_keyring`) is not limited. | `rotate_after_an_exhausted_epoch_is_an_error_not_a_panic`, `adopt_key_refuses_an_implausible_epoch` |
| S5 W-35 | `after_pairing` rotates with `floor = their epoch` (→ `max(theirs, ours) + 1`). An adopter now sends `KeyEpoch` to its other machines at once (they learned the owner's key only on their next connect before). | `pairing_into_a_smaller_mesh_with_a_{higher,lower}_epoch_converges`, `rotate_honours_the_epoch_floor` |
| S6 D11 | `mesh::Unpair{id, reset_devices}` (`RemovePeer` = `reset_devices: false`) → `vault.rotate(.., reset_devices)`: `RotationState.device_reset_pending` is written with the mint. `Vault::finish_device_reset` (the rpc after the unpair, and every `maintain`) writes the new `mesh/device_key` and clears the flag, once (rechecked under the key lock). Link revokes its active devices whenever it sees `mesh/device_key` change, on every engine, not only the one that took the call. The device registry is the replicated `link.sqlite` table, so the preview's count is already mesh-wide. | `device_reset_pending_survives_a_restart_and_finishes_once`, link `a_changed_device_key_revokes_every_local_device` |
| S7 hygiene | `mesh::net::Secret` (serde-transparent, `Debug` `<redacted>`, zeroized) for `PairHello.vault_key` and `PairAnswer.vault_key`; `WireKey.key: Zeroizing<String>`. | `pairing_frames_redact_the_vault_key_in_debug` |
| S8 share links | On becoming the hub (`MeshEvent::Lease` flips `holding`), the document actor re-seals share links sealed under a retained key. | `hub::tests::becoming_the_hub_reseals_share_links` |
| S9 old_build | `on_connect` no longer resets `spoke`: a peer that sent `KeyEpoch` in this process is a 183 build. | `a_peer_that_spoke_is_never_called_an_older_build` |

Calls made (R1):

1. **Quarantine reach.** Origins are recorded only for rows a peer set carried (`Peers`, pairing). A row that reached this engine only by the record replica has no origin and is never quarantined here; the engine that recorded it quarantines it, and that tombstone replicates. Rows introduced up to 5 min before the unpair are quarantined too (clock skew), so a machine unpaired within 5 min of pairing can strand a legit machine it brought until a user keeps it.
2. **The sweep hold ends after 10 s in every case** (also with peers connected that never ack: an old build), so a rotation never waits forever.
3. **The legacy cache only keeps keys.** Caching a negative answer would let `retire` destroy a key a just-arrived `''` row needs; the brief's "throttle" is read as "do not re-try a key already known to be needed".
4. **Contract:** `Vault::rotate(me, members, removed, floor, reset_devices)`; `Vault::{release_sweep, finish_device_reset}`; `RotationState.device_reset_pending` (TS); `MeshStatus.quarantined: QuarantinedPeer[]` (TS); RPC `mesh_confirm_peer(id, keep)`; `Net::merge_peers(incoming, from)`; `keys::after_remove(net, id, reset_devices)`.
5. **The adopter's `KeyEpoch`** after a pairing (found by the W-35 test): without it the adopter's other machines learned the owner's key only on their next connect.

Gates (R1): `check` and `clippy --tests --no-deps -- -D warnings` on `-p mesh -p secrets -p agent -p rpc -p roster -p document -p link -p theuth -p autoresearch -p sandbox`: 0/0. Full `-p mesh -p secrets -p rpc -p roster -p document -p link` twice: document 85 + 29, link 31, mesh 55, roster 100, rpc 81 + 4, secrets 40, 0 failed both runs. `agent --lib` (fresh `t` as both workers): 804 passed, 2 failed: `capping_binds_out_name` (known) and `sensor::tests::monty_budgets_and_allowlist` (its 5 s bound under load; passed alone). `export_bindings` 8 passed; `tsc -b --noEmit` 0; vitest 316 files, 2072 tests passed.

Live (A–F on loopback, `t` with these changes; `/tmp/rs183/p2/evidence-r1.txt`):

| Proof | Result |
|---|---|
| Q5 rotation | PASS: C unpaired; A and B on cde6a30570de, C kept 062b19869540; FAKE_NEW under the new key on A and B, listed on B |
| Q5b replica-first tombstone | PASS: D's tombstone written straight into A's and B's `mesh.sqlite` (no merge, as the replica does); B closed D within 5 s (the tick's refresh); B's `vault_rotate` minted 9296b262efcd without D in its members; A took it, D never did |
| Q6 offline catch-up | PASS: B stopped, E unpaired, B restarted: `KeyGrant` b37593127aa6 (3 tombstones first), no read lost a name |
| Q7 device reset | PASS (link off): `mesh_remove(F, true)`: `mesh/device_key` v2 (`reset`) under af2a362b3cd1 on A and B, F kept v1; the record's `device_reset_pending` back to false |

HANDOFF (R1): a removed engine's other-ALPN connections stay open after its tombstone (only the control connection is closed), so in Q5 C still received the FAKE_NEW row by a replica stream opened before the unpair. It stays sealed (C logs "key cde6a30570deec64 not held" and is never granted it), but closing every connection to an engine that leaves the paired set would stop the ciphertext too.

## Phase 2 as built + live proof (Z2)

Merge: `feat/183-p2-ui` (043cac68) merged at 83a4a2d6; `origin/main` was already contained (the fold was a no-op). Bindings regenerated and copied into the UI; `tsc -b --noEmit` 0. Review fixes in 6100c2f9, live-proof fixes and site docs in c717c5fb, these notes at the closing commit. Evidence `/tmp/rs183/p2/evidence.txt`, screenshots `/tmp/rs183/p2/shots/`, rig `/tmp/rs183/rig/{setup2,start2,up2,stop2}.sh`, `r3`, `fake2.py`.

### Reconcile: U2's assumptions against N and V as built

| U2 assumed | As built | Change |
|---|---|---|
| LAN chip from `MeshStatus.nearby` | still the only source (no mDNS health field) | kept; the chip is the switch's state |
| "rotated at" = `started_ms` | `RotationState` has no finish time | done line reads "Vault key rotated (started 14:02)" |
| `devices` always 0 | real count of active link devices | the phones line shows only when > 0; the reset checkbox always shows (it also resets the device key) |
| `offline` / `old_build` / `peers[].id` as names | engine ids | mapped to names through `mesh_status` (8-char id fallback); the removed machine keeps the name the dialog was opened with |
| preview and rotation not built | V built both | U2-MUST-3 verified live: the preview lists the real names, confirm rotates (Q5) |

### Review items

| Item | Fix | Test (fail-before) |
|---|---|---|
| N-MUST code forcing | Commit-reveal: `PairRequest{commit = b32(SHA256(nonce_a))}` → `PairChallenge{nonce_b}` → new `PairReveal{nonce_a}`; the acceptor checks the reveal before it registers a ceremony, shows a code or counts a press. A reveal that does not match, or none within 10 s, is refused (`the pairing reveal did not match its commitment`) and counts as a failure at once. | `nearby::tests::{a_grinding_acceptor_cannot_steer_the_code, a_bad_reveal_is_refused_and_a_dropped_request_counts_at_once}`; V's `nearby_pairing_mid_rotation_carries_the_keyring` green |
| N-S1 / U2-S4 lease | `watching` → `watch_until_ms`: `mesh_nearby_watch(true)` holds 30 s (`WATCH_TTL_MS`), a timer refreshes when it lapses; a renewal keeps the window's counts, a fresh open resets them. The page sends true on mount, every 10 s, and on reconnect; false on unmount. | `the_watch_lease_lapses_without_a_heartbeat`; vitest "Nearby lifetime …"; live: a closed tab stopped advertising after 23 s, a restarted engine was re-armed by the open page |
| N-S2 caps | One ceremony per machine (a request waiting for its reveal counts), 4 at once (`too many pairings at once`; not a failure). Failures per machine: 3 → `too many failed pairings from this machine` for the window; 10 from anyone close the window. | `ceremonies_are_capped_per_machine_and_per_window`, `three_strangers_cannot_close_an_honest_window` |
| N-S3 token binding | `take_token(net, token, peer)` matches the ceremony's peer to the TLS-authenticated `conn.remote_id()` (`accept_control` already refuses a `PairHello.me.id` that differs from it). | `a_ceremony_token_is_bound_to_its_endpoint` |
| N-S4 adverts | `local_addrs`: private, link-local, loopback (v4) and loopback, link-local, unique-local (v6), at most 8, on our advert and every sighting; a sighting with none is dropped. TXT `name`/`os` cut to 63 bytes on a char boundary. | `adverts_keep_local_addresses_and_names_fit_their_txt_pair` |
| N-S5 retry in flight | `Agents.forwarding`: a same-`request_id` `Deliver` whose leg is in flight returns `Step::InFlight`, attaches its reply sender, and is answered with the leg's settled receipt (`reply: None`); no second forward. | `a_same_request_id_retry_during_the_forward_learns_its_outcome` |
| N alias capture | `Store::root_by_alias` returns owner-scope roots with their owner, newest first; `resolve` takes the first whose owner is the alias's `@engine` (its id, or its display name; this engine by `mesh::self_id`). | `desk_aliases_resolve_…` (a newer `ops@b` root owned by `c` does not win) |
| N typed refusal | `PeerDeliver`'s reply is `Result<PeerReceipt, PeerDeliverError>`; `api::peer_deliver` maps `SendError::HandlerError(PeerDeliverError::NotOwner)` to -32002. Texts unchanged. | `a_forwarder_that_does_not_own_the_sender_is_refused` asserts the variant |
| U2-MUST-1 focus | `onOpenAutoFocus` → preventDefault; Cancel (Done on the end screens) takes the focus each time it mounts (`cancelMount`), since `starting` → `code` swaps the button. | vitest "pair dialog focus: …" ×2; live: Cancel focused on dialer and acceptor |
| U2-MUST-2 cancel while connecting | cancelled-before-id flag; the late id goes to `seen` and is answered false; closing `starting` with an id answers false. | vitest "usePairFlow: Cancel while Connecting …" |
| U2-MUST-3 | V landed; verified live (Q5). | — |
| U2-S5 | confirm disabled until the preview resolved; `gcTime: 0`, `refetchOnMount: 'always'`. | vitest "unpair confirm is disabled until the preview resolved …" |
| U2-S6 | Nearby off: the section says so and points at the switch; `meshNearby` `staleTime: 0`. | vitest "Nearby off: …" |
| U2-S7 | `PairCeremony.addr` (new field); the incoming dialog shows `os · addr · 8-char id`; `meshPair` is a map by ceremony id. | vitest "usePairFlow: … address and short id …", "… a second ceremony does not blank …" |
| U2-S8 | `role="group"` on the code, `aria-describedby` → the vault warning, one `DialogTitle`, a named progressbar, `role="status"` only on the changing text. | — |

### Calls made (Z2)

1. **Failures: per machine and per window.** The 3-failure limit stays, per machine, because a window-wide 3 lets three strangers close an honest user's window. Endpoint ids are free, so a grinder could rotate them; the window-wide cap of 10 bounds that: with commit-reveal each failed ceremony is at most one blind 10⁻⁶ guess, so a window admits at most 10⁻⁵. A refusal at a concurrency cap shows no code and is not counted. Reopening Settings › Network (a fresh lease) resets the counts; a heartbeat does not.
2. **Nearby live proof on a file LAN, not mDNS.** swarm-discovery 0.6.5 always opens its main IPv4 socket on `0.0.0.0:5353` and joins the group on the default-route interface (en0) even with `with_multicast_interfaces_v4`, so it cannot be held to lo0. A debug-only `nearby::dev::DirLan` (`THEUTH_DEV_NEARBY_DIR`, compiled only with `debug_assertions`) writes each open window's advert to a file and reads the others; the dial and the ceremony are real iroh on loopback. No engine held a UDP 5353 socket (lsof); `tcpdump` needs root here.
3. **Expiry is told to the other side.** Windows differ per machine (`mesh.pair_window_secs`), so a dialer with a longer window read "rejected" when the acceptor expired. The side whose window ends now sends `PairReject{"the pairing window closed"}` and the other settles `expired`.
4. **`old_destroyed` once no older key is held.** With two older keys retained, retiring the oldest set `old_destroyed` (and the banner's last step) while the rotation's own old key was still held. Now it is set, and `waiting` → `done`, only when no retained key is left.
5. **Q7 with link off.** `mesh/device_key` is minted only by a link pairing, so the rig minted it with a throwaway hook (`Vault::mesh_device_key` on the stopped engine's vault; not committed). The reset is proven from the vault rows; no link device existed to revoke, and the link actor is absent, so `ResetDevices` is skipped (link's own test covers it).
6. **Recipient of a forwarded `await_reply` is not told a reply is expected.** `peer_deliver` enqueues with `await_reply: false`, so the remote `<message>` has no `reply_expected="true"`. A reply with `reply_to` still settles the waiter (Q1); the rig's fake model answers any "P2 ping". Left as is (HANDOFF).

### Contract changes after V

1. TS: `PairCeremony.addr: string` (serde default `""`).
2. Engine-internal wire (unshipped): `Msg::PairRequest{name, os, commit}` (was `nonce`), new `Msg::PairReveal{nonce}`, and `PairReject{"the pairing window closed"}` at expiry.
3. `mesh_nearby_watch(true)` is a 30 s lease (no signature change). `nearby::take_token(net, token, &EndpointId)`.
4. `agent::PeerDeliverError{NotOwner(MeshRefusal), Other(String)}`; `Message<PeerDeliver>::Reply = Result<PeerReceipt, PeerDeliverError>`. `Store::root_by_alias` → `Vec<(SessionId, String)>`.

### Gates (Z2)

- `check` and `clippy --tests --no-deps -- -D warnings` on `-p mesh -p secrets -p agent -p rpc -p roster -p document -p link -p theuth -p autoresearch` as one set: 0 errors, 0 warnings (the desktop crate is untouched; the UI is TS only).
- Full `-p mesh -p secrets -p rpc -p roster -p document -p link`, twice on the final tree: document 84 + 29, link 30, mesh 49, roster 100, rpc 81 + 4, secrets 34; 0 failed both runs.
- Touched suites twice: `nearby::` 13 + 13, the set above twice, agent twice (S5, alias, -32002 tests green in both final runs).
- Full `agent --lib` (fresh `t` as both workers): 805 passed, 1 failed, `repl::tests::capping_binds_out_name` (known, as in N2 and V2).
- Bindings: `export_bindings` 8 passed, `PairCeremony.ts` gains `addr`; `tsc -b --noEmit` 0; vitest 316 files, 2071 tests passed.
- Build note: midway, `btls-sys`'s build script re-ran over its existing `OUT_DIR` and prefixed BoringSSL's `libssl.a` twice (`_btls_sys_btls_sys_SSL_*`), so every test binary linking `wreq` failed to link. Removing that one `target/debug/build/btls-sys-<hash>` directory and its rlib rebuilt it once; the gates above ran after that.

### Live proof (scratch engines on loopback, fake model and secrets)

A 18561/18661/18461, B 18562/18662/18462, C 18563/18663/18463; D, E, F (18564–18566) for the refusals, the offline unpair and the device reset. `agent.communication` absent everywhere; relay none, discovery off, link/nodes/signal/slack/telegram/update/sandbox off. The engines ran `t` built at 6100c2f9 (every review fix); the two engine defects found live (calls 3 and 4) were fixed afterwards and are proven by their unit tests, the UI fixes live through vite's reload. Lease, live: a closed tab (no unmount) stopped advertising after 23 s; a restarted engine was re-armed by the open page.

| Proof | Result |
|---|---|
| Q1 messaging by default | PASS: source `mesh` on A and B after pairing (`default` before); A's `send_message(session:<B>, await_reply)` returned "P2 pong from B" |
| Q2 per-machine switch | PASS: off → `… agent messaging between a and b is off on a`; on → works |
| Q3 no mailbox stall | PASS: B SIGSTOPped with two sends from A in flight; 20 `get_communication` on A, max 1 ms |
| Q4 Nearby and the code | PASS: C in A's Nearby; 604 226 on both; Cancel focused; one press each → paired. Mismatch (Cancel) and expired (an 8 s window) refused, nothing stored |
| Q5 rotation | PASS: preview listed FAKE_API_KEY, FAKE_TOKEN, fake; A and B on c727d5e285db, C kept 2c017ddb989c; FAKE_NEW reached B only; its row copied into C's store stays sealed |
| Q6 offline machine | PASS: B stopped, D unpaired, B restarted: `KeyRequest` → `KeyGrant` ae5557669ee0, no read lost a name |
| Q7 device-key reset | PASS (link off): `mesh/device_key` v1 → v2 (source `reset`) under the new key on A and B; E kept v1 |
| Q8 Phase 1 | PASS: P1 forward without take (owner b epoch 1), P4 relayed approval allowed on A ran on B |

### Defects found live and fixed

1. The dialer's Cancel lost the focus to the dialog when `Connecting…` turned into the code (c717c5fb).
2. The rotation progress named the removed machine by its 64-char id (c717c5fb).
3. A dialer with a longer window read "rejected" when the acceptor expired (call 3).
4. `old_destroyed` and `done` before the rotation's own old key retired (call 4).
5. The alias test's second writer handle collided on `session_log.seq` (test only).

### HANDOFF

1. A removed machine keeps its own peers rows (the remover tombstones it and closes the connection; the tombstone never reaches it), so it still lists its old peers as paired, treats the remover's Nearby request as a paired connection, and the remover reads "not open for pairing" (`shots/q4-a-dial-stale-peer-refused.png`). Re-pairing it needs `t mesh remove` on its side first, or a removed-notice frame.
2. The forwarded `await_reply` flag (call 6).
3. The incoming pair dialog lives on the Network page only (U2 note, §12.6).
4. The rotation banner names a removed machine by its 8-char id once it is gone from `mesh_status`.

### Release note

Every machine in a mesh needs this build. A machine still on a build before 183 Phase 2 cannot read the mesh's secrets after a vault-key rotation (the first unpair, or Settings › Network › Advanced › Rotate vault key) until it upgrades; the rotation progress names it "runs an older theuth". Agent messaging is now on by default once a machine is paired, unless `agent.communication` is set. Pair nearby machines from Settings › Network by comparing a 6-digit code. Unpairing a machine rotates the vault key; the removed machine keeps what it already had, so replace those secrets at their providers if it is lost.

## Phase 2 engine N (messaging, Network, Nearby)

Every `// 183 P2: N` marker is gone; `// 183 P2: V` and `// 183 P2: UI` stay. `crates/mesh/src/keys.rs` and `crates/secrets` are untouched.

| Item | As built |
|---|---|
| §6.1 default on | Absent `agent.communication`: on with ≥ 1 paired machine, source `mesh` (`get_communication` shows it), else off (`default`). Startup reads `mesh::paired_count()`, else `mesh::has_stored_peers`. Re-evaluated by `Agents::mesh_default` on every `MeshEvent::Peer` / `Lease` (no "peer added/removed" event exists: pairing ends in a connect, removal sends a disconnect, the lease event follows mesh start). `config`, `env`, `command` are never moved. Autoresearch's `AgentDefaults::from_runtime(rt, paired)` takes the same default. |
| §6.1 template | `DEFAULT_CONFIG` writes `  # communication: false …`. `first_run` rewrites a config whose line is byte-identical to the old template line, once (tmp + rename; `write_config_atomic` writes values, not comments). |
| §6.2 switch | `mesh::messaging_off(id)` (a static the plugin and `SetMessaging` keep, written to `mesh.messaging_off` as a flow list). Sender: `forward_prepare` fails the exchange with `denied: send_message: communication_disabled: agent messaging between <self> and <peer> is off on <self>` (one place, so child results to a remote parent are refused too). Receiver: `peer_deliver` refuses `… between <forwarder> and <self> is off on <self>`. |
| §6.3 forward leg | `impl Message<Deliver>` is `DelegatedReply`: `deliver_step` resolves, routes, accepts and registers the waiter in the mailbox and returns the `ForwardJob`; the job runs on a spawned task and `ForwardOutcome` settles the receipt (a failure drops the waiter and `fail_exchange`s; a timeout after the owner already replied keeps the reply). `receipt_error` no longer asks `mesh::Status`: `last_seen` reads `mesh::last_seen_ms` (the connection bookkeeping peer events keep). `deliver()` (tests, `peer_deliver`) still runs the leg inline. |
| §6.4 retry | `accept_in`: a stored exchange `failed` with `{"OwnerUnreachable"…` goes back to `accepted` on a same-`request_id` retry and is forwarded again. |
| §6.5 guards | `PeerEnvelope.chain = ledger.chain(from)` on send; `peer_deliver` hands it to `enqueue_peer` (`Agents.inbound_chain`), and `Ledger::{admit_chained, chain_exhausted_for}` take `carried`. A `Forwarded` `peer_deliver` whose forwarder is not the row owner of `envelope.from` is refused `peer_deliver: <engine> does not own session:<from>`, `-32002` in rpc. A forwarded message into an engine whose ceiling is off reads `communication is off on <self>`. Caps unchanged. |
| §6.6 addressing | Roster: a new desk gets `SetPeerScope{Owner, "<agent>@<engine>"}`; an older desk keeps its scope and gains the alias. `resolve`: a `Relative(name)` with no such child falls back to the newest owner-scope root with that alias (`Store::root_by_alias`). `discover_peers`: remote rows carry `reachable` (`mesh::connected_ids()`); `query` also matches tags and engine (`peer_matches`). Policy refresh: `mirror` of a known root owned by another engine compares its row's scope and alias (`Store::root_scope`) and applies a change as `SetPeerScope` does (waits expire on a scope change); done in `mirror`, which every resolve and intake runs, since `apply_lineage` is the off-mailbox first-sight path. |
| §6.7 MeshStatus | `Peer.path`/`rtt_ms` from the first live control connection: the selected iroh path (else the first), relay → `relay`, direct → `lan`, RTT from the path stats; not connected → `offline`. `relay`: `home_relay_status()` (url, connected, last error), the configured URL before the engine joins; `None` with `mesh.relay: none`. `mesh_rename`: self → `mesh.name` in config, the live name and (with a peer) our own replicated `peers` row; another machine → its `peers.name`, pushed. `mesh_set_nearby` writes `mesh.nearby`. |
| §6.8 Nearby | `nearby::Mdns` trait: `Swarm` (swarm-discovery, the crate iroh's mDNS lookup already pulls in) advertises `_theuth-mesh._udp` with TXT `v=1 id name os`; tests use `nearby::fake::Lan` (in-process, no socket). Runs only while `mesh_nearby_watch(true)` or the `t mesh pair` window (`PairingString` opens `mesh.pair_window_secs`) is open and `mesh.nearby` allows it. `mesh_nearby`: unpaired sightings, newest first, ≤ 32. Ceremony: `PairRequest{name, os, nonce}` → `PairChallenge{nonce}`; both sides publish `MeshEvent::Pair` with the same code; the dialer's Pair sends `PairAnswer{accept}`; the acceptor answers only once both pressed, with a fresh token and its vault key; the dialer then runs `Net::pair_with` (the pasted-string path, vault key and merge unchanged) and the acceptor's `accept_control` takes the ceremony token. Cancel, or the window, ends it (`rejected` / `expired` on both); 3 failed ceremonies on the acceptor close its window. |
| P1 leftover | `store::tests::sessions_wire_schema_tracks_the_replicated_columns` snapshots the columns of `capture::ATTACHED` with `SESSIONS_WIRE_SCHEMA`. |

### Calls made

1. **Nearby does not join the iroh network.** §6.8 says a lone engine joins for the window. The LAN dial uses the advert's addresses, and mDNS runs its own responder, so the relay and pkarr stay dark; pairing joins the network as a pasted string does.
2. **The acceptor's accept waits for both presses** and carries the vault key with the token (`Msg::PairAnswer.vault_key`, below): the key leaves only after both humans matched the code, as a pairing string carries it.
3. **`last_seen`** is the live connection's bookkeeping: now while connected, else the last disconnect, `None` when never connected in this process. Before it was `max(last disconnect, added_ms)` from `Status`.
4. **Relay `rtt_ms` is always `None`**: iroh 1.4.1 measures relay latency only in its unstable net report.
5. **A desk alias collision is not cleared** on the other root (§13 row 28): that row may be owned elsewhere. `root_by_alias` takes the newest.
6. **Child-result forwards** (`deliver_result`, `deliver_spared`) still run their network leg inline in the mailbox; only `send_message`'s `Deliver` has an actor ref to post back to. `ponytail:` comment on `forward_peer`.
7. **`-32002` for a forged `from`** is mapped in `api::peer_deliver` from the supervisor's refusal text.
8. **Ceremony end on a closed stream**: the dialer reads `expired` when the stream closes within 1 s of its own deadline (the acceptor's window started one round trip earlier), `rejected` otherwise; once `pair_with` runs, only it settles.

### Gates (N2)

- `check` and `clippy --tests --no-deps -- -D warnings` on `-p mesh -p agent -p rpc -p roster -p theuth -p secrets -p autoresearch`: 0 errors, 0 warnings.
- Touched suites, each run at least twice on its build set: agent (15 tests: `absent_communication…`, `the_per_machine_switch…`, `a_forwarder_that_does_not_own…`, `the_chain_is_inherited_across_a_forward`, `a_same_request_id_retry…`, `a_blocked_owner_does_not_stall…`, `desk_aliases_resolve…`, `a_discover_query…`, `directory_marks…`, `sessions_wire_schema…`, the 69 P forward tests), mesh (`nearby::tests` ×5, `mesh_status_paths_follow_the_connection`, 4 runs), theuth `the_template_upgrade…`, autoresearch `config::`.
- Full `agent --lib` (fresh `target/debug/t` as both workers): 803 passed, 2 failed: `capping_binds_out_name` (known) and `sensor::tests::monty_budgets_and_allowlist` (5 s bound under load; passed alone twice). Full `mesh -p rpc -p roster`: mesh 36, roster 100, rpc 81 + 4 integration, 0 failed.
- Bindings: `export_bindings` 8 passed, output identical to the C2 copy; `tsc -b --noEmit` 0 errors.

## Phase 2 contract changes after C2

No TypeScript shape changed.

1. `Msg::PairAnswer` gains `vault_key: Option<String>` (engine-internal wire, serde default, skipped when `None`).
2. `mesh::nearby` is public: `Mdns`, `Swarm`, `Advert`, `Seen`, `SERVICE`; `BindOpts.mdns: Option<Arc<dyn Mdns>>` (`None` = real mDNS).
3. `Net.name` is private behind `Net::name()`; new `Net::{rename, rename_peer, relay_state, last_seen, nearby_events, pair_with}`; `mesh::{paired_count, last_seen_ms, messaging_off, set_messaging_off}`.
4. Agent-internal: `Ledger::{admit_chained, chain_exhausted_for}` take `carried`; `peer_mesh::Failure::receipt_error` and `peer_mesh::last_seen` are sync; `peer_mesh::UNREACHABLE_PREFIX`; `Store::{root_by_alias, root_scope}`; `Message<Deliver>` replies through `DelegatedReply` (callers' `ask` unchanged).
5. (V) The `VaultKeys` seam is gone: `Net` holds the process's `Arc<secrets::Vault>` (`BindOpts.keys` → `BindOpts.vault: Option<Arc<Vault>>`, `None` = the secrets plugin's, handed over by `Net::set_vault` from `secrets::handle`; `Net::vault()`). With it go the dead actor messages `GetMeshKey`, `AdoptMeshKey`, `Rotate`, `AdoptKey`, `Keyring` and `Vault::adopt_mesh_key` (replaced by `Vault::adopt_keyring`, which pairing uses: the owner's first key current whatever its order, the rest retained, then the sweep). `GetRotation` stays (`vault_rotation`).
6. (V) New `Vault` API: `set_engine`, `current_key`, `held_keys`, `missing_keys`, `watch_rotation`, `set_legacy_members`, `set_rotation_peers`, `adopt_keyring`, `maintain(now)`, `reseal_with_mesh_key`, `names_written_by(cutoff)`, `rotate_device_key`; `secrets::vault::{key_id, SWEEP_BATCH, SWEEP_GRACE_MS, RETIRE_GRACE_MS}`. `RetireHold` holds the counter. `meta["mesh_keys"]` entries carry `since_ms` (when the key stopped being current).
7. (V) `link::ResetDevices` (→ `Result<u32, String>`), `document::share::Reseal` (→ `bool`). `keys.rs` hooks: `pairing_keyring(net, joiner)` is sync, `adopt_keyring` → `adopt_pairing(net, from, vault_key, keyring)`, new `after_pairing`, `start`, `tick`; `on_frame` owns its streams (spawned off the `drive` loop).

## Phase 2 engine V (vault-key rotation, D7, D11)

| Item | As built |
|---|---|
| §6.9.4 key id | `write_row`, `migrate_legacy` write `key_id` = the current key's W-29 id. `refresh` opens each row with the key it names (`''` by trial, legacy first); a row under a key not held keeps its previous in-memory value and its id goes to `missing_keys` (W-38); otherwise "waiting for the vault key from the mesh". |
| §6.9.3 mint | `Vault::rotate`: 32 random bytes, epoch = max held + 1, one `BEGIN IMMEDIATE` on `meta` (`mesh_key`, `mesh_key_info`, `mesh_keys`, `rotation`), then the ring swap. A failure leaves the old key current and the record `failed` (in memory). `keys::after_remove` mints for paired + self after the tombstone; grants go out and the sweep starts in a spawned task, so `mesh_remove` returns once the key is stored. |
| §6.9.5 frames | `KeyGrant` pushed to every connected paired peer, carrying every tombstone in the store; the receiver merges them (and closes their connections) before `adopt_key`, answers `KeyAck` on the stream. `KeyEpoch{current, held}` on every connect and again whenever a key becomes current; a receiver whose current orders lower, or whose rows name a key the sender holds, sends `KeyRequest`; any holder grants; an unpaired asker gets `KeyRefused`. One request per (peer, key) per 30 s. |
| §6.9.6 sweep/retire | `Vault::maintain(now)` every 5 s from the secrets plugin: sweeps (64 per transaction, CAS on `key_id`, an in-loop cursor past stuck rows) when a retained key exists and this engine minted the current key or 60 s passed since it became current here; retires a retained key after 10 min with no row naming it, no `''` row opening with it and no hold, under `PRAGMA secure_delete = ON`; `old_destroyed`, phase `done`. |
| §6.9.8 member rule | `keys::tick(now)` every 5 s: a tombstoned engine in `members(current)` rotates at first sight + 30 s + up to 30 s jitter, unless a key without it became current. The legacy key's members are written once by `keys::start` (paired + self). |
| §6.9.10 old builds | a peer connected 5 s without `KeyEpoch` is `old_build` in `RotationState.peers` and `UnpairPreview.old_build`. |
| §6.9.11 blobs | `open_with_mesh_key` tries every held key; the document hub re-seals links on each new rotation key (`share::Reseal` under `retire_hold("document")`). |
| W-36 preview | `names_written_by(cutoff)`: user names and `oauth/<ns>` (as `ns`) with a kept version `updated_ms ≤ cutoff`; never `mesh/*`, `sandbox/*`. `offline`, `old_build` are engine ids; `devices` = active link devices (rpc). |
| D11 | `mesh_remove(id, true)`: after `RemovePeer` (new key stored) and only if the rotation did not fail, `Vault::rotate_device_key` writes a new `mesh/device_key` version, then `link::ResetDevices` revokes every active device and drops the cached mesh identity; other engines' link notices the changed row on `SecretsChanged` and restarts its mesh uplink. |
| Events | every change of the rotation record goes on the bus as `SecretsRotation` (a watch channel in the vault, forwarded by the plugin). |

### Calls made (V)

1. **The device key travels as a vault row**, a new `mesh/device_key` version sealed under the new mesh key, not as a second `KeyGrant`: records replication reaches only paired engines, and the removed one lacks the key to open it. Same reach, no new frame.
2. **W-35 rotation** after a pairing only when the adopter had other engines (`rank.paired_peers > 0`) and sent a keyring (a pre-183 or lone adopter has no one to strand); otherwise every fresh machine's pairing would rotate.
3. **Preview window**: an engine this process never saw connected has no reliable last sighting (`Peer.last_seen_ms` falls back to `added_ms`), so all names are listed, as for a connected one.
4. **Members**: a joiner is added to the members of the keyring copy it receives; a grantor adds a requester to its own copy. The member rule only needs the engines that held the key.
5. **No sweep without a retained key**: the `''` rows of §9.5 are rewritten only inside a rotation or pairing. Pairing sweeps at once (79 F5's single re-seal); grants wait out the 60 s grace unless this engine minted.
6. **Adopters write a rotation record too** (`removed` = old members − new), so the banner shows on every remaining machine.
7. **Key order ties** (two fresh vaults, both `(0, "")`) break on the id.
8. **`mesh_remove(id, true)` after a failed mint** errors with "unpaired, but phone and iPad pairing was not reset": the unpair stands, the device key is not touched under the old mesh key.

### Tests and gates (V2)

- New: secrets `vault::tests::{rotation_reseals_every_row_and_retires_after_the_grace, a_crash_mid_sweep_resumes_and_another_holder_takes_over_after_60_s, two_rotations_at_once_the_higher_key_wins_on_both, rows_under_a_key_not_held_keep_their_last_value_until_the_key_comes, pairing_during_a_rotation_carries_the_retained_keys, a_legacy_row_opens_by_trial_and_holds_the_legacy_key, unpair_preview_names_respect_the_window_and_the_exclusions, rotate_device_key_writes_a_new_value}`; mesh `keys::tests::{unpair_rotates_for_the_remaining_machines_only, an_offline_machine_gets_the_key_on_reconnect_and_never_reads_blank, pairing_during_a_rotation_carries_the_keyring, a_tombstoned_member_rotates_after_the_wait, a_pre_183_peer_is_named_in_the_rotation}` (loopback iroh, a real vault per node); document `share::tests::share_links_reseal_across_a_rotation`; link `follow::tests::reset_devices_revokes_every_device_and_drops_the_mesh_identity`.
- `check` and `clippy --tests --no-deps -- -D warnings` on `-p secrets -p mesh -p document -p rpc -p link -p agent -p theuth`: 0 errors, 0 warnings.
- Touched suites twice on `-p secrets -p mesh -p document -p link -p rpc`: secrets 22, mesh 16, document 4, link 3, both runs green. Full set: secrets 33, mesh 41, link 30, rpc 81 + 4, document 84 + 29, 0 failed (one earlier full run hit `store::tests::concurrent_first_opens_all_succeed` "database is locked" under load; untouched code, passed alone and on the rerun).
- Bindings: `export_bindings` 8 passed, output identical to the UI copy (no TS change); `tsc -b --noEmit` 0 errors.
- Not done here: the §15 scratch-engine proofs (three engines, `t mesh remove`); every step they check is covered in-process above.
- R1 (Nearby, W-35): the Nearby Grant needs no keyring. After both presses the dialer runs `Net::pair_with`, the same `PairHello`/`PairAck` exchange as a pasted string, so the keyring rides `PairAck`, `key_owner` decides, and `KeyEpoch` on connect covers anything newer. No code change; the ceremony is untouched. Test `nearby::tests::nearby_pairing_mid_rotation_carries_the_keyring` (an established, rotated `a` with K0 still retained; a fresh `b` pairs over Nearby and ends on K1 holding K0); nearby + keys suites 11 passed, twice.

## Phase 2 contract (C2)

Skeleton only, on `feat/183-p2` from b23f877f: every shape of §14.3 and §14.7 plus the Phase 2 RPCs of §10.1 compiles, old wire decodes, and behaviour is unchanged. Each place behaviour goes carries a one-line marker: `// 183 P2: N` (messaging, Nearby, pairing, MeshStatus paths), `// 183 P2: V` (vault-key rotation), `// 183 P2: UI` (Settings › Network). `rg -n "183 P2"` lists them; the Phase 1 `// 183 P2:` markers (`peer_of` path/RTT, `Status` relay, supervisor chain and `reachable`) stay N's.

### Migrations

| Store | File | Content |
|---|---|---|
| secrets | `crates/secrets/migrations/0003_key_id.sql` | `vault.key_id TEXT NOT NULL DEFAULT ''` + index `vault_key_id` (§9.5, verbatim) |
| mesh | none | §9 names no Phase 2 mesh migration (0006 was Phase 1) |

0003 was the next free number (newest was 0002). The keyring and epoch metadata (`mesh_key_info`, `mesh_keys`, `rotation`) are keys of the local-only `meta` k/v table (§9.5), so they need no schema change; nothing writes them yet. `store::Row.key_id` (`crates/secrets/src/store.rs:34`) reads the column; nothing writes it yet, so every row stays `''`. Test `store::tests::a_row_without_key_id_reads_as_the_legacy_key`.

### Shapes (file:line at this commit)

| Area | Shape | Where |
|---|---|---|
| Config §9.3 | `MeshConfig.messaging_off: Vec<String>` (`[]`), `.nearby: bool` (`true`), `.pair_window_secs: u64` (`120`): read, not enforced | `crates/mesh/src/config.rs:52`, `:54`, `:56` |
| | `AgentConfig.communication: Option<bool>` (absent = `None`; today `unwrap_or(false)` at the supervisor and autoresearch) | `crates/agent/src/lib.rs:3817` |
| | `CommunicationState.source` documents `mesh` | `crates/agent/src/communication.rs:431` |
| Refusals §11.4 | `agent::MeshRefusal{Off{engine}, Messaging{from, to, on}, NotOwner{engine, from}}` with the exact texts as `Display` | `crates/agent/src/communication.rs:877` |
| Guard §6.5 | `PeerDeliver.forwarder: Option<String>` (serde default); `peer_deliver` fills it from `Forwarded(peer)`; the supervisor ignores it | `crates/agent/src/communication.rs:617` |
| Wire §10.2 | `Msg::PairRequest{name, os, nonce}`, `PairChallenge{nonce}`, `PairAnswer{accept, token}` | `crates/mesh/src/net.rs:107`, `:113`, `:116` |
| | `Msg::KeyEpoch{current, held}`, `KeyRequest{id}`, `KeyGrant{key, removed}`, `KeyAck{id}`, `KeyRefused{id, reason}` (appended; `Msg` is JSON by variant name, so old tags are untouched) | `crates/mesh/src/net.rs:122-136` |
| | `PairHello.keyring`, `PairAck.keyring: Vec<WireKey>` (serde default, skipped when empty) | `crates/mesh/src/net.rs:84`, `:99` |
| | `pairing::Payload.vault_key_id: Option<String>` (always `None` yet) | `crates/mesh/src/pairing.rs:41` |
| Mesh types | `UnpairPreview{id, name, connected, last_seen_ms, secrets, sign_ins, offline, old_build, devices}` | `crates/mesh/src/types.rs:362` |
| | `MeshStatus.messaging_off: Vec<EngineId>`, `MeshStatus.nearby: bool` (from config) | `crates/mesh/src/types.rs:345`, `:348` |
| Mesh actor | `Nearby`, `NearbyWatch(bool)`, `PairNearby(EngineId)`, `PairAnswer{ceremony, accept}`, `SetMessaging{id, on}`, `SetNearby(bool)`, `Rename{id, name}`, `PreviewUnpair(EngineId)`, `RotateVault` | `crates/mesh/src/lib.rs:261-316` |
| | `RemovePeer` handler calls `keys::after_remove` after the tombstone (W-28) | `crates/mesh/src/lib.rs:792` |
| VaultKeys | `keyring()`, `adopt_key(WireKey)`, `rotate(members, removed)` beside `get` and `adopt` | `crates/mesh/src/keys.rs:22` |
| Secrets §14.7 | `KeyInfo`, `WireKey` (no TS; `Debug` omits the key), `RotationPhase`, `RotationPeer`, `RotationState`, `RetireHold` | `crates/secrets/src/vault.rs:108`, `:125`, `:144`, `:160`, `:177`, `:203` |
| | `Vault::rotate`, `adopt_key`, `keyring`, `sweep`, `retire_hold`, `rotation` | `crates/secrets/src/vault.rs:402-437` |
| | actor `Rotate{members, removed}`, `AdoptKey(WireKey)`, `Keyring`, `GetRotation` | `crates/secrets/src/lib.rs:237`, `:247`, `:251`, `:255` |
| | `SecretsEvent::Rotation(RotationState)`, wire `SecretsRotation` (never published yet) | `crates/secrets/src/lib.rs:300` |
| RPC §10.1 | `mesh_remove(id, reset_devices: Option<bool>)` (D11) | `crates/rpc/src/api.rs:2565` (impl `:8789`) |
| | `mesh_nearby`, `mesh_nearby_watch(open)`, `mesh_pair_nearby(id)`, `mesh_pair_answer(ceremony, accept)`, `mesh_set_messaging(id, on)`, `mesh_set_nearby(on)`, `mesh_rename(id, name)`, `mesh_unpair_preview(id)`, `vault_rotation()`, `vault_rotate()` | `crates/rpc/src/api.rs:2571-2600` (impls `:8808-8861`) |

What the stubs answer today: `mesh_nearby` `[]`; `mesh_nearby_watch` `null` (no-op); `mesh_pair_nearby`, `mesh_pair_answer`, `mesh_set_messaging`, `mesh_set_nearby`, `mesh_rename`, `vault_rotate` an `-32000` "… is not built yet (183 phase 2)"; `mesh_unpair_preview` the real `id`, `name`, `connected`, `last_seen_ms` with empty lists and `devices: 0`; `vault_rotation` `null`; `mesh_remove(id, true)` refuses before anything is removed. A stranger's `PairRequest` is closed like any stranger without a token. No new frame is sent, and received key frames are logged at debug and ignored, as an older engine does.

### Module split (parallel N and V)

| Module | Owner | Functions | Called from |
|---|---|---|---|
| `crates/mesh/src/keys.rs` | V | `VaultKeys` and `SecretsKeys` (moved from `net.rs`, re-exported as `mesh::net::{VaultKeys, SecretsKeys}`), `pairing_keyring`, `adopt_keyring`, `on_connect`, `on_frame`, `after_remove`, `rotate`, `unpair_preview` | `net.rs` `add_peer` (send + adopt keyring), `accept_control` (adopt + send keyring), `drive` (`on_connect` after `Peers`; one match arm for the five key frames); `lib.rs` `RemovePeer`, `RotateVault`, `PreviewUnpair` |
| `crates/mesh/src/nearby.rs` | N | `watch`, `list`, `pair`, `answer`, `on_pair_request` | `lib.rs` `Nearby`, `NearbyWatch`, `PairNearby`, `PairAnswer`; `net.rs` `accept_control` (a stranger's first frame `PairRequest`) |

`Net.store` is `pub(crate)` for them. N keeps `lib.rs`'s `SetMessaging`/`SetNearby`/`Rename` handlers and `net.rs` `peer_of`; V's later `net.rs` diff is the bodies behind the hooks above plus the pairing-string `vault_key_id`. Each adds its own state field to `Net` (an unread field would warn now).

### Deviations from §14, and calls made

1. **`key_id` `''`, not NULL.** The brief says "NULL key_id meaning the current key"; §9.5 freezes `TEXT NOT NULL DEFAULT ''` = the legacy key, which is the current key until the first rotation. §9.5 wins: same meaning for old rows, and the replicated column never carries NULL.
2. **`VaultKeys::adopt` and `secrets::AdoptMeshKey` stay** beside `adopt_key`/`AdoptKey`. Today's pairing still moves the single key through them; removing them now would change behaviour. V removes both when `adopt_key` carries pairing (§14.7 "replaces").
3. **`RetireHold` has a private field**, not the unit struct of §14.7, so V can add the hold count without a public change. It stays opaque either way.
4. **`Msg::PairAnswer` gains `token: Option<String>`.** §10.2 has `PairAnswer{accept}`, but §6.8 runs "the existing `PairHello` path with a freshly minted token", which the dialer must learn: the acceptor's accept carries it. `PairRequest` follows §10.2 (`{name, os, nonce}`), not §6.8 (`{name, nonce_a}`); `nonce` is 16 random bytes, base32.
5. **D11 is a trailing `reset_devices: Option<bool>` on `mesh_remove`**, not a new `mesh_unpair` RPC or request struct. `t mesh remove`, sandbox Destroy and older clients keep calling `mesh_remove(id)`, and `RemovePeer(EngineId)` stays unchanged as §14.7 says. The device key is reset in the RPC after `RemovePeer` returns, which is after the new vault key is stored, so the order "device key after vault key" holds without threading the flag through the actor. `true` is refused for now.
6. **Additions not in §14**: `MeshStatus.messaging_off` and `MeshStatus.nearby` (the two Network switches need their state; §6.7 gives no source), `mesh_set_nearby(on)` (§6.7's "Let nearby machines find this one" switch writes `mesh.nearby`; §10.1 has no setter), `PeerDeliver.forwarder` (the §6.5 guard runs in the supervisor, which never sees the RPC extension), `MeshRefusal` (§11.4's texts as one type).
7. **`MeshRefusal::Messaging{from, to, on}`.** §6.2 writes `between <self> and <peer> … on <self>`, §11.4's example `between cloud-1 and xeus is off on xeus` puts self second. Sender first, receiver second, `on` = whose list refused: both texts come out right.
8. **`vault_rotate` goes through the mesh actor** (`RotateVault`) because `members` is the paired set; `vault_rotation` asks secrets' `GetRotation` directly.
9. **No `key_id(key)` helper yet.** The W-29 id needs `sha2` in `crates/secrets/Cargo.toml`, which §15 gives to V.
10. **`UnpairPreview.devices` is 0** until V asks link; mesh cannot name link.
11. **`AgentConfig.communication` skips serializing `None`**, so nothing ever writes `communication: null`.
12. **TUI** gets a no-op `SecretsEvent::Rotation` arm (`crates/tui/src/model.rs`).

### Bindings the UI worker imports

New: `UnpairPreview.ts`, `KeyInfo.ts`, `RotationPhase.ts`, `RotationPeer.ts`, `RotationState.ts`. Changed: `MeshStatus.ts` (`messaging_off`, `nearby`), `SecretsEvent.ts` (`SecretsRotation`), `CommunicationState.ts` (doc), `methods.json` (+10). Hand-kept `crates/desktop/ui/src/rpc/api.ts`: the 10 new methods, `mesh_remove(id, reset_devices?)`, and `METHODS`. UI edits forced by the types (each `// 183 P2: UI`): `components/tasks/mock.ts` `MESH` gets `messaging_off: []`, `nearby: true`.

### Gates

- `check` and `clippy --tests --no-deps -- -D warnings` on `-p mesh -p secrets -p agent -p rpc -p roster -p document -p theuth -p sandbox -p autoresearch -p tui`: 0 errors, 0 warnings.
- `mbx test -p secrets -p mesh -p rpc -p agent --no-fail-fast`: secrets 25, mesh 30, rpc 81 (+ 4 integration), all passed. agent 792 passed, 4 failed, all in the Python code runtime and none in code C2 touched: `sensor::tests::{fs_exists_through_monty, monty_budgets_and_allowlist, pr_watch_type_checks}` ("monty worker crashed: unsupported protocol version 6 (server supports protocol version 5)", a worker binary from another build in this environment) and `loop_::runtime_bridge_tests::guessed_direct_calls_are_bridged_or_refused` (needs the same runtime).
- Bindings: `export_bindings` 8 passed; `tsc -b --noEmit` 0 errors; vitest `src/rpc/api.test.ts` + `src/components/tasks`: 5 files, 33 tests passed.

## Phase 1 review round R2 (Z183-R2)

UI re-review approved; the engine re-review had one must-fix. Fixed in 184f7bb5 and a follow-up; evidence `/tmp/rs183/r2-evidence.txt`, screenshots `/tmp/rs183/shots/r2/`.

| Item | Fix |
|---|---|
| R2-M1 half-drawn reply after a follow loss | The slice keeps `following` from `FollowState`. A replicated `ContextChanged` is swallowed (and a settled page refused) over an open reply only while `following` is true. `FollowState{live:false}` (or a peer disconnect) marks the slice stale and requests a settled reload (`theuth:reload-session`), and the stream coming back up retries a settle a still-open turn refused. Reducer test: open tail, follow lost, replicated change → stale → the settled page installs the full reply. Live: B killed mid-reply and restarted (A untouched, no refocus); A showed the full resumed reply, `tick19`. |
| 1 owner change to here | The aborted follower's relayed cards are forgotten and its live flag cleared (with a `FollowState` false). |
| 2 seat at spawn | An actor spawn seats the session at `sessions.cwd` when it exists and no seat is set, so a restart keeps a chosen folder and its prompt. |
| 3 `SESSIONS_WIRE_SCHEMA` | `agent::store::SESSIONS_WIRE_SCHEMA = 53` replaces the newest-migration number in `Hello`/`Release`. **Bump rule** (also on the constant): bump it to a migration's number exactly when that migration adds, drops or renames a column of a replicated session table; a migration touching only local tables leaves it. |
| 4 H1 | Done in R1 (94ce476f). |
| 5a replayed prompts | The owner sends the cards already pending at `Watch` with `seq` 0 (as `Watching`); the viewer adds `replay: true` to their JSON and the UI states no "denies at" for them. Test `a_card_pending_before_the_follow_is_flagged_as_a_replay`; live: after a reload, the replayed card had no deadline. |
| 5b `routed()` | MemoryPanel zoom and recall, Peers answer, Communication session and scope setters (the global switch is this engine's), SessionList bulk delete and archive (per session). |
| 5c tests | Header approvals pill (`lib/header.ts` `approvalsOff`), vanished machine target, Composer NotOwner retry (`retryNotOwner`). |
| Live extra | After B's crash-resume finished, A kept "Running on b": a replica's page with the owner's last report (`resumed: true`, no open turn) now shows no banner. Test in `reduce.test.ts`. |

## Phase 1 review round R1 (Z183-R1)

Both reviews (`/tmp/rs183/review-{engine,ui}-p1.md`) returned CHANGES. Fixed in 2a78d330 and its follow-ups (94ce476f, 470c6e31 for H1–H3); evidence `/tmp/rs183/r1-evidence.txt`, `/tmp/rs183/r1-mixed-87f4.txt` (the mixed run against a `t` built from 87f4acf0 by `git archive` + `mbx build`, binary `/tmp/rs183/t-base-87f4acf0`; an earlier run against 182's `t-base` 8316913d is `/tmp/rs183/r1-mixed.txt`), screenshots `/tmp/rs183/shots/r1/`.

| Item | Fix |
|---|---|
| E-M1 relay blind after reconnect | The owner no longer filters on `from_seq`: nothing is buffered to replay, and its per-session counter restarts with the `served` entry or the process. The viewer starts each stream at 0. Test `a_reconnected_viewer_still_gets_events_and_cards` (drop on the owner, reconnect, new delta and card arrive). Live P3b: A restarted mid-turn, deltas resumed, a new card arrived in 380 ms. |
| E-M2 routine pin outlives its file | Every fire applies the file's `approvals:`: `false` pins off, `true` clears the pin (`schedule::apply_approvals`). Test `approvals_true_fire_clears_the_pin_a_false_fire_left`; live P4b: row `off` after a false fire, `NULL` and a card after a true one. |
| E-M3 mixed versions | The agent schema version (`Store::schema_version()`, the newest migration, 53) rides as a postcard trailer after `Hello` and `Release`. Old decoders ignore trailing bytes (postcard 1.1.3 `from_bytes`); a frame without it decodes as `schema: None`. A peer with no or an older version gets no log rows, relays or snapshots, and its cursor does not move: `sessions: holding log for <peer>: its agent schema unknown < ours 53`. Its `Release` is refused by closing the stream without `Released` (an old taker's `release_from` fails, so an orphan/write take does not happen; an explicit old "Continue here" still takes without the tail, as for an unreachable owner). Skipped tables on apply (log and snapshot) warn `apply SKIPPED table …`. Tests in `types`, `sync`, `owner`. Live: A new, B `t-base`: B got 0 of 8 messages and A logged the hold; B upgraded, restarted, and both sessions with all 8 messages arrived. B's record replica (mesh stats, not sessions) logged `engine_stats.os is not a replicated column here` while old. |
| U-M1 `pin: null` | Done in pass 1 on the engine (`pin` omitted when `None`, 4498d19c); the UI also treats `null` as inherit now, with `pin: null` tests. |
| S1 opt-out row ↔ policy | `mirror` (so `GetCommunication` and every reader) and actor spawn apply `sessions.communication` both ways (`sync_opt_out`). |
| S2 fork opt-out | `ForkSession` writes `communication = 0` on the fork when `record_fork` opts it out; the row sync above would otherwise undo it. |
| S4 cell kernels | The owner's relay drops `CELL_PREFIX` kernel jobs (tracks `Started`), so a mid-run follower never shows one as a terminal. |
| S5 moved row | `append_moved` decides `in_context` inside its own transaction (open turn → context-excluded). |
| S6 `resume_turn` | Dead `from = owner_peer()` removed, doc corrected; `turn_openers` doc names the moved record. |
| S7 `get_approvals` on an old owner | Deviation: no engine fallback. The row's pin alone cannot give the effective value (it needs the owner's `tools.approvals`), so the UI reads `-32601` as "<owner> needs an update" and greys the row. |
| S8 remote running | `mark_remote_running` counts an open remote turn only while its owner is in `mesh::connected_ids()`. |
| UI 2–10 + tests | See the UI agent's list below; tsc 0, full vitest green. |
| Chip "live" | New client event `FollowState{session, live}` from the viewer's relay: `live` once the owner answered `Watch`, `false` when the stream ends; re-announced to a second client. The chip reads `on b · live` only then, `on b` when connected but not followed (old owner), `on b · offline` otherwise. |
| H1 old owner's pending prompt after a move | `release()` drains the session, then tells the approval and ask actors `ForgetSession`: the pending entries go (their waiters publish `*Resolved`, so cards clear and a reload's snapshot no longer lists them) and the `questions` rows stay `pending` for the taker's re-ask. Test `forget_session_drops_the_card_and_keeps_the_row_pending`; live: after a mid-turn move and a reload, one card on each machine. |
| H2 job notes name the machine | `communication::mesh::machine_name` (mesh name, else "another machine") in the interrupted-job note, the lost `wait` reply and the transferred job's `owner changed (ran on …)` body. Unit tests updated (no mesh in tests → "another machine"). |
| H3 banner after a move | A transfer reads `Moved here from <machine> at HH:MM — resumed here`; `Taken from <machine>` stays for a takeover. `Loaded.resume` gains `from` (the latest moved record's `from_id`) so the name survives a reload. Live: `Moved here from a at 08:20 PM — resumed here`. |

UI changes (UI R1): `withdraw` on the approvals/asks stores and `relayGuards` in `App.tsx` (peer disconnect, owner change, unfocus of a remote chat, popouts too); "denies at" only for cards that arrived live (`arrived` map, `firstSeen` gone); `-32013` keeps the draft and retries once after `SessionOwnerChanged`; `-32014` reads "live output on <machine>" for terminals and cells; `routed()` for the model/effort picker, Rename, sidebar row actions, queued withdraw, regenerate/edit, remove_context, Memory panel, Settings › Approvals, and no "Stopped N agents" toast for a parked stop; "<owner> default (on|off)" from the effective value; NewChatPane `pickMachine` drops a foreign model pick, clears a vanished target, aria-labels carry values, Browse debounced with Enter accepting; parked calls clear on peer connected; container-width short placeholder (`useNarrow`); a replica no longer shows a stray "✓" line for a relayed `ToolCallFinished` it never saw start.

Observed, not fixed: after a viewer restart mid-turn, A showed only the deltas since the reconnect until `TurnFinished` settled the full row (P3b). The UI's first page brings the replica's partial, which the next delta replaces as for a local resume.

Gates (R1, final 470c6e31): `check` and `clippy --tests --no-deps -- -D warnings` on agent, rpc, sessionsync, mesh, tools, platform, tui, cells, schedule, replica: 0 / 0. Touched suites twice, both runs green. Full suites (worker env) on agent, rpc, sessionsync, mesh, tools, schedule: agent 795 passed, 1 failed (`capping_binds_out_name`, known); rpc 81, sessionsync 42 + 2, mesh 29, tools 218, schedule 82, 0 failed (an earlier full run also missed `sensor::tests::monty_budgets_and_allowlist`'s 5 s bound under load; it passed alone twice). Bindings 8 passed; tsc 0; vitest 315 files / 2046 tests passed.

## Phase 1 as built + live proof (Z183)

**Merge and fold.** `feat/183-p1-ui` (c44ba763) merged with no conflict; it touched only `crates/desktop/ui/` and `183-ui-notes.md`. `origin/main` folded: three docs-only commits (3d9a4bd5, c5999547, e78ef26b) on `183-remote-sessions-and-a2a.md` and `183-rulings.md`; no code overlap.

### Reconcile: U's assumptions against E's contract and the engine

| U assumption | Finding | Done |
|---|---|---|
| Approvals tri-state reads `ApprovalsState.pin` (`undefined` = machine default) | The wire sent `"pin": null` (serde default, no skip) while TS says `pin?`, so a session without a pin read **Off**. And `pin` came from the registry's memory, filled only when the session's actor spawns: a session idle since a restart or a move read "inherit" though its row said `on`. | `pin` (and `MeshStatus.relay`) skip `None`. `get_approvals` re-applies the row's pin (`RestoreApprovals`) before reading; `SetApprovals` now awaits its journal write, so that re-read never undoes a fresh click; actor spawn restores `None` too, dropping a pin this registry kept from before a move away. Test: `tools` `session_pin_is_journaled_and_restore_is_not` (written before the reply). |
| Relayed card deadline = first seen + 300 s | The owner's deadline is not on `ApprovalRequested`; adding it touches ~20 struct literals in 6 crates. | Kept U's ponytail (an owner with another `approval_timeout_secs` shows the wrong time). |
| "live" = owner connected | The engine exposes no follow-stream state to the UI. The open chat is focused, so it is followed whenever the owner is connected; P3 shows the stream under `on b · live`. | Kept. |
| Header chip `approvals off` | Read the engine-wide `Info.approvals`: a chat pinned On under `tools.approvals: false` (or any chat owned elsewhere) showed this machine's switch. | `Header.tsx` reads `get_approvals(session)` (keyed on this engine's switch too). |
| ⋯ Effort (U skipped) | §12.2 has it. | Built: Effort ▸ default + the model's levels, `set_effort` through the owner, greyed offline or when the model has no levels. |

### Closer items

- **(a)** The placeholder takes its short form under `md` (768 px, the sidebar breakpoint) as well as on touch, and the machine name never breaks: no-break spaces and a word joiner after each hyphen. At 430 px: `Runs on b` (`a-430-remote-chat-placeholder.png`).
- **(b)** With the owner offline every setter greys (Approvals, Agent messaging, Model, Effort) and Continue here stays enabled (`p2-a-offline-menu.png`). Move to shows "<owner> is offline — Continue here first" when it shows at all (two engines: the only target is the owner, so it is absent).
- **(c)** `resume_calls::resume_job_call` and the lost `wait` reply said "the engine was stopped … and restarted" after a transfer whose job row named no other engine; any transfer or takeover now says the session moved (`job_note_after_a_move_says_moved_without_an_engine_on_the_row`). Live, the mid-call move found the shell already promoted to a job (within ~1 s), so the interrupted path was not hit; the unit test covers it.
- **(d)** Built the sidebar Chat row's **▸** menu ("New chat on" each machine, only with paired peers) and the ⌘K empty-query group "New chat on <machine>" (`openNewChatOn` in `NewChatPane.tsx`), plus Effort ›. Nothing skipped.
- **(e)** `▶` → `►` in `crates/rpc/src/api.rs` (`cell_run`) and in the same cells sentence in `crates/cells/src/{types,actor}.rs`; the UI (`NoteCell.tsx`, `rpc/api.ts`) already said `►`.
- **(f)** Not real. `partial_upsert` runs only inside an open turn's stream; `remove_context` refuses unless the session is idle (`BUSY`), and the title job captures its revision at the end of a turn, after which the next turn opens with a user-row append that already bumps the revision. A partial therefore never invalidates a revision that an append would not. No change.
- **(g)** Covered live: a forwarded `create_session` owned by the target from epoch 1 (P6), a forwarded setter changing the owner's row (`sessions.model = fake/crawler-2` and `approvals = on` on B, P1), a pin that survives two real moves (P5: `approvals kept: Ask` both ways; B's `get_approvals` read `pin: true` right after the idle move, before any turn ran there). No rpc test: the forward tests pair transports, not engines with stores and registries.

### Gates

- `check` and `clippy --tests --no-deps -- -D warnings` on `-p agent -p rpc -p sessionsync -p mesh -p tools -p platform -p tui -p cells`: 0 errors, 0 warnings.
- Touched suites twice (same set, filter `session_pin job_note a_chat_created notes_spell_the_cause approvals pin forward`): 14 + 12 + 4 passed, both runs.
- Full `agent` with the worker env: 794 passed, 2 failed: `capping_binds_out_name` (known) and `talk_ending_releases_parked_items` (load-sensitive: passes alone twice). Full `rpc` 81, `sessionsync` 36 + 2, `mesh` 29, `tools` 217 passed, 0 failed.
- `tui` `no_emoji_in_desktop_chrome_strings`: passed. Bindings regenerated (8 passed); `tsc -b --noEmit` 0 errors; vitest 315 files / 2027 tests passed.

### Live proof (two scratch engines, loopback)

Rig `/tmp/rs183/rig/` (from `/tmp/taskhome`): A rpc 18561 / mesh 18661 / fake 18461, B 18562 / 18662 / 18462; `setup.sh` turns relay, discovery and every outside plugin off; B paired with `mesh_pairing_string` on A → `mesh_add`. `fake.py` streams `SLOW` over 20 s, answers `FAKE-SHELL` / `FAKE-SLEEP` with a `shell` call, and logs each request's `Machine:` / `Working directory:` lines and any `<session_moved/>` or `<engine_restart/>` it sees. vite dev on 18587 (A) and 18588 (B), headless Chromium on 127.0.0.1 only. Evidence `/tmp/rs183/evidence.txt`; screenshots `/tmp/rs183/shots/*.png` (all PNG).

| Proof | Result |
|---|---|
| P1 forward without take | PASS: send, approve, set_model and set_approvals from A; owner `b` epoch 1 throughout; B's fake ran each turn |
| P2 offline card | PASS: card 35 ms after Enter, nothing ran; Continue here → epoch 5 on A, the parked send ran on A |
| P3 live stream | PASS after the fix below; sidebar `Thinking` glyph on an unfocused B chat |
| P4 relayed approval | PASS: `b asks: shell` / `destructive · relayed from b · denies at …`; Allow on A ran on B; Deny → `denied by user`; answered on B → A's card gone in 88 ms |
| P5 moved card | PASS after the fixes below: MovedCard on both; B's next request carried `<session_moved to="b" from="a" os="macOS" … cwd=… approvals="ask"/>`; mid-turn move B→A: EngineRestartCard `Resumed on a after 9 s · moved from b · 1 call replayed`, 0 raw tags in either DOM |
| P6 new chat on B | PASS after the fix below: sidebar ▸ / ⌘K / New chat on [b ▾], Browse b… (forwarded completion), owner `b` epoch 1, `Machine: b (macOS, aarch64)`, `Working directory:` the chosen folder; a missing folder is refused `… does not exist on b` |
| P7 phone | Not driven: devices need the link plugin, which the network rule keeps off. Their mode-0 records enter the same Methods (`crates/link/src/conn.rs:796`), so the P1 front door and E's `classify_forwards_to_a_connected_owner_and_is_offline_otherwise` cover it |

### Defects found live and fixed

1. **A followed reply restarted every 5 s** (P3). Each replicated partial raised `ContextChanged{replicated}`; the settle swapped the live text for the greyed partial, and the next delta began a new reply. A settle retry could also drop the first delta. Fix: a replicated `ContextChanged` leaves the slice alone while an assistant text is open, and a settled page is refused while the replica reports the owner's turn open (`resume.here: false`) over an open text (`openTail`, `reduce.ts` / `sessions.ts`). The owner's `TurnFinished` settles. Tests in `reduce.test.ts`.
2. **"Running on b" never cleared** on a replica after the owner's turn ended (until a reload): `resumeFromLoaded` drops a replica's banner when the page has no resume.
3. **A mid-turn move's `moved` row rendered as a "Removed from context" tombstone** (it is context-excluded by design); `Message.tsx` renders it as its MovedCard.
4. **Two approval cards after a mid-turn move**, on both machines: the suspended turn's card (still parked in the old owner's approval actor) beside the new owner's re-ask. `App.tsx` withdraws a session's cards on `SessionOwnerChanged` (they precede the re-ask on both sides). The snapshot half is a HANDOFF.
5. **The moved banner said "Sending a message here brings it back"**, false since D1: now "…it runs there; what you send here runs there too." (wording test deleted).
6. **A new chat's folder was only recorded**: the session ran in the engine's own cwd. `new_root` seats a created root at its `cwd` when it exists there (`a_chat_created_in_a_folder_is_seated_there`).
7. **Header approvals chip** (reconcile table).

### HANDOFF

- *(fixed in R1, H1)* **The old owner keeps a moved turn's approval or ask pending** in its actor until `approval_timeout_secs`, and its `PendingPrompts` snapshot lists it: after a reload B showed its own dead card beside A's relayed re-ask. Shape: at release, drop the session's pending prompts in `Approval` / `Ask` (the waiter then publishes `ApprovalResolved`) without settling the `questions` row, which the taker re-asks from.
- *(fixed in R1, H2)* Interrupted-job notes and the job card name the origin by engine id (`moved here from 91f6…`, `owner changed (ran on 91f680ca…)`); §11 wants mesh names, which `resume_calls` cannot see today.
- *(fixed in R1, H3)* The taker's banner after a transfer reads `Taken from another engine … resumed here`: `TurnResumed.from` is not reaching the replica banner (139 text).
- In headless Chromium a Puppeteer mouse click on a Radix sub-menu item did not select it (the live proof used DOM clicks). [INFERENCE] harness-only; not seen with a real pointer.
- Pre-existing (E): three `tools` registry tests share the global `jobs` actor name.

## Contract changes after C (Phase 1 engine)

No TypeScript shape changed. The UI worker should know two meaning changes:

1. **`ApprovalsState.session` is now the effective value for the session**: its own or nearest ancestor's pin, else `tools.approvals`. Before 183 it was "the session did not opt out", which was `true` even with the global switch off. `pin` is the row's own `sessions.approvals`. A ⋯ switch that toggles `!state.session` therefore pins on with the global switch off (ask 7). The doc comment changed, so the regenerated `ApprovalsState.ts` comment differs.
2. **`set_approvals(session, true)` now pins `'on'`** instead of clearing the opt-out. `clear: true` returns to inherit (the "follow cloud-1 default" item).

Wire (engine-internal, no TS): an owner answers `Msg::Watch` with a first `Msg::Bus{kind: "Watching", seq: 0, bytes: []}`. Then it sends the session's approvals and asks that are already pending, so a focus after the ask still shows the card. A viewer that sent `Watch` and saw no frame at all treats the owner as pre-183 and uses `Events` until that owner reconnects.

## Phase 1 engine notes (E183)

Calls made where the design left room:

- **Front door.** `forward::route` forwards for every row owned by a connected engine (user sessions and desks), answers `-32010` when the owner is not connected, and never takes. A `Forwarded` call for a row owned elsewhere is `-32013 NotOwner` (`forward::admit_forwarded`). `session_move` on a viewer forwards to the owner, which makes its own `session_take` call to the target (a fresh extension set, so it is not refused as a second hop). `cell_attach` on a run forwarded to its owner, and `subscribe_pty` for a job of a session owned elsewhere, answer `-32014 RunsElsewhere`.
- **Relay.** A Bus frame is re-published locally as `sessionsync::events::Relayed`, and only the RPC bridge subscribes to it. The viewer's telegram, slack, signal, double and attention actors therefore never act on another engine's prompt. Relayed `JobEvent`s pass through the bridge's cell-kernel filter. `CellEvent::KernelChanged` names no session and is not relayed. Relayed cards are dropped when the follow stream ends or on unfollow. No synthetic `*Resolved` is published, so the UI withdraws them through its chip state or the next snapshot.
- **Focus follows every focused session.** The relay's follower returns at once for a row owned here, and starts following if the session later moves away.
- **`resolve_approval` / `answer_ask`** fall back to the relayed card's session when the `questions` row has not replicated yet.
- **`create_session{engine}`** forwards with `engine` cleared, then waits up to `sessions.release_wait_secs` for the row to replicate. A forwarded create whose `cwd` is not a directory on the target is refused: "`<cwd>` does not exist on `<machine>`".
- **`list_models(engine)` / `path_complete(None, …, engine)`** forward to that engine. A failed remote completion returns an empty completion, never this machine's paths.
- **Settings.** `ToolRegistry.bypass` became `pin: HashMap<SessionId, bool>`, and the nearest pin up the ancestor chain wins. The registry persists through a new `tools::Journal::approvals` method (default no-op), and the row's pin is applied at actor spawn by the new `tools::RestoreApprovals`, which does not persist. Communication: an opt-out is stored as `0` and an opt-in as NULL. `Store::session` (which returns the wire `SessionSummary`) is unchanged. `Store::session_settings(id)` reads the two columns.
- **Moved notice.** It is written by `Take::take` after `set_owner`, once per real ownership change. It goes in context only when no turn is open. With an open turn the row is UI-only, the model reads the extended `<engine_restart … to= os= cwd=/>`, and `from` is now the mesh name. A take that leaves the turn pending writes `pending: true` and replaces the old local "taken from" note. `runtime = reset` when the session has any `python` tool call. With no pin, `approvals` comes from the taker's `tools.approvals`. `seat::land` seats the taken session at its recorded cwd, or at the home directory when that is missing (W-9), and `sessions.cwd` is not rewritten.
- **Machine.** `platform::process::machine()` takes its name from the mesh (`set_machine_name` at mesh start), else the hostname. The first read fixes it for the process.
- **Partial redraw.** `partial_upsert` bumps `context_revision`. Risk: a title job or a `remove_context` that captured the revision before a 5 s checkpoint of the *next* turn is discarded as stale, as for any other mutation.
- **Phase 2 markers.** The supervisor's `PeerEnvelope.chain` and `PeerInfo.reachable` markers were labelled `183 P1: ENGINE` but belong to §6.5/§6.6. They are relabelled `183 P2:` and left alone.
- **Pre-existing test isolation bug (not C's).** Three `tools` registry tests (`slow_call_is_promoted_to_a_job`, `registration_adopts_a_running_untracked_invocation`, `lifecycle_barrier_waits_for_expected_and_collects_acks`) fail when run concurrently and pass with `--test-threads=1`. Cause: `slow_call` registers a live `Jobs` actor under the process-global name `jobs` (`registry.rs` test), and the other two do a global lookup of that name. C's diff touches none of this. The same failures reproduce at the parent commit 87f4acf0 (built from `git archive`).

## Contract commit (Phase 1)

Skeleton only: every shape of §14.1–§14.3 compiles and decodes old wire; no behaviour changes. Each place behaviour goes carries a one-line `// 183 P1: ENGINE` (or `// 183 P2:` for a Phase 2 field that ships as a shape now).

### Migrations

| Store | File | Content |
|---|---|---|
| agent | `crates/agent/migrations/0053_remote_sessions.sql` | `sessions.approvals TEXT NULL CHECK IN ('on','off')`, `sessions.communication INTEGER NULL CHECK IN (0,1)`, `messages.moved TEXT NULL` |
| mesh | `crates/mesh/migrations/0006_engine_os.sql` | `engine_stats.{os, os_name, arch, host} TEXT NOT NULL DEFAULT ''` |

§9's guessed numbers were free in this tree (agent newest was 0052, mesh 0005). Every column is nullable or defaulted. Nothing writes the session columns yet (NULL); `engine_stats` writes and reads the four fields (`Store::put_stats` / `Store::stats`), empty until the engine fills `stats::probe`. Tests: `agent::store::tests::remote_session_columns_default_null_and_check_values`, `mesh::store::tests::tables_attach_to_replication` (round-trip).

### Shapes (file:line at this commit)

| Area | Shape | Where |
|---|---|---|
| Front door | `NOT_OWNER = -32013`, `RUNS_ELSEWHERE = -32014` | `crates/rpc/src/forward.rs:53`, `:57` |
| | `FrontDoorError::NotOwner{owner}`, `::RunsElsewhere{owner}` | `crates/rpc/src/forward.rs:92`, `:97` |
| | `Route::Offline(PeerId)` (beside `Here`, `Forward`, and the old `Take`) | `crates/rpc/src/forward.rs:114` |
| | `classify(store, session, connected: &[PeerId])` | `crates/rpc/src/forward.rs:205` |
| Relay | `Msg::Watch{session, from_seq}` tag 11, `Msg::Bus{session, seq, kind, bytes}` tag 12 | `crates/sessionsync/src/types.rs:140`, `:148`, tags `:176-177` |
| | owner answers `Watch` as unknown (bails, stream closes: the old-owner fallback) | `crates/sessionsync/src/lib.rs:193` |
| | `events::RelayedPrompts{approvals, asks}`, `events::relayed_prompts()` (empty) | `crates/sessionsync/src/events.rs:102`, `:110` |
| Approvals | `SetApprovals.clear: bool` (serde default) | `crates/tools/src/registry.rs:450` |
| | `ApprovalsState.pin: Option<bool>` (serde default; always `None` yet) | `crates/tools/src/registry.rs:782` |
| Moved notice | `agent::SessionMoved` | `crates/agent/src/lib.rs:1821` |
| | `HistoryEntry::Moved{moved, context}` (role `moved`; never produced) | `crates/agent/src/lib.rs:3081` |
| New chat | `CreateSessionOptions.engine: Option<String>` (ignored) | `crates/agent/src/lifecycle.rs:74` |
| Messaging (P2 fields) | `PeerEnvelope.chain: u32` (serde default 0) | `crates/agent/src/communication.rs:599` |
| | `PeerInfo.reachable: Option<bool>` (skipped when `None`) | `crates/agent/src/communication.rs:406` |
| Mesh | `EngineStats.{os, os_name, arch, host}` | `crates/mesh/src/types.rs:215-224` |
| | `Peer.path: PeerPath`, `Peer.rtt_ms` (P2; `Offline`/`None` yet) | `crates/mesh/src/types.rs:110`, `:113` |
| | `PeerPath{Lan, Relay, Offline}` (lowercase, default `Offline`) | `crates/mesh/src/types.rs:122` |
| | `RelayState`, `NearbyEngine`, `PairCeremony` | `crates/mesh/src/types.rs:135`, `:149`, `:166` |
| | `MeshEvent::Nearby(Vec<NearbyEngine>)`, `MeshEvent::Pair(PairCeremony)` | `crates/mesh/src/types.rs:289`, `:291` |
| | `MeshStatus.stats` (empty yet), `MeshStatus.relay` (`None` yet) | `crates/mesh/src/types.rs:337`, `:341` |
| RPC | `path_complete(session, input, cursor, engine: Option<String>)` | `crates/rpc/src/api.rs:215` (impl `:4634`) |
| | `list_models(engine: Option<String>)`, now `with_extensions` | `crates/rpc/src/api.rs:726` (impl `:5091`) |
| | `set_approvals(session, enabled, clear: Option<bool>)` | `crates/rpc/src/api.rs:849` (impl `:5345`) |
| | `get_approvals(session: Option<SessionId>) → ApprovalsState` (new; local answer) | `crates/rpc/src/api.rs:858` (impl `:5370`) |

Callers updated for the new fields and params: `tui` (client calls, `HistoryEntry` arms), `schedule`, `signal`, `slack`, `telegram` (`SetApprovals`), `roster`, `sandbox`, `double`, `tasks` (test literals of `Peer`, `EngineStats`, `MeshStatus`), `rpc/src/acp.rs` (`MeshEvent` and `HistoryEntry` arms). New deps: `mesh → uuid` (`PairCeremony.id`), `sessionsync → tools` (`RelayedPrompts`).

### Deviations from §14, and calls made

1. **`Route::Take` stays** until the engine worker lands §4.1. Removing it now would change behaviour (D1 forward-not-take). `classify` takes `connected` and ignores it; `route` passes `&[]`; the `Offline` arm already answers `-32010`.
2. **`relayed_prompts()` takes no session and returns `sessionsync::events::RelayedPrompts`**, not `rpc_types::PendingPrompts`. No `rpc_types` crate exists; `PendingPrompts` lives in `rpc`, which depends on `sessionsync`, so returning it would be a cycle. §5.1 already calls it with no argument, and `PendingPrompts::collect` has no session in hand (it filters through `keep`).
3. **`ApprovalResolved` / `AskResolved` are unchanged.** The brief mentions a session id on `ApprovalResolved`. The design does not have one: §10.4 says the shapes are unchanged, and §5.1 keeps `prompts: HashMap<Uuid, SessionId>` in the owner's relay for exactly this reason. Adding the field would also touch 13 constructors across 6 crates.
4. **`MeshStatus` loses `Eq`.** `stats: Vec<EngineStats>` holds an `f32`. No caller needed `Eq`.
5. **TS optionality.** The new `Option` fields are `ts(optional)`: `ApprovalsState.pin?`, `CreateSessionOptions.engine?`, `MeshStatus.relay?`, `PeerInfo.reachable?`. Existing TS literals keep compiling, and the trailing RPC params are optional in `rpc/api.ts`. `PeerEnvelope.chain` is a plain `number` because ts-rs allows `optional` only on `Option`.
6. **`PeerInfo.reachable` is `skip_serializing_if = None`.** `discover_peers` output, which is model-facing, stays byte-identical until the engine fills it.
7. **`get_approvals` offline.** W-5 says the offline reader answers with `global: null`, but frozen §14.2 has `global: bool`. §14.2 wins. Call: when the owner is offline, `get_approvals` returns `-32010` like the setters, and the ⋯ menu renders its disabled offline line (§4.4, §12.2). The pin is read again after Continue here.
8. **`set_approvals` forwards `(session, enabled)` only.** `clear` is applied locally, and is not yet forwarded to the owner. The engine worker forwards all three params once owners take them.
9. **`platform::process::Machine` / `machine()` is not added here.** It is in §14.4, outside this contract's §14.1–§14.3. It is engine-internal (no TS), so the P1 engine worker adds it verbatim from §14.4 with §5.4.
10. **Phase 2 fields ship as shapes now**, because they are in §14.2–§14.3: `Peer.path` / `rtt_ms`, `RelayState`, `NearbyEngine`, `PairCeremony`, the `MeshEvent` variants, `PeerEnvelope.chain` and `PeerInfo.reachable`. They are marked `// 183 P2:` where they are filled.
11. **The TUI and ACP need a `Moved` arm today.** The TUI renders nothing (`crates/tui/src/lib.rs`, `model.rs::from_history`). ACP replays `[session moved to <to>]`. Both are marked ENGINE (`tui/src/model.rs` is the P1 engine worker's per §15).

### Bindings the UI worker imports

Generate: `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings`, then `cp -R bindings/. crates/desktop/ui/src/rpc/bindings/`. Keep the root `bindings/` too (`api.test.ts` reads `bindings/methods.json`).

New or changed files in `crates/desktop/ui/src/rpc/bindings/`: `SessionMoved.ts`, `HistoryEntry.ts` (`role: "moved"`), `ApprovalsState.ts` (`pin?`), `CreateSessionOptions.ts` (`engine?`), `EngineStats.ts` (+4), `MeshStatus.ts` (`stats`, `relay?`), `Peer.ts` (`path`, `rtt_ms`), `PeerPath.ts`, `RelayState.ts`, `NearbyEngine.ts`, `PairCeremony.ts`, `MeshEvent.ts` (`nearby`, `pair`), `FrontDoorError.ts` (`NotOwner`, `RunsElsewhere`), `PeerEnvelope.ts` (`chain`), `PeerInfo.ts` (`reachable?`), `methods.json` (`get_approvals`).

Hand-kept `crates/desktop/ui/src/rpc/api.ts`: `get_approvals(session)`, `list_models(engine?)`, `path_complete(…, engine?)`, `set_approvals(session, enabled, clear?)`, and `'get_approvals'` in `METHODS`.

UI edits forced by the types (each marked `// 183 P1: UI`): `store/reduce.ts` `historyEntries` gets a `moved` case that renders nothing, and `components/tasks/mock.ts` `MESH` gets `Peer.path` / `rtt_ms` and `stats: []`.

## XTICK (legacy routines fired on the wrong machine), calls made

Full notes: `182-xtick-notes.md`.

1. **H2 (R1).** S8 is restored for stamped files: a stamp naming an unpaired engine fires on the hub, as 182 had it. XTICK changes unstamped files only.
2. **"Created here"** means stamped with this engine (the stored identity at load, before the mesh plugin starts), or legacy on a lone engine. A stamp that changes to this engine is a claim made elsewhere, and is held one interval.
3. **The hub's claim grace** is 150 s (2 × TICK plus 30 s of slack). After it, the hub claims an unclaimed legacy file for the agent's one engine, else for itself.
4. **No offline cover (R2).** A stamped `agent:` file waits while its engine is offline, like any stamped file. The R1 cover created one task per interval.
5. **`ScheduleInfo.waiting?`** is a new optional field. It shows in `schedule_status` and as one muted line in the routines list. Both binding dirs (`/bindings`, `crates/desktop/ui/src/rpc/bindings/`) are gitignored and generated by `export_bindings`, so a clean checkout regenerates them before running tsc.
