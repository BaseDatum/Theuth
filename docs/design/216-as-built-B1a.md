# 216 — As built: B1a wire exposure (B1's part outside `crates/plugins`)

Builder B1a of design 216 (contract `docs/design/216-contract.md` v1, §5.5–§5.7, §11.2). `crates/plugins`
is B1b's; this part provides the APIs B1b calls.

## Files

| File | What |
|---|---|
| `crates/wire/src/lib.rs` | `Exposure` (+ `as_str`); `WireEvent::{EXPOSURE, DROP, STRIP, REVIEWED_PUBLIC, CONDITIONAL, plugin_view}`; `EventDescriptor` fields `exposure, drop, strip, reviewed_public, conditional, plugin_view`; `EventDescriptor::of::<E>()`, `EventDescriptor::public(topic, version, schema)`, `EventDescriptor::external_view(&Value) -> Option<Cow<Value>>`; `WireFrame::external_view(&self) -> Option<Cow<'_, Value>>` (method, no field); a process-wide same-`TOPIC` agreement check at declaration |
| `crates/wire/src/tests.rs` | `external_view_none_without_descriptor`, `drop_and_strip_apply`, `same_topic_with_disagreeing_exposure_fails_declaration` |
| `crates/rpc-core/src/discovery.rs` | `x-theuth.events[].exposure` |
| `crates/rpc-core/tests/{catalog,discovery}.rs` | descriptor literals → `EventDescriptor::public` |
| owner crates (consts only) | `secrets` DROP `SecretRequested`; `link` DROP `LinkPairing`; `signal` DROP `LinkUrl`, STRIP `Linked.number`; `telegram` DROP `PairingLink`; `mesh` DROP `pair`; `a2a` DROP `contact_pair`; `sandbox` DROP `OpenDesktop`; `webprofile` Private; `meeting` DROP `Utterance`, STRIP `description`/`join_url` of every `EventRef` (`Upcoming.event`, `Detected.event.event`, `Detected.event.alternatives`); `voice` DROP `DictationText`, `DictionarySuggestion`, `SpeechAudio`; Internal: `rpc.stream`, `sessionsync.follow-state`, the six `artifact.*-requested/-resolved` and two `browser.element-form-*` topics; `sessionsync` test twins Internal like production |
| `crates/agent/src/lib.rs` | `MessageDelivered.contact: bool` (`#[serde(default)]`); `AgentEvent` STRIP `ContactFileChanged.file.{name,path}`, `CONDITIONAL` (contact rule + the router's meeting-session rule), `plugin_view` (drops `text` when `contact`) |
| `crates/agent/src/communication.rs` | `PeerDelivered` `CONDITIONAL` + `plugin_view` (drops `text`, `files` when `origin.contact_mode` is set); tests `contact_message_text_hidden_from_plugins`, `contact_peer_delivered_body_hidden` |
| `crates/secrets/src/{vault,lib}.rs` | `PLUGINS_PREFIX`, `writable_name` accepts `plugins/<seg>/…`, `PLUGINS_GRANTS_KEY`, `Vault::plugins_grants_key()`; test `plugins_rows_are_versioned_hidden_and_unexpandable` |
| `crates/tools/src/mcp.rs` | `pub expand`, `pub expand_map`, `pub risk_from_hints` (called by `annotated`), `pub gate_for(mcp_dir, server, tool)`, `PLUGIN_SERVER_PREFIX`, `plugin_server_key(name)`; `wanted` skips servers named `plugin.*` with a warning; tests `risk_from_hints_matches_annotations`, `plugin_named_servers_refused_and_plugin_gates_read` |
| `crates/tools/src/{registry,lib}.rs` | hide key of `ToolSource::Plugin { name }` = `mcp_server_key("plugin.<name>")`; `InvokeHumanOnly { call, requester }` (re-exported); test `invoke_human_only_follows_policy_and_asks_human_only` |
| `crates/theuth/tests/wire_exposure.rs` + `wire_exposure.txt` | snapshot, name lint, catalog/contract agreement, same-`TOPIC` agreement |
| `crates/rpc-compose/tests/wire_golden.rs` + `fixtures/wire/plugins.{events,ui}.json` | the two topics' families; `agent.events.json` golden gains `"contact": false` |
| `crates/theuth/tests/fixtures/wire/openrpc-v1.json` | the accept (B0's additions + `exposure` keys + `MessageDelivered.contact`) |

## Deviations from the contract

1. **`plugins/grants-key` is not a vault row.** Vault rows replicate (`secrets::store::TABLES` = `vault`,
   `vault_current`, `secret_requests`, every row), so a vault row would make one engine's approvals verify on
   every paired engine (D9). `Vault::plugins_grants_key()` keeps the key in the local-only `meta` table under
   the name `PLUGINS_GRANTS_KEY`, sealed with this engine's key (as `mesh_key`), created on first use under
   `keylock`. Signature as contracted. `PLUGINS_PREFIX` rows (`plugins/<name>/<key>`) still follow `share/`'s
   rules for any future plugin vault rows. No hand-off needed.
2. **Hide key for plugin tools** is `mcp:plugin.<name>` (`mcp_server_key(plugin_server_key(name))`), so the
   existing per-session "hide server" and `mcp/config.yaml` gate lines treat a plugin as the reserved server
   `plugin.<name>` (design §8). The contract said "the key `plugin.<name>`"; this is that key in the registry's
   existing namespace.
3. **`InvokeHumanOnly` shape:** `pub struct InvokeHumanOnly { pub call: ToolCall, pub requester: String }`
   (`requester` e.g. `"plugin turns"`). Policy is the tool's own (allow runs, deny refuses); an `Ask` raises
   `RequestHumanOnly`. `ApprovalRequested` has no title field, so the card's args gain
   `requested_by: "plugin turns wants to run http_request"` (UIs may render it as the title — hand-off to B3 if
   wanted). Uses `harness: false` (the builtin switch applies to brokered calls).
4. **`MessageDelivered.contact` is `false` at every producer today.** Contact (186) bodies arrive as
   `PeerDelivered` (where the rule applies through `origin.contact_mode`); no `MessageDelivered` producer has a
   `PeerOrigin`. The field and view exist so a future producer only has to set it.
5. **Same-`TOPIC` agreement** is enforced process-wide in `wire` (`register_wire_event_with` refuses a second
   impl of a topic whose exposure consts differ, in any runtime). `sessionsync`'s `#[cfg(test)]` twins cannot
   be seen by a `theuth` integration test, so `wire_exposure.rs::same_topic_impls_must_agree` proves the check
   with a public twin of `browser.web-profile-changed`.
6. **Snapshot rewrite** is `UPDATE_WIRE_EXPOSURE=1 cargo test -p theuth --test wire_exposure`, not
   `just wire-schema-accept` (the justfile is in no builder's row). Hand-off to Z: add that line to
   `wire-schema-accept`.
7. `plugin_view` runs only when `CONDITIONAL` is non-empty (an owner that overrides it must list it), so
   `external_view` borrows for most frames.
8. Files outside the B1 row touched mechanically because the contract's field/struct changes break them:
   `crates/rpc-core/tests/{catalog,discovery}.rs` (descriptor literals), `crates/rpc-core/src/discovery.rs`
   (`exposure` key, contract §5.5), MessageDelivered constructors/patterns in `crates/agent/src/{supervisor.rs,
   supervisor/{acp_resume,activity,child_waits}.rs, acp_child/observed.rs}`, `crates/slack/src/channel.rs`
   (test), `crates/telegram/src/worker.rs` (tests), `crates/tui/src/{model/agents.rs, model/tests.rs,
   model/components.rs, components/peers.rs}` (B2's crate; announced), the `agent.events` golden, `crates/desktop/ui/src/store/reduce.test.ts` (one literal, orchestrator-assigned), `crates/rpc-compose/tests/wire_parity_inc5.rs` (plugins families).

## Hand-offs

- Z: `justfile` `wire-schema-accept` should also run `UPDATE_WIRE_EXPOSURE=1 cargo test -p theuth --test wire_exposure`.
- B1b: router order per contract §13: `frame.external_view()` (cache per frame) → remote-session drop → grants →
  meeting-session strip (the field list is in `AgentEvent::CONDITIONAL`) → `tools::secrets::redact_value` → cap.
  Tools: `tools::mcp::{expand, expand_map, risk_from_hints, gate_for, plugin_server_key}`; brokered calls via
  `tools::InvokeHumanOnly`; grants key via `secrets::handle(rt)` → `vault.plugins_grants_key()`.

## Gates run

- `cargo test -p wire` — 12 passed (incl. `external_view_none_without_descriptor`, `drop_and_strip_apply`, the agreement test).
- `cargo check --tests -p wire -p rpc-core -p secrets -p tools -p agent -p link -p signal -p telegram -p mesh -p a2a -p sandbox -p webprofile -p meeting -p voice -p artifact -p browser -p sessionsync -p slack -p rpc-compose -p tui -p theuth` — green (one `plugins` dead-code warning, B1b's).
- `cargo test -p theuth --test wire_exposure` (first run, before the accept): `same_topic_impls_must_agree` passed; the name lint found `secrets.events: SecretsRotation.key`, `SecretsRotation.peers.has_key` (reviewed: `KeyInfo` holds a key id hash, no material; now `REVIEWED_PUBLIC`); `catalog_matches_the_tracked_contract` failed only because the fixture had no `exposure` yet (topic sets equal).
- `just wire-schema` + `cargo test -p theuth --test wire_schema_snapshot`: classifier changes are `Additive` (19 `plugin_*` methods, `plugins.events`, `plugins.ui`), `Nonbreaking` (doc lines), and `ReviewRequired` only for the enum extensions (`AttentionKind::plugin`, `ChannelVia::plugin`), the new `x-theuth.events[].exposure` keys, `MessageDelivered.contact` and the `x-theuth/plugins` set. Reviewed, accepted (fixture copied from the fresh `bindings/openrpc.json`), committed in c21a37291.

## Gates after the link fix

The `theuth` link failure (`_btls_sys_SSL_CTX_add_cert_compression_alg` from a cached `wreq`) was the mbx object cache; rebuilt with
the plain rustup cargo after `cargo clean -p btls-sys -p wreq`. Theuth-linking targets use `/Users/sky/.cargo/bin/cargo`.

- `cargo test -p theuth --test wire_exposure --test wire_schema_snapshot` — 4 + 1 passed; `wire_exposure.txt` generated, reviewed
  (every DROP/STRIP/Private/Internal/CONDITIONAL line matches design §7.3), committed in f7cc9547.
- `cargo test -p secrets --lib plugins_rows` 1 passed; `-p tools --lib mcp` 37 passed; `-p tools --lib invoke_human_only` 1 passed;
  `-p agent --lib contact_` 35 passed.
- `cargo clippy -p wire -p secrets -p tools -p agent --no-deps -- -D warnings` — green.
- `bun run check` in `crates/desktop/ui` — green (bindings already regenerated in the worktree; I did not rerun `just ts-desktop`).

## Not verified

- `cargo test -p rpc-compose --test wire_golden --test wire_parity_inc5`: does not compile while `crates/plugins` (B1b, in flight)
  fails (`host::Launch` field / arity errors). The plugins families and fixtures are in; rerun once B1b's crate compiles.
- `cargo check -p plugins --features ts`: fails first in `roster` (`#[ts(optional)]` on non-`Option` fields in
  `crates/roster/src/attention.rs`, pre-existing, not from 216 B1a) — hand-off to Z.
