# 190 DROP-CAUTIOUS (recon; written as 189 before renumbering): read-only recon at main 8f879fbd

Read-only: nothing built, run, edited or committed. Paths are relative to `/Users/Sky.Moore/Develop/theuth`; lines are from reading that tree.
Short names: **A** = `crates/a2a/src`, **N** = `crates/agent/src/communication/network.rs`, **F** = `crates/agent/src/communication/network/full.rs`, **L** = `crates/agent/src/loop_.rs`, **S** = `crates/agent/src/supervisor.rs`, **UI** = `crates/desktop/ui/src`, **T** = `crates/tools/src/registry.rs`.
The ruling: knowledge `decisions/theuth-drop-cautious-contact-mode-sky-2026-11-11` (every share is full, no mode choice, owner input behaves as normal).

## 0. Findings first

1. Cautious is a fork, not a flag. It forks at three points: the mode set at admission (`A/server.rs:622-634` stamps `ContactOrigin.mode`), the delivery switch (`N:1644-1653`), and the inbox limits (`N:3067-3173`). Everything else is a satellite hanging off those forks (capped turn, wrapper, final-text answer, queue rows, D8 guard, tools editor, contact cap).
2. FULL is already complete and independent: `deliver_full` (`F:272-361`), the return route (`F:98-104`, `F:634+`), the full inbox (`N:3153-3167`). Dropping cautious is mostly deletion plus fixing everything that defaults to cautious.
3. The fail-closed cap (186 R2/R3) is **not** cautious code. It is a second consumer of primitives that cautious introduced (`install_contact_limits`, `INBOX_FAIL_CLOSED`, `SetCap`/`SetAllowOnly`, the `inbox_full` row flag, the resume cap). Section 2 lists exactly what to keep.
4. Four traps that would hurt a builder who just deletes:
   - `peers.yaml` structs are `deny_unknown_fields` (`A/config.rs:115-116`, `:142-143`, `:204-206`). Deleting the `modes`, `tools` or `cap` fields from `PeerEntry`/`ExportEntry` makes the new build refuse a file the 186 build wrote.
   - The card JWS test pins a fixed signature vector over the card with the 186 extension (`A/contacts.rs:2402-2406`). Touching `EXT_FULL`/the card changes the vector.
   - W-40 (`N:4013-4019`) hard-codes `Cautious` for a chat owned by another engine of the mesh. The ruling does not say what that path becomes (section 7, ambiguity 1).
   - The turn-resume cap (`L:1706-1756`, `store/durability.rs:1085-1110`) reads the journal source `contact` that only cautious wrote. Turns journaled before the upgrade still need it.
5. The upgrade silently widens authority for every existing cautious grant (default: all of them, per 186 D2). See section 5 and section 7, risk 2.

## 1. Engine map (crates/a2a, crates/agent, crates/rpc, crates/tools, crates/talk, crates/tui, Slack/Signal/Telegram)

### 1.1 Everywhere the mode is read or written

Types and config (`A` = a2a):

| where | what | R/W | in 189 |
|---|---|---|---|
| `crates/agent/src/communication.rs:673-699` | `ShareMode {Full, Cautious}` (`#[default] Cautious`), `as_str()` | type | keep (read side of old rows/cards), stop offering `Cautious` |
| `communication.rs:701-726` | `ContactOrigin {contact, machine, export, cap, holder, tools (serde default), mode (serde default)}` | W by holder, R by owner | keep fields as wire-compat; `cap`/`tools`/`mode` unused except W-40 |
| `communication.rs:728-758` | `ContactEnvelope`, `ContactDeliver` (W-40 payload) | wire | keep unless W-40 goes |
| `communication.rs:248-251` | `PeerRecord.contact_mode: Option<ShareMode>` (ts optional) | R/W | keep |
| `communication.rs:801-803` | `PeerOrigin.contact_mode: Option<ShareMode>` (`#[serde(default)]`) | R/W | keep (UI tag, `by`) |
| `A/config.rs:19-22` | `no_full()` skip predicate | W | delete |
| `A/config.rs:142-143` | `PeerEntry` is `#[serde(default, deny_unknown_fields)]` | | **why `modes` must stay readable** |
| `A/config.rs:172-175` | `PeerEntry.modes: BTreeMap<String, ShareMode>`, `skip_serializing_if = "no_full"` | R/W | keep field, `skip_serializing`, never write |
| `A/config.rs:196` | `PeerEntry::is_contact` counts `!self.modes.is_empty()` | R | drop that clause |
| `A/config.rs:292-294` | `Peer.full: BTreeSet<String>` | R/W | delete (all granted exports are full) |
| `A/config.rs:388-427` | `same_grant` (via `Peer ==`) and `unchanged_exports` (`:423 a.full.contains(*e) == b.full.contains(*e)`) | R | drop the mode term |
| `A/config.rs:638-643` | `stage`: `full` = exports with `modes[e] == Full` | W | delete |
| `A/config.rs:832` | `full: BTreeSet::new()` (HTTP peer) | W | delete |
| `A/config.rs:1066-1096` | test `modes_default_to_cautious_and_are_written_only_for_a_full_grant` | test | rewrite: file with `modes:` still loads, never rewritten |
| `A/lib.rs:127`, `:262-273` | `A2aExportStatus.mode` from `p.full` (contacts only) | W | remove field |
| `A/lib.rs:507` | `PeerRoute.full` from `p.full` | W | set from `p.exports` (or drop) |
| `A/lib.rs:567-604` | `ContactLimits {contact, cap, exports (agent exports NOT full), full}` | W | shrink to `{contact, full-granted exports}` |
| `A/lib.rs:1141` | `full: Default::default()` | W | delete |
| `A/server.rs:141-146`, `:202-222` | `ContactGrant.full` | R/W | delete |
| `A/server.rs:622-634` | `ContactOrigin {cap: g.facts.cap, tools: self.tools.clone(), mode: full?Full:Cautious}` | W | stamp `Full` (or drop) |
| `A/contacts.rs:62-63`, `:515-528`, `:640-647` | `EXT_FULL`, card extension, `card_full()` | R/W | **keep** (wire + JWS vector) |
| `A/contacts.rs:99-102` | `Contact.modes`, `Contact.full_capable` | W | drop `modes`; keep `full_capable` as the version gate |
| `A/contacts.rs:125-137` | `FirstWaiting.mode`, `.queued`, `.position` | W | drop all three (see 1.3 (e) for W-40) |
| `A/contacts.rs:153-155` | `ContactPatch.modes` | R | see section 6 |
| `A/contacts.rs:540-545` | card skill tags `[text, kind, state, mode]` | W | keep emitting `full` (older peers read it, `F:659-661`) |
| `A/contacts.rs:1570-1608` | `contact_view` builds `modes` from `p.full` | W | delete |
| `A/contacts.rs:1611-1618` | `drop_export` removes `modes` | W | keep tolerant removal only |
| `A/contacts.rs:1666-1725` | `update_contact`: D3 `:1688-1697` 'choose full or cautious', `:1698-1700`, D10 `:1701-1708` 'update <machine> to share in full', D2 `:1718-1725` store only full | R/W | D3 goes; D10 becomes 'a newly granted export needs a full-capable contact' |
| `A/contacts.rs:1903-1913` | re-pointed export drops `modes` (shared cautious until re-chosen) | W | **safety goes away** (section 7, risk 7) |
| `A/contacts.rs:2184-2234` | `contact_waiting`/`waiting(queued)`; `:2218-2223` `FirstWaiting.mode` | R | drop mode and queued variant |
| `A/contacts/offers.rs:43-46`, `:84-121`, `:125-170`, `:306-309`, `:375-377` | `Cached.full`, `ContactOffer.mode` from `peer.full` / from card tag, `full_of()` | R/W | keep `of_card` tag read and `Cached.full` (version skew); `granted()` always `Full` |
| `N:314-315` | `ContactOffer.mode` | R | keep |
| `N:420-422`, `:496-497`, `:735`, `:2846`, `:2925` | `PeerRoute.full`, `RemotePeer.full` | R/W | keep as "exports this contact may write to", or rename |
| `N:624-626` | `NetworkState.full_inbound` (full message id -> task) | R/W | keep |
| `N:801-806` | `discover_peers` row tags `[contact, kind, state, mode]` | W | drop mode tag or always `full`; update `send_message` text (`crates/agent/src/peers.rs:437`) |
| `N:1504-1507`, `:1517`, `:1536-1540` | admission: `full` flag, 'unknown contextId' only for `!full`, `re` only for full | R | always full |
| `N:1598-1609` | `contact_scopes.insert` + `set_exchange_contact(receipt.id, origin.mode, None)` | W | keep as provenance, always full |
| `N:3067`, `:3153-3174` | inbox: `full = origin.mode == Full`; cautious branch installs `origin.cap`/`origin.tools` | R | delete the cautious branch only |
| `F:20`, `:199-210` (D12), `:236-238` (`offer.mode != Full -> Ok(None)`), `:328`, `:339`, `:659-661`, `:728`, `:739` | shape choice, downgrade check, provenance, return-route admission | R/W | see 1.8 |
| `L:1813-1818`, `:5641-5645` | journal source: `contact_inbox` / none (chat) / `contact_full` / `contact` when `peer.cap` | R/W | `contact_full` stays; others legacy |
| `crates/agent/src/store/communication.rs:397-455`, `:1517-1537` | `peer_exchanges.contact_mode` read (`:453-455` anything not 'full' = Cautious) and `set_exchange_contact` | R/W | keep (provenance; history has 'cautious') |
| `S:6337` | `PeerOrigin.contact_mode = record.contact_mode` | R | keep |
| `crates/rpc/src/export.rs:99` | ts export of `ShareMode` | | keep if the type stays |

