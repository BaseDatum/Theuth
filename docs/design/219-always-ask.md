# 219 — Always ask: a per-tool approval override nothing can bypass

Status: contract (2026-11-25). Builders implement exactly the names below; change this file only through the orchestrator.

## 1. What and why

The owner wants to mark individual tools as **always ask**. A call to a tool marked this way always raises an approval card for a human to answer, even when:
- approvals are off globally (`tools.approvals: false`),
- the chat has its approvals pin set to off,
- a routine runs with `approvals: false`,
- the call comes from autoresearch, an advisor, a desk or a subagent,
- the tool policy says Allow for the tool's risk (for example, a read-only tool).

It is meant for dangerous tools, or for places where mistakes are likely. It applies to built-in tools, runtime functions called from `python`, and MCP tools.

There are two scopes:
- **Harness-wide**, kept permanently in `~/.theuth/config.yaml` under `tools.always_ask`.
- **Per chat**, stored in the sessions database. Subagents inherit it the same way hides and pins are inherited.

## 2. Semantics

### 2.1 Keys
An entry is a string:
- a **registered tool name**, exactly as the registry knows it (`shell`, `write_file`, `browser_click`, `kubernetes__pods_delete`, or `<server>__<tool>` when names clash);
- or **`mcp:<server>`**, which matches every tool of that MCP server. This is the existing `mcp_server_key(server)` form from `crates/tools/src/registry.rs`.

Matching rules:
- Matching is exact. There are no globs.
- An entry may name a tool that is not registered right now, for example one from an MCP server that is offline. It is stored and takes effect once that tool registers.
- **Loop-local tools cannot be added.** These are `python`, `todo`, `set_status`, `advise`, `consult` and the endless tools. `set_always_ask` rejects them with error `always_ask: <name> runs inside the agent loop and is never gated; mark the runtime functions it calls instead`. The runtime functions called from `python` (`shell`, `write_file`, …) go through the registry, so marking those works.

### 2.2 The decision (single choke point)
The rule lives in `ToolRegistry::effective()` (`crates/tools/src/registry.rs`). Every registry path goes through it: `Invoke`, `HarnessInvoke`, `AppInvoke`, `ExternalApproval` and `InspectTool`.

```rust
// after the capped/owner_absent Deny checks and after `mentions`, next to the human_only early return:
if human_only || self.always_ask_applies(session, name) { return action.max(PolicyAction::Ask); }
```

`always_ask_applies(session, name)` is true when `name` or `mcp_server_key(<server of name>)` is in any of these:
- the global set;
- the session's own set;
- the set of any ancestor in `parents`, using the same walk as `hidden_chain` / `pinned`.

Consequences:
- **Deny still wins.** Caps, contact-turn caps, `allow_only` and a policy of `Deny` all return earlier or stay Deny, because `max` keeps Deny.
- **When no owner is present** (a contact turn, a non-owner chat thread, i.e. `owner_absent`), an always-ask call is **denied**. There is nobody to ask. The deny reason is `always ask: <tool> needs the owner's approval and the owner is not in this conversation`.
- **App press (`admit_app`).** A `UserActivation` press does not count as the approval for an always-ask call. It must raise a real card, the same way the `human_only` arm does.
- **Timeout.** An unattended always-ask call waits `approval_timeout_secs`, then is denied as timed out, like any other card. This is intended: a routine using such a tool stalls, and the owner sees the card.

### 2.3 The card is human-answered
- `ApprovalRequested` gains `#[serde(default, skip_serializing_if = "std::ops::Not::not")] pub always_ask: bool`. It is threaded from `RequestApproval` (new field `always_ask: bool`, default false) through `Approval::request` and `Pending`.
- In `approval.rs`, a `ResolveBy { by != Answerer::Human }` on a row with `always_ask == true` is refused, using the same guard as `human_only`.
- **The double.** `crates/double/src/actor.rs` skips the card when `m.human_only || m.always_ask || !cfg.capture.approvals`. The double never answers an always-ask card.
- `always_ask` does **not** imply `human_only`. Any human surface may answer: desktop, TUI, Slack, Telegram, Signal, iOS. Owner-only restrictions stay tied to `human_only`.
- `HonourAnswer` replay on resume is unchanged. A stored answer for such a row can only have come from a human.

### 2.4 Who may change the lists
- **Adding** an entry (either scope) is never gated. It makes things safer.
- **Removing** an entry (either scope) needs the human. This uses the existing "human press" rule (design 171 §12.1, as used by `crates/share/src/rpc_share.rs` after commit `54bb6d99b`):
  - A call that is authenticated, not a share guest, and not from the internal connection (desktop/iOS UI press) runs directly.
  - Any other caller (internal connection: the agent's halves, MCP console, ACP, TUI, forwarded calls) gets `ask_human_only_on(...)`, a human-only card with tool `always_ask.remove` and args `{ "tool": <name>, "scope": "chat" | "harness" }`. A deny or timeout returns error `always_ask: removing <name> was not approved`.
- **No agent tool or runtime function wraps `set_always_ask`.**
- **Config file guard.** Writes by the built-in file tools to `<data_dir>/config.yaml` and `<data_dir>/mcp/config.yaml` are treated as always ask. This covers `write_file`, `edit`, `move_text`, `copy_text`, `indent`, and any other fs tool that writes a path. It is implemented as a per-call check in `effective()`'s caller path: a helper `fn protected_write(name, args) -> bool` in the registry, OR'ed into the always-ask condition, so the card carries `always_ask: true`. `shell` and `python` file writes cannot be detected. This is a documented residual, mitigated by marking `shell` always-ask.

## 3. Storage

### 3.1 Harness-wide
- `ToolsConfig` gets `#[serde(default)] pub always_ask: Vec<String>`. YAML key: `tools.always_ask`.
- Registry field: `always_ask: HashSet<String>`, loaded at `registry::start`.
- Persist with `runtime::write_config_value(&self.config_path, "tools", "always_ask", <json array sorted>)`, writing the file first and applying in memory second. This is the `SetBuiltinsDisabled` pattern. A failed write returns `always_ask: config.yaml not written: <err>`.

### 3.2 Per chat
- Registry field: `always_ask_session: HashMap<SessionId, HashSet<String>>`.
- Journal: `async fn always_ask(&self, _session: SessionId, _names: Vec<String>) {}`, a default no-op on `tools::Journal`. The agent's impl calls `Store::set_session_always_ask(id, &names)`.
- Migration `crates/agent/migrations/0073_session_always_ask.sql`:
  - `ALTER TABLE sessions ADD COLUMN always_ask TEXT NULL;`, a JSON array. An empty list is stored as NULL, mirroring `hidden_tools`.
  - Bump `SESSIONS_WIRE_SCHEMA` 70 → 71 and fix every test that pins it.
- `Store::session_always_ask(id) -> Vec<String>` is the reader.
- Restore: `Supervisor::inherit_approvals` sends `RestoreAlwaysAsk { session, names }` next to `RestoreHidden`.

## 4. Registry messages (`crates/tools/src/registry.rs`)
```rust
pub struct SetAlwaysAsk { pub session: Option<SessionId>, pub tool: String, pub on: bool }   // -> Result<AlwaysAskState, AlwaysAskError>
pub struct RestoreAlwaysAsk { pub session: SessionId, pub names: Vec<String> }              // -> ()
pub struct GetAlwaysAsk { pub session: Option<SessionId> }                                  // -> AlwaysAskState

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AlwaysAskState {
    pub global: Vec<String>,     // sorted
    pub session: Vec<String>,    // this chat's own entries, sorted (empty when session is None)
    pub inherited: Vec<String>,  // entries from ancestor chats, sorted, not in `session`
}

#[derive(Debug, thiserror::Error)]
pub enum AlwaysAskError { LoopLocal(String), NotWritten(String), Empty }   // Display strings as in §2.1/§3.1; Empty = "always_ask: tool name is empty"
```

- `SetAlwaysAsk` does **not** do the human check. That is the RPC layer's job (§2.4).
- `SetAlwaysAsk` with `on` equal to the current state is a no-op that returns the state.

## 5. RPC (`crates/agent/src/rpc_tooling.rs`, `AgentToolingApi`, audience owner)
```rust
#[method(name = "get_always_ask", with_extensions)]
async fn get_always_ask(&self, session: Option<SessionId>) -> RpcResult<AlwaysAskState>;
#[method(name = "set_always_ask", with_extensions)]
async fn set_always_ask(&self, session: Option<SessionId>, tool: String, on: bool) -> RpcResult<AlwaysAskState>;
```

- Both forward to the session's owner engine with `forward::route`, like `set_mcp_hidden`. A `session: None` call runs on the engine that receives it.
- Before applying, `set_always_ask(.., on: false)` runs the §2.4 human check, but only when the entry is actually present.
- Errors map to `ErrorObject` invalid-params for `LoopLocal` / `Empty`, and internal for `NotWritten` and not-approved.
- Accept the wire fixture (`just wire-schema-accept`, under the build lock) and regenerate bindings (`just ts-desktop`).

## 6. Clients

### 6.1 Desktop (`crates/desktop/ui/src`)
- **Approval card** (`components/chat/ApprovalCard.tsx`). When `approval.always_ask`:
  - show a Tag/badge `always ask` in the title row;
  - hide `<DoubleChip>`;
  - keep Allow / Deny.
- **Approval card for any other call** (not always_ask). Add a small tertiary action "Always ask for <tool>". It calls `set_always_ask(session, tool, true)`. Only the per-chat scope is offered here. No confirm is needed because adding is safe.
- **Settings → Agent → Approvals** (`components/settings/Approvals.tsx`). Below the existing switch, add a Section "Always ask, even when approvals are off":
  - the global list with a remove control per row (calls `set_always_ask(null, tool, false)`);
  - an add field with tool-name autocomplete from `list_builtins` + `list_mcp` (server rows offer `mcp:<server>`).
  - Copy: "These tools always need your approval, in every chat, routine and subagent. The double never answers them."
- **Tool rows** (`components/library/Tools.tsx` `ToolRow`, `components/library/Builtins.tsx` `ToolRow`, plus a server-level control on the MCP server row). Add a compact control "Always ask" with three states: `off | this chat | always`.
  - `this chat` is only offered when a `session` prop exists.
  - It reads `get_always_ask(session)`. A tool inherited from a parent shows as `this chat (inherited)` and is read-only there.
- **Composer approval shield popover and the ⋯ session menu.** When `session.length + inherited.length + global.length > 0`, show a line `Always ask: N tools` that opens the chat Tools sheet. If approvals are off and N > 0, the shield's tooltip says `Approvals off · N tools always ask`.
- **Slash command** (`components/chat/slash.ts`): `/always-ask <tool> [on|off] [always]`.
  - Default `on`.
  - `always` means harness-wide (`session: null`).
  - A bare `/always-ask` lists the state.
  - A not-yet-created chat stores the value in drafts, like `/approvals`.
- Update the tests that pin row ids, mount order and fixtures (`ApprovalCard.test.ts` fixture gets `always_ask`, `SessionMenu.test.ts`, `ComposerMeta.test.ts`, `registry.test.ts` only if a tab is added — **do not add a new Settings tab**).
- No emoji (`no-emoji.test.ts`).

### 6.2 TUI (`crates/tui`)
- **Approval modal** (`ui.rs`). When `always_ask`, render `approval: {tool} ({risk}) · always ask`.
- **Slash command** `/always-ask <tool> [on|off] [always]`. It needs a list entry in `model/slash.rs`, a dispatch arm in `model/submission.rs`, an order entry, and a handler next to `approvals_cmd`.
  - It adds `Command::SetAlwaysAsk { session, tool, on }` and `Command::GetAlwaysAsk { session }`.
  - The reply is `UiEvent::AlwaysAsk(AlwaysAskState)`, shown as a status line.
  - A bare `/always-ask` lists the state.
  - Removal from the TUI goes through the internal connection, so it raises the human-only card. That is expected.
- **`/mcp` view.** Key `!` toggles always-ask for the selected tool or server in this chat. `Shift`+`!` is not needed. Show a `!` marker on rows that are always-ask.
- **Fixtures.** Update the literals that construct `ApprovalRequested` (`model/tests.rs`, `components/double.rs`, `components/secrets.rs`).

### 6.3 Chat bridges (Slack, Telegram, Signal)
- Compile fixes only: add `always_ask: false` to literals.
- The card text adds ` · always ask` when set, wherever the bridge renders the tool line. No new commands.

## 7. Checks (the orchestrator runs the gate)
- `cargo test -p tools` covers registry tests for:
  - (a) global entry forces Ask with approvals off;
  - (b) session entry forces Ask with the pin off and is inherited by a child;
  - (c) `mcp:<server>` matches;
  - (d) a cap Deny still wins;
  - (e) owner_absent denies;
  - (f) `admit_app` does not admit;
  - (g) the `ResolveBy` double is refused on an always_ask row;
  - (h) loop-local names rejected;
  - (i) write_file to `<data_dir>/config.yaml` raises an always_ask card.
- `cargo test -p double` covers that the double skips always_ask cards.
- `cargo test -p agent` covers the store round-trip, the migration and the schema pin at 71, and restore at spawn.
- `cargo test -p theuth --test wire_schema_snapshot` and `cargo test -p rpc-compose`.
- `cargo test -p tui`, plus the slack/telegram/signal crates compile.
- Desktop: `bun run check` and `bunx vitest run` for the touched files.
- `cargo clippy` on the touched crates with `-D warnings`.

## 8. Residuals (documented, not fixed)
- `shell` and `python` can still write config.yaml or the database, and can read the RPC token and call `resolve_approval` / `set_always_ask`. Mark `shell` always-ask to close this in practice.
- Tools run by an external harness (claude-acp, omp, opencode) reach `ExternalApproval` with a title, not a stable tool name. A name entry rarely matches them.
- Loop-local tools are never gated (§2.1).