`contact_mode` struct-literal sites that break if the field is removed (7): `L:10094`, `crates/agent/src/session_read.rs:515`, `N:6307`, `crates/schedule/src/lib.rs:5063`, `crates/slack/src/channel.rs:4447`, `crates/tui/src/model.rs:14244`, `:19928`.

### 1.2 The delivery fork (decides what stays)

`N:1636-1653`, `start_or_queue`: the single switch between FULL and the cautious slot logic.

```rust
        // 186 (D4): in full, the message takes no slot: it goes in as a mesh peer's at once.
        // W-40 (D11): a chat another engine owns gets it there (that owner lowers it).
        if self
            .network
            .contact_scopes
            .get(&task.request_scope)
            .is_some_and(|o| o.mode == super::ShareMode::Full)
            && (self.network.inboxes.contains_key(&task.session)
                || self.remote_owner(task.session).await.is_none())
        {
            return self.deliver_full(task, generation).await;
        }
```
Consequence: today a FULL grant of a chat owned by another mesh engine already runs the cautious path (W-40). After 189 the `is_some_and(mode == Full)` term disappears and the remote-owner term decides alone: see 1.7.

### 1.3 The cautious delivery path (receiver), piece by piece

(a) **One A2A task per message.** `admit_send` `N:1369-1630` (resume of a task `:1393-1471`; new task `:1509-1597`), `accept_a2a_request` (`crates/agent/src/store/communication.rs:2203`), `start_or_queue` `N:1636-1709`, `start_task` `N:1714-1779`, `register_scope` `N:1783-1814` (deadline watcher = `request_deadline()`, i.e. `max_runtime_secs`). The `contextId` rule is cautious-only: an unknown context is `invalid_params "unknown contextId"` unless `full` (`N:1516-1519`).
(b) **Capped turn.** `network_contact_frame` `N:3388-3413` (returns `Chat` for a W-40 reply or a scoped task, `Inbox` for an inbox; FULL gets `None` because `full_intake` sets `record.request_scope = None`, `F:397`). `ContactFrame` `L:208-235` and `PeerMeta.cap/tools/contact` `L:196-213`. The PeerInput handler `L:1777-1852` picks `contact_message()` (`L:8764-8775`), `cap = origin.cap`, `tools = origin.tools`. `TurnScope.cap` `L:284-300`. Start-of-turn install `L:1285-1298` (`set_turn_limits`, `L:8708-8733`: `SetAllowOnly{turn:true}` + `SetTurnCap`), cleared at turn end `L:5997-6004`.
(c) **Final-text answer.** `network_turn_finished` `N:2322-2365` (`sc.result = Ok(text)` `:2360`; `cancelled` -> `interrupted` + `owner_stop` `:2354-2357`), `try_settle` `N:2110-2160` (Completed artifact `:2141`; `Canceled` with `STOPPED_BY_OWNER` `:2124-2129`), `STOPPED_BY_OWNER` `N:630`, `Scope.owner_stop` `N:511`. **These are the generic A2A scope machinery too** (HTTP A2A peers with bearer tokens use the same `scopes`, `try_settle`, queue): do not delete them, only the contact flavour.
(d) **Queue of 8.** `CONTACT_QUEUED = 8` `N:3185`, applied `N:1477-1502`: counts `queued` + `held` + (186 D9) `full_inbound`. Still needed for held and unconsumed full messages.
(e) **Queued rows and the sender's 'queued: the chat is busy' note** (186 R1 item 9): `QUEUED_NOTE` `N:633`, set in `start_or_queue` `N:1680-1701`, `ContactsQueued`/`contacts_queued` `N:3698-3751`, `Store::note_queued_exchange` (`crates/agent/src/store/communication.rs:1553-1582`), sender-side parse in `apply_remote_task` `N:4528-4540`, `A/contacts.rs:2184-2234` (queued rows), `FirstWaiting.queued/position` (`A/contacts.rs:127-137`). All cautious-only (a FULL task takes no slot). If W-40 stays capped, the holder still queues those tasks, so the rows survive for W-40 only.
(f) **Held tasks.** Two different things: (i) the first-contact hold (W-27, Ask first): `hold_task` `N:3469-3517`, `rebind_held` `N:3520-3557`, `release_held`/`start_held` `N:3560-3637`, `HOLD_TTL`/`HELD_*` `N:636-643`; mode-agnostic (`ask_first` `N:1613`, release goes back through `start_or_queue` `N:3629`) so it **stays**; (ii) a FULL message's own task, `working` + `held=1` until consumed (`F:311-313`, `full_consumed` `F:404-436`), also stays.
(g) **Cautious agent inbox.** `handle_inbox` `N:3169-3173` installs `cap = ceiling.map_or(origin.cap, ...)` (`:3148`) and `origin.tools`; first row carries the `<contact_inbox/>` notice (`ContactFrame::Inbox`, `L:1801-1811`, `contact_inbox()` `L:8777-8787`); journal source `contact_inbox` (`L:1814`).
(h) **W-40:** 1.7.

### 1.4 The D8 cancel guard and its callers

- The guard: `S:4160-4166`

```rust
if m.reason == crate::CancelReason::Input && self.contact_turn(m.session).await {
    tracing::info!(session = %m.session, "cancel for input refused: a contact's turn is running");
    return;
}
```

- Its inputs: `F:453-464` `contact_turn()`, `crates/agent/src/store/durability.rs:1112-1134` `turn_took_contact` (journal sources `contact | contact_full | contact_inbox`; note it also matches FULL: the guard was never cautious-only).
- The reason type: `crates/agent/src/lib.rs:432-446` `CancelReason {Input, Stop(default)}`, `Cancel.reason` `:466-468`; RPC `crates/rpc/src/api.rs:267-276` (trait), `:4610-4637` (impl; a plain Stop forwards the old 2-parameter call `:4618-4623`); ts export `crates/rpc/src/export.rs:837`.
- Senders of `Input` (the only three): talk barge-in `crates/talk/src/lib.rs:662-674`; TUI interject `crates/tui/src/lib.rs:392-407`; desktop `UI/components/chat/Composer.tsx:866`, `:888` (interject), `:998` (flush).
- Senders of `Stop`, which never hit the guard, so no behaviour change; they only matter if the `reason` field is deleted (compile): Slack `crates/slack/src/channel.rs:193-203`, `commands.rs:392-405`; Signal `crates/signal/src/commands.rs:227-261`; Telegram `crates/telegram/src/port.rs:173-184`; schedule `crates/schedule/src/lib.rs:2146,2159`; autoresearch `actor.rs:4724,5577`, `wave.rs:119`; advisor `crates/agent/src/advisor.rs:679,1137,1559,1632,1684`; `crates/agent/src/lib.rs:500`; `S` test literals `:17928`, `:17988`, `:18011`, `:18725`, `:18738`, `:20105` and the D8 test `:14933-14953`. About 28 `Cancel {..}` construction sites in 13 files. UI `api.cancel(.., 'stop')`: `App.tsx:343`, `Composer.tsx:1036`, `ObservedBar.tsx:121`, `slash.ts:704`.
- Slack, Signal and Telegram have **no** interject or cancel-on-input path (a message while a turn runs is a `Steer`, `crates/slack/src/channel.rs:180-190`); only the explicit `/cancel`, always `Stop`.
- D8's contact text: `STOPPED_BY_OWNER` `N:630`; `contact_turn_finished` maps `cancelled` to it `N:4048-4049` (W-40 return) and `try_settle` `N:2124-2129`.

### 1.5 The `<contact_message>` / `<contact_inbox>` wrappers and contact prompts

- Writers: `contact_message()` `L:8761-8775` (neutralises `<contact_...`/`<message...` in the body; says 'Tools above <cap> are refused for this turn; your reply goes back to <contact>'), `contact_inbox()` `L:8777-8787`. Only the cautious `ContactFrame` calls them (`L:1792-1810`).
- Prompt: `crates/agent/src/frame.rs:499-521` `contact_baseline`/`CONTACT_RULE` ('your final text is your answer and is all the contact sees'), chosen per turn by `tools::ContactScoped` (`L:4970-4983`, `unwrap_or(true)` = fail closed). In FULL the rule's last clause is false; it still governs the capped inbox state (inert there: no tools, so nothing can answer).
- Readers of old rows (history keeps them forever, so these stay): TUI `crates/tui/src/model.rs:11584-11636` `contact_row` (+ `:6646-6649`, `:11384`, tests `:14234-14350`); UI `lib/injected.ts`, `store/reduce.ts:325`, `:385-394`, `components/chat/HarnessCards.tsx:376-428` (`ContactInboxCard`, `ContactMessageCard`: 'X will see only this turn's answer'), `components/chat/Message.tsx:102-104`.

### 1.6 CONTACT_GRANTABLE and the per-export tools editor's RPC

- Engine: `T:616-635` `CONTACT_GRANTABLE` (11 names), `:640-644` `contact_grantable`, `:660-700` `GrantableTool` + `ContactGrantable`, `:709-747` `SetAllowOnly` (filters every installed allowlist through `contact_grantable`, `:731-741`), `:1096-1132` admission (`allowlist_admits`, `grant_key`).
- Config: `A/config.rs:206-251` `ExportEntry.tools` (`deny_unknown_fields`, written only when not `default_contact_tools()` = `search_web`,`read_url`), `contact_tools()` `:235-247`, staged `:349-370`, applied `:695`, `:720`.
- RPC: `crates/rpc/src/api.rs:2684-2686` (+ impl `:9615-9617`) `a2a_grantable_tools`; tools ride `a2a_export_put`/`a2a_exports` via `Export.tools` (`A/contacts.rs:239-244`, views `:1827`, `:1837`, put validation `:1876-1892`, keep-list `:1900-1902`); handler `Handler.tools` `A/server.rs:223-250` -> `ContactOrigin.tools` `:628`; live push `ContactLimits.exports` `A/lib.rs:567-588`, `N:3364-3380`; ts export `crates/rpc/src/export.rs:249`.
- UI: `UI/components/settings/Contacts.tsx:919-1003` (`DEFAULT_TOOLS`, `READS`, `toolNote`, `toolsLine`, `ExportTools`), `:759` intro line, `:845-849` (shown for cautious rows only), `UI/lib/rpc.ts:55-56`, `UI/rpc/api.ts:799`, `:1427`.
- After 189 the only allowlist anything installs is the empty fail-closed one, so the editor, the RPC, `CONTACT_GRANTABLE` and `GrantableTool` have no live consumer. **Keep** `SetAllowOnly`, `allow_only`/`turn_allow_only`, `allowlist_admits`, `contact_scoped`, `Allowlisted` (section 2). Under any allowlist `grant_key` admits only grantable names (`T:1130`: `_ => CONTACT_GRANTABLE.contains(&name).then_some(name.into())`), so an empty set denies everything even if `CONTACT_GRANTABLE` were emptied.
- The contact **cap** (`read_only|mutating`) is cautious-only too and has the same fate: `A/config.rs:48`, `:62`, `:166`, `:619` (`a2a.contact_cap`, `PeerEntry.cap`), `ContactFacts.cap` `:326`, `Contact.cap` `A/contacts.rs:85-86`, `ContactPatch.cap` `:148`, `update_contact` `:1681-1686`, `cap_changed` `A/lib.rs:525`, `ContactOrigin.cap`, UI `CAP`/`CAPS`/cap `Select` (`Contacts.tsx:53-58`, `:782`, `:800-813`). A FULL share ignores it (186 D6), so a visible cap control would mislead.

### 1.7 W-40: a contact's chat that lives on another engine of the mesh

How it runs today (all cautious):
- Holder H (the engine that holds the contact and its A2A task): `start_task` forwards instead of running (`N:1750-1771`, `forward_contact` `N:3873-3903`, `contact_forward` `crates/agent/src/communication/mesh.rs:141-161`, RPC `contact_deliver` `crates/rpc/src/api.rs:250-251`, `:4490-4510`). FULL falls into this because of the `remote_owner` term in 1.2.
- Owner O: `contact_deliver` `N:3908-4042`. The decisive lines, `N:4013-4019`:

```rust
// The owner may lower the cap, never raise it above `mutating`. 186 (D11): and the
// mode, never raise it: in full a contact's answers travel the holder's return route,
// which a turn here cannot reach, so this engine serves it cautiously.
let mut env = env;
env.origin.cap = env.origin.cap.min(tools::Risk::Mutating);
env.origin.mode = super::ShareMode::Cautious;
```

  then a capped turn (`network_contact_frame` `N:3392-3397` -> `ContactFrame::Chat`, allowlist = H's `origin.tools`) whose final text goes back: `network_turn_finished` `N:2331-2344` -> `contact_turn_finished` `N:4044-4083` -> `contact_forward` to H -> `contact_deliver` with `reply_to` on H completes the A2A task with the text (`N:3919-3947`).
- The contact's engine (the sender) then reads a Completed task with no `receipt` metadata and imports the text as a reply (`apply_remote_task` `N:4453-4454`).
- Why FULL cannot simply run there: the answers go out as a POST to the contact's `/agents/~return/rpc` from the engine whose endpoint the contact pinned (H). O's agent has no `peer:<alias>` binding and cannot speak as H.
- Surface: `crates/agent/src/communication.rs:701-758`, `N:3799-4084`, `mesh.rs:121-165`, `crates/rpc/src/api.rs:250-251,4490-4510`, `NetworkState.contact_waits/contact_replies` `N:609-615`, `resumed_contact_envelope` `N:2367-2400`, supervisor tests `S:16317-16600`. About 420 lines of production code.
- Options for the builder (the ruling does not pick one): A keep W-40 as an internal capped final-text path; B build FULL over W-40 (O's `send_message(reply_to)` relayed to H's return route: a new RPC); C refuse a contact message to a remote-owned chat ('the chat lives on another machine of this mesh'). See section 7.

### 1.8 Sender side: what stays and why

- `network_send` `N:4141-4361` branches on `full_shape` (`F:157-268`): `Some(shape)` = conversation (contextId, `metadata.theuth.re/chain`, return route for answers), `None` = 183 shape (a plain task; a reply to an exported task's contact = an INPUT_REQUIRED question, `N:4190-4207`, `:4279-4298`).
- `None` is returned when the contact's card has no offer for the export or `offer.mode != Full` (`F:226-238`). Keep this while peers on 186/183 builds exist: a 186 receiver with a cautious grant answers a full-shaped message `invalid_params "unknown contextId"` (`N:1516-1519` on its side).
- `apply_remote_task` `N:4441-4548` and `import_reply` `N:4552-4616` (Completed final text), `import_question` (`N:4621+`) are **the generic A2A HTTP peer path** (Completed -> reply; INPUT_REQUIRED/AUTH_REQUIRED -> question), so they stay regardless of cautious. `full::task_is_receipt` (`N:4453`) picks the FULL receipt.
- `A/client.rs:781-794` (metadata for full), `:1021` (receipt on the status event): unchanged.
- The two FULL checks that mention the mode: `F:199-210` (answering a conversation the contact opened requires `p.full.contains(&t.export)`, D12) becomes 'the export is still granted' (`p.exports`); `F:659-661` (return-route admission requires the contact's latest card to list the export as `Full`) becomes 'lists the export'. Keep the 7-day TTL (`F:29`) and the bound of 8.

### 1.9 RPC and ts-rs rows (proposal in section 6)

`contact_update(name, ContactPatch)` (`crates/rpc/src/api.rs:2666-2671`, impl `:9587`), `contact_list`, `contact_first_waiting` (`:2698-2699`, impl `:9633-9635`), `a2a_exports`, `a2a_export_put`, `a2a_grantable_tools` (`:2684-2686`), `cancel(session, scope, reason)` (`:267-276`), `contact_deliver` (`:250-251`). ts exports: `crates/rpc/src/export.rs:85` `PeerRecord`, `:87` `ContactEnvelope`, `:88` `ContactOrigin`, `:98` `PeerOrigin`, `:99` `ShareMode`, `:111` `A2aExportStatus`, `:113-122` `FirstContact`, `Contact`, `ContactPatch`, `ContactTarget`, `ContactWindow`, `NearbyAgent`, `ExportTarget`, `Export`, `ContactEvent`, `FirstWaiting`, `:249` `GrantableTool`, `:837` `CancelReason`.

### 1.10 Store and migrations

Migrations `crates/agent/migrations/`: 0054 `a2a_contexts.inbox`, `inbox_identity`; 0055 `a2a_tasks.held`; 0056 `peer_exchanges.context_id`, `peer_exchanges.contact_mode CHECK (... IN ('full','cautious'))`, `contact_threads`, `a2a_contexts.inbox_full`; 0057 `a2a_contexts.inbox_agent`, `inbox_ceiling CHECK (... 'none','read_only','mutating','destructive')`.
No column is dead after 189 (verdicts in section 5.6). Store functions that carry mode: `set_exchange_contact` (`crates/agent/src/store/communication.rs:1517-1537`), the `contact_mode` read (`:397-455`), `note_queued_exchange` (`:1553-1582`, dead with the queue rows), `turn_took_contact` (`crates/agent/src/store/durability.rs:1112-1134`, dead with D8); the fail-closed functions (`mark_a2a_inbox` `:2071`, `unmark_a2a_inbox_full` `:2114`, `a2a_inbox_full` `:2126`, `full_a2a_inboxes` `:2146`, `a2a_inboxes_of` `:2166`, `revoke_peer_bindings_except` `:1136`) stay.

### 1.11 Rust tests that pin cautious behaviour (delete or rewrite)

- `N` test module (`N:4738-7178`): the helper `bob(export, cap)` builds a `Cautious` origin (`:5438-5449`), so these run the cautious path: `first_contact_holds_until_the_owner_answers` (`:5466`), `a_held_first_message_expires_unanswered` (`:5568`), `the_hold_timer_fires_at_the_ttl_and_not_before` (`:5612`), `a_contact_cancel_of_a_held_message_ends_it_at_once` (`:5643`), `a_contact_never_runs_out_of_contexts` (`:5734`), `retries_of_one_message_leave_one_context` (`:5774`), `a_contact_session_spawns_nothing` (`:5828`), `a_contact_turn_in_a_chat_spawns_nothing_at_the_registry` (`:5869`), `an_inbox_neither_sees_nor_reaches_another_contact` (`:5909`), `a_held_send_reads_held_and_names_the_contact` (`:5955`), `a_grant_change_keeps_held_first_messages_of_exports_still_granted` (`:6046`), `a_cap_or_allowlist_change_reaches_a_live_inbox_at_once` (`:6117`), `an_inbox_is_capped_and_default_deny_at_every_actor_start` (`:6151`, rewrite as the fail-closed test), `a_restarted_inbox_runs_the_exported_agents_persona_again` (`:6206`), `a_contact_inbox_is_one_capped_root_reused_per_contact_and_export` (`:6253`), `an_agent_export_inbox_runs_the_exported_agents_persona_model_and_ceiling` (`:6354`), `an_owner_stop_tells_the_contact_so` (`:6520`), `a_queued_contact_message_is_visible_until_its_turn_starts` (`:6790`). Keep: the `full_inbox_tests` module (`:6879-7178`: ceiling, no-persona refusal, unshare/block/unpair, removed-while-down) and `a_full_contact_message_is_a_peer_message_and_its_task_a_receipt` (`:6426`), `the_return_route_admits_answers_only_on_a_live_full_conversation` (`:6537`).
- a2a e2e (`A/contacts/e2e_tests.rs`): helper `cautious()` `:1131-1136`, `modes: Some(cautious(exports))` `:1168`, `:1333`, tag assertion `:1219` (`["contact","agent","allowed","cautious"]`), D3 refusal `:1594`, downgrade/re-point `:1845-1925`; tests `a_contact_reaches_its_export_first_contact_waits_and_block_refuses_at_the_gate` (`:414`), `a_put_without_tools_keeps_the_exports_allowlist` (`:1025`), `a_granted_export_is_a_discover_peers_row_on_the_contacts_side_until_revoked` (`:1145`), `a_grant_change_keeps_a_waiting_first_message_and_always_releases_it` (`:1308`), `a_chat_shared_in_full_talks_both_ways_like_a_mesh_peer` (`:1545`, loses the D3/downgrade assertions).
- `S`: D8 test `:14933-14953`, W-40 tests `:16317-16600` (`:16398` Full origin, `:16441` Cautious), `:17808`.
- `A/config.rs:1066-1096`; `crates/agent/src/store/durability.rs:1986-2030` (`a_contact_turn_is_known_from_its_journal_whatever_rows_follow`: **keep** the `contact` and unknown-journal assertions); `L:9990-10001` (`contact_inbox` notice test); `T` tests at `registry.rs:3419-3630` (allowlist, `ContactGrantable`, `ContactScoped`: keep the deny-all ones).

## 2. The fail-closed state that MUST stay

### 2.1 The capped states and where each lives

| # | state | trigger | where |
|---|---|---|---|
| S1 | inbox runs capped (read-only + empty allowlist) | row has `inbox = 1` and `inbox_full = 0`: never admitted full, or unmarked | `reinstall_contact_limits` `N:3226-3271`; fallback `(INBOX_FAIL_CLOSED, [])` `N:3264-3270`; installer `install_contact_limits` `N:3192-3220` |
| S2 | full inbox, ceiling unknown | `inbox_full = 1` but `inbox_ceiling` NULL, agent missing (`Live::Missing`) | `full_ceiling` `F:591-604` -> `run_full_inbox(id, None)` `F:480-492` -> `install_contact_limits(id, INBOX_FAIL_CLOSED, [])` |
| S3 | binding revoked at actor start | `peer_binding_by_id` finds nothing | `N:3238-3257` (unmark + fall through to S1) |
| S4 | share taken out of full / not granted any more | reload: unshare, delete, block, unpair (and, today, downgrade) | `apply_contact_limits` `N:3334-3381` (`unmark_a2a_inbox_full`; `!granted` -> `INBOX_FAIL_CLOSED`) |
| S5 | contact removed or blocked while the engine was down (186 R3) | first `CommitPeerConfig` after boot | `drop_unrouted` `F:520-530` -> `Store::revoke_peer_bindings_except` (`crates/agent/src/store/communication.rs:1136`) -> `cap_inboxes_of` `F:496-515` |
| S6 | no persona for a full agent share (186 R2) | roster not joined (except `theuth`), agent file missing or invalid | admission refused `N:3072-3089` ('export unavailable: <export> is not available'); live ceiling re-read at admission `N:3090-3167`, at actor start `N:3247-3252`, on roster change `F:536-575` (`InboxPersonasChanged`, `crates/roster/src/lib.rs:516-521`) |
| S7 | resumed turn that took a contact | journal `input_accepted` source `contact`, or no `input_accepted` row at all ('unsure counts as a contact's'), or the lookup errors | `Session::Resume` `L:1706-1756` (`set_turn_limits(.., ReadOnly, Some([]))`, `self.scope.cap`), `Store::contact_turn_input` `crates/agent/src/store/durability.rs:1085-1110`, clear at turn end `L:5997-6004` |
| S8 | limits cannot be installed -> no actor | `install_contact_limits` error | `S:1100-1105` |
| S9 | prompt hygiene for capped sessions | `ContactScoped` true (or error -> true) | `L:4970-4983`, `frame::contact_baseline` `crates/agent/src/frame.rs:499-521` |
| S10 | 'none' vs NULL ceiling | `'none'` = agent has no ceiling; NULL = unknown = capped | `crates/agent/src/store/communication.rs:276-302` (`ceiling_text`, `parse_ceiling`), `mark_a2a_inbox` `:2071-2095` |

### 2.2 Exactly what that reuses (so the builder deletes the mode and keeps these)

Keep as they are, or edit only as noted:

1. Registry primitives (`T`): `SetCap` `:514-534`, `SetAllowOnly` `:709-747` (both `turn:false` for inboxes and `turn:true` for the resume cap), `SetTurnCap` `:536-556`, maps `cap`/`turn_cap`/`allow_only`/`turn_allow_only` `:69-82`, `allowlist_admits` `:1100-1120`, `contact_scoped` `:1136-1150`, `cap_for` `:1173-1185`, `ContactScoped` `:1641-1649`, `Allowlisted` `:1655+`, `EffectiveCap` `:1624+`, `CAP_EXEMPT` (`send_message` is exempt from the cap **except in a contact session**, so a capped inbox cannot relay: comment at `:1187-1189`). Only the `CONTACT_GRANTABLE` filter (`:731-741`, `:1130`) can go.
2. Agent limit installers: `INBOX_FAIL_CLOSED` `N:3181`, `install_contact_limits` `N:3192-3220`, `reinstall_contact_limits` `N:3226-3271`, `clear_contact_limits` `N:3275-3305` (the FULL state: no allowlist, `SetCap` = the agent's own ceiling), `run_full_inbox` `F:480-492`, `cap_inboxes_of` `F:496-515`, `drop_unrouted` `F:520-530`, `refresh_full_inboxes` `F:536-575`, `Live`/`full_ceiling` `F:578-604`, `apply_contact_limits` `N:3334-3381` **but only its first half** (`:3339-3363`: unmark + cap what is no longer granted); the `for (export, tools) in m.exports` half (`:3364-3380`) is the cautious allowlist push and goes with `ContactLimits.exports`/`.cap`.
3. `inbox_limits` (`N:621`): after 189 it only ever holds `(INBOX_FAIL_CLOSED, [])`. May be dropped if `reinstall_contact_limits` calls the fail-closed install directly. Do not drop the call.
4. Store/columns: `a2a_contexts.inbox`, `inbox_full`, `inbox_agent`, `inbox_ceiling` and `mark_a2a_inbox`, `unmark_a2a_inbox_full`, `a2a_inbox_full`, `full_a2a_inboxes`, `a2a_inboxes_of`, `revoke_peer_bindings_except` (lines in 1.10). **`inbox_full` changes meaning, not purpose:** it stops meaning 'this share is full' (all are) and means 'this inbox may run uncapped under the agent's ceiling'; `0` is the capped state. Do not delete or rename without a migration that keeps the default `0`.
5. Resume cap: `Store::contact_turn_input` (`durability.rs:1085-1110`), `Session::Resume` `L:1706-1756`, `set_turn_limits` `L:8712-8733`, `TurnScope.cap` `L:284-300`, end-of-turn clear `L:5997-6004` (`if self.scope.cap.is_some()` keeps working because Resume sets `scope.cap` at `L:1749-1750`). A turn journaled `contact` before the upgrade and interrupted by the restart resumes capped; a turn with a missing journal does too. Only the *start-of-turn* install for a new cautious turn (`L:1285-1298`) and `PeerMeta.cap/tools` go.
6. Prompt: `ContactScoped` -> `contact_baseline` (`L:4970-4983`, `frame.rs:499-521`). Edit `CONTACT_RULE` ('your final text is your answer and is all the contact sees') since it is false in FULL.
7. Supervisor: `S:1100-1105` (no actor without limits), `S:765` (`refresh_full_inboxes`).

Delete (cautious-only): the cautious branch of `handle_inbox` (`N:3169-3173` and `cap = ... origin.cap` `:3148`), `ContactLimits.cap/.exports` (`A/lib.rs:567-588`), `PeerMeta.cap/tools` + `ContactFrame` + `contact_message()` + `contact_inbox()` (`L:196-235`, `:1285-1298`, `:1788-1812`, `:8761-8787`), `network_contact_frame` for non-W-40 (`N:3388-3413`), `turn_took_contact`/D8, `CONTACT_GRANTABLE` filter, queue rows (1.3 (e)).

### 2.3 Other safety checks that lean on cautious code

- **W-40 `ContactOrigin.mode` lowering** (`N:4013-4019`): a safety clamp (`cap.min(Mutating)`, mode forced `Cautious`) that exists only because FULL cannot be answered from O. If W-40 stays capped it must stay as is; if W-40 goes FULL the clamp is moot but the missing return route must be solved first.
- **Resume cap source `contact`**: written only by cautious turns (`L:5641-5645`: `if peer.cap.is_some() { "contact" }`). Nothing new will write it; the reader (S7) must stay for old journals and the unsure case.
- **Re-pointing an export drops the grant to cautious** (`A/contacts.rs:1903-1913`): today a contact never silently gets FULL access to a different chat or agent than the one full was chosen for. After 189 re-pointing keeps the contact on the new target at full. (The grant epoch still rotates: `same_grant`/`unchanged_exports` compare `ExportEntry` equality, `A/config.rs:388-427`, so held first messages fail with `HOLD_CHANGED`.) Decision needed, section 7.
- **Default-deny for contact turns in chats and agent inboxes** (`allowlist` + contact `cap`): gone for every non-fail-closed case by design (186 D6 already says FULL uses the chat's own policy; approvals `tools.policy` still apply).
- **'Refusal without persona'** (`N:3072-3089`), **'offline-removal capping'** (S5), **'none' vs NULL** (S10): all FULL-side, none leans on cautious code; they only need the shared installers kept.
- **First-contact Ask/Allow/Block** (`A/server.rs:635-645`, `N:1613`, `N:3469-3637`): mode-agnostic.
- **Rate and concurrency bounds** (10 req/s, 8 concurrent per contact at the gate; the return route's 8 unconsumed answers per contact, 186 D9, `docs/design/186-contact-collaboration.md:23`): unchanged.

## 3. UI (`crates/desktop/ui/src`, "UI/")

188 moved the contacts UI to **Settings › Network › Contacts** (`UI/components/settings/categories/network.tsx:19` -> `ContactsTab` `Contacts.tsx:142-146` -> the contact list `:155-270` and `ContactAccess` `:750-766`) and a chat **Messaging…** sheet (`UI/components/chat/MessagingSheet.tsx`, `messaging-sheet.ts`, opened from `SessionMenu.tsx:561`, `CommunicationNotice.tsx`). The sheet has **no mode, cap or contact-share UI** (grepped: only an unrelated description at `MessagingSheet.tsx:167`); the chat's share control is `Share with contact…` (`SessionMenu.tsx:45,561-567` -> `SharingItems`/`ShareDialog`, `Contacts.tsx:1007-1101`).

### 3.1 Share mode choice, badges and changers

| file:line | what | 189 |
|---|---|---|
| `lib/contacts.ts:15-16` `SHARE_RULE`, `:19-20` `PAIRED_RULE` | copy that offers 'Full collaboration ... Cautious: plain text, one limited turn per message, read-only tools' (shown `Contacts.tsx:176`, `:361`, `:631`) | rewrite without the choice |
| `lib/contacts.ts:23` `modeOf` (default `'cautious'`), `:26` `capBinds`, `:29-37` `modeLines`/`MODE_LABEL`/`modeHint`, `:39` `updateNeeded` | mode helpers | delete all but `updateNeeded` (D10 gate) |
| `lib/contacts.ts:76-82` `sharedLabel` | chat header chip: `Shared with bob · full` / `· read only` (`components/chat/Header.tsx:214`) | `Shared with bob`, no cap |
| `Contacts.tsx:45` `NO_PATCH` (`modes: null`), `:57` `MODES` | patch literal, mode list | drop `modes` |
| `Contacts.tsx:127-139` `ExportChip` (`mode` suffix `:135`), `:197` | chip text `... · cautious` | drop `mode` |
| `Contacts.tsx:200-203` | 'Shared with you: ... (full)' (reads `ContactOffer.mode`) | drop the suffix |
| `Contacts.tsx:641-657` `useGrant({mode})` | grant with a required mode (`modes: {[exp]: mode}` `:653`) | no mode argument |
| `Contacts.tsx:660-665` `useMode`, `:696-711` `ModeSelect`, `:825`, `:1070` | the per-grant mode changer | delete |
| `Contacts.tsx:671-693` `ModeButtons` (two buttons; `old = !c.full_capable` -> `updateNeeded` `:675-687`) | the required choice when sharing (`:853-867` in Access, `:1076` in ShareDialog) | one 'Share' action, disabled with `updateNeeded` for a contact that is not `full_capable` |
| `Contacts.tsx:845-849` | row text 'Full collaboration: <hint>' else `ExportTools` | drop |
| `Contacts.tsx:1026`, `:1082` | ShareDialog doc comment and 'switch ... between Full collaboration and Cautious' | reword |
| `settings/Communication.tsx:9`, `:209`, `:215-217` | Messaging tab A2A exports: mode `Badge`, text '... full collaboration ...' / '... cautious: one capped turn per message' (reads `A2aExportStatus.mode`) | drop badge, one text |

### 3.2 The per-export tools editor and the contact cap

- Editor: `Contacts.tsx:919-1003` (`DEFAULT_TOOLS`, `READS`, `toolNote`, `toolsLine`, `ExportTools`), used `:759` (the `toolsLine` paragraph) and `:848`; `UI/lib/rpc.ts:55-56` `queryKeys.grantable`; `UI/rpc/api.ts:799` + `:1427` (`a2a_grantable_tools`).
- Explainer text 'Cautious shares can use only these tools. Everything else, including your files, sessions and vault, is hidden from them. ...': `toolsLine` `Contacts.tsx:944`.
- Cap: `Contacts.tsx:53-58` (`CAPS`), `:188` (`capBinds(c) && CAP[c.cap]`), `:782`, `:787-813` (cap `Select`, `allFull`, 'The cap applies to cautious shares only'), `:1066`; `UI/lib/contacts.ts:9` `CAP`; `MessagePeer.tsx:174-175`, `:275` `capLabel`, `PeerGroup.tsx:43`, `HarnessCards.tsx:372` (these four render the cap of an OLD wrapper row: keep for history).
- `Contacts.tsx:755-758` intro ('which tools it may use there (read only, or able to change things)') is stale.

### 3.3 Composer, StopButton, QueuedChips (D8 UI)

- `components/chat/Composer.tsx`: `:68` `CAP` import, `:71`/`:345-346` `contactTurn` -> `contact`, `contactName`; `:866`, `:888`, `:998` `api.cancel(session, null, 'input')`; `:1017` `interrupt(at, filled, !!contact)`; `:1289-1299` pill 'Runs after {contact}'s turn' + ghost 'Stop {contact}'s turn' (`stop('root')`); `:1558-1559` send tooltip; `:1566` `<StopButton ... label={contact ? `Stop ${contactName}'s turn` : undefined} />`.
- `lib/composer-state.ts:15-26`: `interrupt(p, filled, contact = false)`; `:23` `if (contact) return filled ? 'steer' : null` is the D8 neutering. Drop the third parameter.
- `components/chat/StopButton.tsx:19-20` `label: tap = 'Stop'` prop (`:111-112` aria-label/title). Drop the prop or leave it unused.
- `components/chat/QueuedChips.tsx:14`, `:74-75` `contactTurn`, `:94` `queued · runs after {contact}'s turn` vs `queued · {chord} to interrupt`.
- `store/sessions.ts:857-864` `contactTurn()` (opener scan: `user | contact_message | peer with .contact`): its only callers are `Composer.tsx:345` and `QueuedChips.tsx:75`, so it becomes dead (`reduce.test.ts:172-191` goes with it). `ContactNote`/`turnBy` stay.

### 3.4 Strip rows, `useFirstWaiting`'s queued filter

- `components/chat/ContactAllowStrip.tsx`: `:3` imports `MODE_LABEL, modeLines, useFirstWaiting`; `:15` row text '... wants to message this chat · {MODE_LABEL[w.mode]}'; `:24-43` `QueuedRow` ('N message(s) wait(s) for this turn to finish'); `:52` `useFirstWaiting(true, true)`; `:54-57`, `:71`, `:80-82` queued grouping; `:85-90` `modeLines(...)`. After 189 the strip shows first-contact holds only.
- `lib/contacts.ts:53-59`: `useFirstWaiting(enabled, withQueued)` with `select: withQueued ? undefined : (rows) => rows.filter((w) => !w.queued)`. Drop the parameter and the filter (unless W-40 keeps queued rows).
- Other `FirstWaiting.mode` readers: `components/attention/PlainScreens.tsx:12,155,165` (`modeLines([mode], kind)[0]`), `components/hud/Hud.tsx:23,203-206` ('{MODE_LABEL[mode]} share. '), `settings/Communication.tsx:121`, `Contacts.tsx:72-124` (`waitingLine`/`WaitingRow`: no mode, fine).

### 3.5 PeerCards, SendCard, 'asked during X's message' (must keep working)

- `components/chat/MessagePeer.tsx:136-180` (`Provenance`: 'Access' row from the wrapper cap), `:279-302` (`full` prop: `outside = !full && (isOutside(sender.key) || !!contact)`), `:320` header tag `contact ? capLabel(contact.cap) : full ? 'Contact · full' : undefined`, `:379` `full={origin.contact_mode === 'full'}`. `SendCard.tsx:220-221`: reply is Markdown unless `answer.origin.contact_mode !== 'full'`.
- The tag: `store/reduce.ts:414` (`h.peer?.contact_mode === 'full' ? h.peer.from : null`), `:800` (`e.origin.contact_mode === "full" ? { turnBy: e.origin.from }`), `:1062`, applied to tool cards at `:1143`, `:1162` as `by`; shown by `components/chat/Message.tsx:52-54` (`AskedDuring`, `PROMPTED[entry.name]`). It keys on **`PeerOrigin.contact_mode === 'full'`**, so keep the engine writing `contact_mode: full` for every contact exchange (`F:328`, `:339`, `N:1605`) and keep the field in the binding. Switching it to a boolean or to `from.startsWith('peer:')` is wrong (HTTP A2A peers are also `peer:`).
- After 189 the `contact_mode === 'cautious'` branches (`PeerCards.test.ts:254-300`, `reduce.test.ts:1729-1760`) are history-only.

### 3.6 Tests that pin each (rewrite or delete)

- `components/settings/Contacts.test.ts` (734 lines, about 14 tests touch the mode, cap or tools): `:389` cap, `:401` export picker 'chosen mode' (`modes: {research:'cautious'}` `:421-422`), `:438` tools editor (`:441`), `:460` tools line, `:476` ShareDialog (`:479`, `:488-490`), `:536` ContactFirstScreen mode, `:602` Communication, `:619` queued is no hold (`:620`), `:628` 'Shared with you', `:651`, `:666` D3/D10 two buttons, `:687` per-grant mode + cap off when all full (`:693-709`, `:723-725`), `:728` Communication mode text.
- `components/chat/ContactAllowStrip.test.ts` (149 lines; `:13` fixture `mode:'cautious'`, `:49`, `:59` mixed modes, `:114`, `:126`, `:142` queued rows).
- `components/chat/PeerCards.test.ts:254`, `:271` (full vs cautious rendering), `:302` (`by`), `:193`, `:452` (old wrapper cards: keep).
- `components/chat/Composer.test.ts:59-61` ('Stop sky's turn'), `:66-` (input vs stop reasons); `components/chat/QueuedChips.test.ts:74-80`; `lib/composer-state.test.ts:29-35`; `store/reduce.test.ts:172-191` (`contactTurn`), `:1729-1760` (`by`), plus `contact_mode: null` literals in `components/chat/Peers.test.ts:22`, `ToolRunCard.test.ts:95-125`, `jump.test.ts:21`, `lib/injected-wrappers.test.ts:101`, `reduce.test.ts:186,468,493,526,534` (they only break if the field is removed).
- `rpc/api.test.ts` checks `api.ts` method names against `<repo>/bindings/methods.json` (generated by `just ts`): removing `a2a_grantable_tools` means regenerating it.

## 4. TUI and docs

TUI (`crates/tui/src`): **there is no share/mode UI.** `/a2a` prints peers and exports without a mode (`model.rs:11751-11800` `a2a_lines`), there is no contacts screen and no contact Attention/strip. The only contact-related code:
- interject: `lib.rs:392-417` `Command::Steer {interject}` -> `api.cancel(session, None, Some(agent::CancelReason::Input))` (`:402-406`, comment `:401`). Becomes a plain cancel (or stays if `reason` is accepted and ignored).
- history rows: `model.rs:11584-11636` `contact_row` (parses an old `<contact_message>`/`<contact_inbox/>` wrapper into the peer card, cap shown as 'read-only'/'can make changes'), used at `:6646-6649` and `:11384-11392`; tests `:14234-14350` (`contact_rows_show_the_words_and_the_facts_never_the_wrapper`, `a_contact_turn_is_never_echoed_as_a_you_bubble`). Keep for history.
- `contact_mode: None` literals `:14244`, `:19928` (break only if `PeerOrigin.contact_mode` is removed).

Docs:
- `docs/design/186-contact-collaboration.md` (128 lines) is the **only** markdown in the repo that says 'cautious' (24 hits: D1-D3, D8, D10-D12, Contract, Tests). It needs a supersede note plus a 189 as-built doc. `docs/design/183-*.md` describes the capped turn without the word.
- `site/` (user docs): no page describes cautious, full or A2A contacts. `site/content/docs/reference/configuration.mdx:38,254-256,348` documents `a2a/peers.yaml` and `a2a.enabled|listen|public_url` only (not `contact_cap`); `workspace/desktop.mdx:112` 'Network (A2A)'; `reference/cli.mdx:145,205` `/a2a`; `automation/channels.mdx` 'contact' means Signal phone contacts. Nothing to delete there.
- Stale strings to reword (code, not docs): config template `crates/theuth/src/main.rs:1267` (`contact_cap: read_only   # a new contact's risk cap`); `A/contacts.rs:214` ('a fresh capped inbox'), `:219` ('one capped turn per message (§7.10)'); `crates/agent/src/communication.rs:673-677`; `crates/agent/src/frame.rs:519-521`; `crates/agent/src/peers.rs:436-438` (the `send_message` description names tag `full`); `UI/components/chat/CommunicationNotice.tsx:16-17` ('a contact's message runs one capped turn').
- Knowledge base (outside the repo) already describes the cautious path as current: `facts/theuth-a2a-contact-message-path-as-built-19251272-2026-11-10`, `facts/theuth-contact-turns-and-return-path-as-built-19251272-2026-11-10`, `projects/theuth-design-186-contact-collab-2026-11-10`, `warnings/cautious-contact-chat-cannot-read-files-default-deny-2026-11-11`; mark them superseded when 189 lands.

## 5. Compatibility

### 5.1 `peers.yaml` `modes` (and its siblings)

- Reading a file the 186 build wrote: `modes: {<export>: full}` is present whenever a full grant existed (`A/config.rs:172-175`, `skip_serializing_if = "no_full"`). `PeersFile`, `PeerEntry` and `ExportEntry` are all `deny_unknown_fields` (`A/config.rs:115-116`, `:142-143`, `:204-206`). If 189 deletes the `modes` field, `ExportEntry.tools` (written when the list is not the default, `:221-225`) or `PeerEntry.cap`, the new build **refuses the whole file**: `stage` fails, contacts and exports are dead until the file is hand-edited. So keep these as `#[serde(default, skip_serializing)]` read-and-drop fields; never write them.
- Values: `modes: {x: full}` is ignored (all grants are full). A hand-edited `modes: {x: cautious}` now also becomes full: log one line when it is read.
- Writing: the new build stops writing `modes`. A **186** build reading that file reads every grant as cautious (`A/config.rs:638-643`: only `modes[e] == Full` is full), a fail-closed downgrade, no error. A **183** build refuses a file that contains `modes` (as it does today for a 186 file with a full grant). Option: keep writing `modes: {x: full}` for each grant (rollback-proof to 186, about 6 lines); not recommended, and Sky's two Macs both update.
- `a2a.contact_cap` in `config.yaml` is `#[serde(default)]` without `deny_unknown_fields` (`A/config.rs:32-34`): safe to keep or drop.
- 189 must add **no new key** to `peers.yaml` (a 186 build refuses unknown keys in all three structs); it only stops writing one.

### 5.2 Existing cautious grants become full

- A grant with no `modes` entry is cautious (186 D2): every grant made before 186 plus every 186 grant the owner chose cautious (Sky's laptop and 333 included: the 11-11 13:27 failure was a D2-migrated grant). After 189 every export in `PeerEntry.exports` is full. No rewrite and no epoch bump (a fresh boot has no previous `Staged`; bindings are idempotent per alias and endpoint).
- What changes for the owner without being asked: a shared **chat** now takes the contact's messages as peer messages under the chat's own tools and approvals (a chat named 'Read-only diagnosis (change nothing)' stops being read-only); a shared **agent** inbox runs under the agent's own `tools:` ceiling instead of the export allowlist (`search_web`, `read_url` by default) plus the contact cap. Its stored inbox rows keep running capped (`inbox_full = 0`, S1) until the contact's next message re-admits them as full (`N:3138-3167`), so there is no gap in the safe direction.
- D10 and a contact whose card lacks `EXT_FULL` (`Contact.full_capable == false`: a 183 build, or before the first card refresh, `offers.rs:375-377`): nothing refuses at boot. At runtime that contact's 183 build sends a plain task; our task completes with a `receipt` and no text (`F:404-436`), its sender imports an empty reply, and our agent's `send_message(reply_to)` hits a 404 on its `~return` and reads 'update needed: <alias>'s theuth cannot take answers in full; update it' (`N:4393-4398`). `update_contact`'s D10 gate (`A/contacts.rs:1701-1708`) only stops new or upgraded entries; with `modes` gone redefine 'upgraded' as 'newly granted export'. Both Sky machines are 186, so this only bites an older third contact.

### 5.3 An older 186 peer sending `ContactOrigin.mode = cautious`

- `ContactOrigin` crosses engines only inside W-40 (`ContactEnvelope`, the mesh RPC `contact_deliver`); between contacts the mode travels only as the card skill tag. `mode` is `#[serde(default)]` and the struct has no `deny_unknown_fields` (`communication.rs:701-726`), so present or absent parses.
- 186 holder -> 189 owner: gets `full` or `cautious`; today the owner overrides it to cautious anyway (`N:4018`). Under W-40 option A nothing changes; under B/C ignore the field.
- 189 holder -> 186 owner: if the field is dropped or always `full`, the 186 owner lowers it itself (`N:4018` in its build) and answers with final text: works as today.
- Between contacts: keep emitting the `full` skill tag (`A/contacts.rs:540-545`): a 186 peer admits our answers on its return route only if **our** card still says `full` for that export (`F:659-661`). Keep `EXT_FULL` too (186 `full_capable`; JWS vector `A/contacts.rs:2402-2406`).

### 5.4 An older receiver that still runs a cautious grant: does the sender's final-text import stay?

Yes, for two independent reasons. (1) `apply_remote_task` `N:4454` -> `import_reply` `N:4552-4616` (and `import_question`) is the generic A2A HTTP-peer path, used by every non-contact peer. (2) An older receiver answers a cautious grant with a Completed task whose text is the turn's final text. For the sender to *reach* such a receiver it must also keep choosing the 183 shape from the card tag (`offers.rs:161-166` -> `F:226-238`); a 189 sender that always sends the full shape gets `invalid_params "unknown contextId"` from a 186 receiver with a cautious grant (`N:1516-1519` in that build). During the update window: 186 -> 189 works (our card says full); 189 -> 186 works only if the tag-based shape choice stays.

### 5.5 Mixed UI and engine (remote engines in the mesh)

- A 189 UI against a 186 engine cannot create a share unless it still sends `modes: {[exp]: 'full'}` (186 D3 refuses, `A/contacts.rs:1688-1697`). Cheap fix: keep sending it for one release; the 189 engine accepts `full` and refuses `cautious` with a clear error rather than turning it into full.
- `cancel(session, scope, reason)`: a 189 UI sending two arguments to a 186 engine means `Stop` there (reason defaults), so an interject under a contact turn cancels it, which is the ruled behaviour; a 186 UI sending `'input'` to a 189 engine is harmless if the parameter is still accepted and ignored (`crates/rpc/src/api.rs:4618-4623` already forwards the two-argument form for a plain Stop).

### 5.6 Migrations: nothing is dead, nothing to add

- `peer_exchanges.contact_mode` (0056): stays; new rows are always `full`, history keeps `cautious` (the CHECK still allows it); it is the 'this row came from a contact' flag the UI tag and the journal source read.
- `peer_exchanges.context_id`, `contact_threads` (0056): FULL conversations; stay.
- `a2a_contexts.inbox`, `inbox_identity` (0054), `inbox_full` (0056), `inbox_agent`, `inbox_ceiling` (0057): the fail-closed state (section 2); stay. Do not edit 0056/0057 in place: 186 already did that once and noted a scratch store must be remade (`docs/design/186-contact-collaboration.md:74`).
- `a2a_tasks.held` (0055): first-contact holds and FULL messages waiting for consumption; stays.
- At restart every interrupted task is already failed in the store (`N:569-571`), so queued or held cautious tasks do not survive the upgrade; a cautious turn in flight resumes capped (S7).

## 6. Proposed minimal contract

Rule: remove the choice and the cautious-only features; keep every wire name, stored name and field that an older build, an old row or an existing literal depends on.

### 6.1 Remove

| item | where | note |
|---|---|---|
| `ContactPatch.modes` | `A/contacts.rs:153-155`; TS `ContactPatch.ts` | the choice. Engine: accept `full` for one release (a mixed UI, 5.5), refuse `cautious` with 'cautious shares are gone' |
| `Contact.modes` | `A/contacts.rs:99-100`; `Contact.ts` | |
| `FirstWaiting.mode`, `.queued`, `.position` | `A/contacts.rs:125-137`; `FirstWaiting.ts` | keep `queued`/`position` only if W-40 option A keeps queued rows |
| `A2aExportStatus.mode` | `A/lib.rs:126-127`; `A2aExportStatus.ts` | |
| `Export.tools` (RPC) | `A/contacts.rs:239-244`; `Export.ts` | the engine still parses `ExportEntry.tools` from files |
| RPC `a2a_grantable_tools`, `tools::ContactGrantable`, `GrantableTool`, `CONTACT_GRANTABLE`, `contact_grantable()` | `crates/rpc/src/api.rs:2684-2686`, `:9615-9617`; `T:616-700`; `export.rs:249` | delete `GrantableTool.ts`; `UI/rpc/api.ts:799`, `:1427`; `bindings/methods.json` |
| Rust internals | `Peer.full`, `ContactGrant.full`, `no_full`, `ContactLimits.cap/.exports`, D8 guard + `contact_turn` + `turn_took_contact`, `PeerMeta.cap/tools`, `ContactFrame`, `contact_message()`, `contact_inbox()`, `QUEUED_NOTE`, `ContactsQueued`, `note_queued_exchange`, the `discover_peers` mode tag (and the `send_message` sentence naming tag `full`, `peers.rs:437`) | sections 1 and 2 |
| UI | `modeOf`, `capBinds`, `MODE_LABEL`, `modeHint`, `modeLines`, `ModeButtons`, `ModeSelect`, `useMode`, `ExportTools`/`toolsLine`/`toolNote`, cap `Select`, `contactTurn`, composer pill and 'Stop X's turn', queued strip rows | section 3 |

### 6.2 Keep, deprecated, always full

| item | after 189 |
|---|---|
| `ShareMode` (`ShareMode.ts`) | stays; the engine only writes and offers `full`; `cautious` is read from old rows and old cards |
| `PeerOrigin.contact_mode`, `PeerRecord.contact_mode` (`PeerOrigin.ts`, `PeerRecord.ts`) | `full` for a contact's message, `null` otherwise; the UI `by` tag and 'Contact · full' key on it (3.5) |
| `ContactOffer.mode` (`ContactOffer.ts`) | the peer's declared mode from its card tag (a 189 peer says `full`); keeps the sender's shape choice for older peers (5.4) |
| `ContactOrigin.{cap, tools, mode}` (`ContactOrigin.ts`), `ContactEnvelope` | wire compat for W-40 with older holders; used only under W-40 option A |
| `Contact.full_capable`, `EXT_FULL`, `card_full`, the card skill tag `full` | the version gate ('update X to share') and what older peers read; the card must not change (JWS vector) |
| `Contact.cap`, `ContactPatch.cap`, `PeerEntry.cap`, `a2a.contact_cap` | inert (a FULL share ignores it); hide the UI, keep the fields so files and old UIs parse |
| `PeerEntry.modes`, `ExportEntry.tools` | read and drop, never written |
| `CancelReason`, `Cancel.reason`, `cancel(.., reason)` (`CancelReason.ts`; `UI/rpc/api.ts:28`, `:400`) | accepted and ignored. Deleting it later costs about 28 Rust construction sites in 13 files and 7 TS call sites (`App.tsx:343`, `Composer.tsx:866,888,998,1036`, `ObservedBar.tsx:121`, `slash.ts:704`) |
| `PeerRoute.full`, `RemotePeer.full` | rename optional ('exports it may write to') |
| every DB column (5.6) | unchanged |

### 6.3 Exact TS bindings affected

Generated into `bindings/` then copied to `crates/desktop/ui/src/rpc/bindings/` by `just ts-desktop` (`justfile:113-123`):
- deleted: `GrantableTool.ts`.
- field removed: `ContactPatch.ts` (`modes`), `Contact.ts` (`modes`), `FirstWaiting.ts` (`mode`, `queued`, `position`), `A2aExportStatus.ts` (`mode`), `Export.ts` (`tools`).
- unchanged: `ShareMode.ts`, `ContactOffer.ts`, `PeerOrigin.ts`, `PeerRecord.ts`, `ContactOrigin.ts`, `ContactEnvelope.ts`, `CancelReason.ts`, `FirstContact.ts`.
- hand-written, not generated: `UI/rpc/api.ts` (`:28`, `:399-400`, `:789-803`, `:1427`), `UI/lib/rpc.ts:55-56`; `bindings/methods.json` loses `a2a_grantable_tools` (regenerated; `rpc/api.test.ts` reads it).
- Both binding trees in the working tree are stale snapshots (the copy under `src/rpc/bindings` has `A2aExportStatus.ts` without `mode` and no `ShareMode.ts`): run `just ts-desktop` before any `tsc`.
- Literals that break: `NO_PATCH` (`Contacts.tsx:45`, `modes: null`), the `W: FirstWaiting` fixture (`ContactAllowStrip.test.ts:13`), every `modes:`/`mode:` expectation in `Contacts.test.ts`. The `contact_mode: null` literals keep compiling.

## 7. Size, risks, what the ruling leaves open

### 7.1 Size estimate (changed lines, files)

| side | production | tests |
|---|---|---|
| Engine (Rust) | about 950-1,150 lines in about 22 files (`A/config.rs`, `contacts.rs`, `offers.rs`, `lib.rs`, `server.rs`; `agent` `communication.rs`, `network.rs`, `network/full.rs`, `loop_.rs`, `supervisor.rs`, `lib.rs`, `frame.rs`, `peers.rs`, `store/communication.rs`, `store/durability.rs`; `tools/registry.rs`; `rpc/api.rs`, `export.rs`; `talk`, `tui`; struct-literal fixes in `slack`, `schedule`, `session_read`); +about 420 more if W-40 is removed (`N:3799-4084`, `mesh.rs:121-165`, `communication.rs:701-758`, `api.rs:250-251,4490-4510`); +28 sites in 13 files if `Cancel.reason` is deleted | about 1,500-1,900 lines in about 9 files (`N` test module about 700, a2a `e2e_tests.rs` about 450, `S` about 300, `registry.rs` about 120, `config.rs`/`durability.rs`/`loop_.rs`/store about 100) |
| UI | about 500-600 lines in about 17 files (`lib/contacts.ts`, `Contacts.tsx` about 250, `Composer.tsx`, `composer-state.ts`, `StopButton.tsx`, `QueuedChips.tsx`, `ContactAllowStrip.tsx`, `Communication.tsx`, `PlainScreens.tsx`, `Hud.tsx`, `MessagePeer.tsx`, `SendCard.tsx`, `HarnessCards.tsx` text, `Header.tsx`, `store/sessions.ts`, `rpc/api.ts`, `lib/rpc.ts`) | about 600-800 lines in about 13 files (`Contacts.test.ts` 734 lines with about 14 tests touching modes/cap/tools; `ContactAllowStrip.test.ts`; `PeerCards.test.ts`; `Composer.test.ts`; `QueuedChips.test.ts`; `composer-state.test.ts`; `reduce.test.ts`; fixture fixes) |
| TUI | about 5 lines (`tui/src/lib.rs:401-406`) | none required |
| Docs | supersede note in `docs/design/186-...md` + 189 as-built; stale-string rewording (section 4); site: none | |
| DB | no migration, nothing dropped | |
| Bindings | 5 files lose fields, 1 deleted, `methods.json` -1 method | |

It is mostly deletion; the risk is in what must not be deleted (section 2) and in the three skew traps (5.1, 5.3, 5.4).

### 7.2 Top risks

1. **Fail-open by over-deletion.** Removing `install_contact_limits`/`INBOX_FAIL_CLOSED`, the `inbox_full` flag, the resume cap (`contact_turn_input`) or the empty-allowlist semantics of `SetAllowOnly` because 'they are cautious' brings back the three 186 review findings (uncapped after restart, no persona, offline removal). Section 2.2 is the keep list.
2. **Silent widening on upgrade** (5.2): chats shared cautiously take the contact's messages under the chat's full tools; agent exports lose the `search_web`/`read_url` allowlist; hand-edited `cautious` entries turn full. The ruling accepts 'every share is full' but nobody is told per share.
3. **peers.yaml bricking** (5.1): deleting `modes`, `ExportEntry.tools` or `PeerEntry.cap` from the serde structs makes the new build refuse a 186 file.
4. **W-40** (1.7): FULL to a remote-owned chat cannot be answered from the owner today; `N:4018` is the clamp.
5. **Wire skew**: dropping the card `full` tag or `EXT_FULL` breaks older peers' return-route admission (`F:659-661`) and the JWS vector test; dropping the tag-based shape choice breaks 189 -> 186-cautious sends (`unknown contextId`).
6. **Churn**: 28 `Cancel {..}` sites, 7 `contact_mode: None` literals, stale ts bindings, `methods.json`.
7. **Re-pointing an export no longer downgrades** (`A/contacts.rs:1903-1913`): an owner who re-points an export to another chat gives the contact full access to the new target.
8. **Old transcripts**: `<contact_message>`/`<contact_inbox/>` rows stay in history; the web `HarnessCards`/`injected.ts` and the TUI `contact_row` must keep rendering them (Sky's 11-08 rule: never raw XML).
9. **Visibility regression of a promised feature**: the 'queued: the chat is busy' note and strip rows (186 R1 item 9) disappear with the queue. A FULL message to a busy chat steers in at the next batch boundary (`L:1839-1850`), the sender's receipt stays `accepted` until consumed.

### 7.3 What the ruling leaves ambiguous

1. **W-40 (a contact's chat on another mesh engine).** Keep the capped final-text path for it (A: smallest, contradicts 'drop cautious' on that path, keeps `ContactFrame::Chat`, `contact_message`, `set_turn_limits`, `contact_replies`, queued rows), build FULL over the mesh (B: owner's `send_message(reply_to)` relayed to the holder's return route, a new RPC), or refuse (C: breaks any chat reached through a contact on another machine, e.g. a chat on xeus shared through 333). Sky's mesh is 333 + xeus, so C may not be free.
2. **Notice on migration.** Convert all grants silently (the literal ruling), or tell the owner once which shares widened.
3. **Contact cap and tools allowlist.** Both only ever bound cautious turns. The ruling says the mode choice goes; it does not say the cap and tools controls go. Leaving them is a control that does nothing; removing them is the proposal here.
4. **Re-pointing an export:** keep the grant, or revoke it (the old cautious fallback is gone).
5. **Old-contact gate (D10):** disable 'Share' for a contact without `EXT_FULL` ('update X'), or allow it and let the empty-reply failure happen.
6. **`discover_peers` mode tag** and the `send_message` sentence about tag `full`: drop, or keep always `full`.
7. **`CancelReason`:** accept-and-ignore now (proposed), or delete the field and its 28 + 7 sites in this change.
8. **Receiver-side opt-in** Sky floated at 13:47 (two-way opt-in) was answered 'full already works that way' (answers only go into the chat that wrote first); not part of the ruling.
