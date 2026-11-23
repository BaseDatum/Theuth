# 210 as built — FIX-GUARDS (review findings M3, M4)

Design: `docs/design/210-public-share.md` rev 8, §7.2 (F-m7, owner-only human-only cards). Builds on
`210-as-built-CONSENT1.md`. Scope: `crates/computer`, `crates/browser`, `crates/signal`.

## Findings

| id | status | how |
|---|---|---|
| M3 web client in the user's browser passes `guard_self` | fixed | `computer::tools::is_own`: `ax::is_self` **or** the window's `AxFront.document_url` is on one of theuth's own origins. `drives_self`/`guard_self` take the origins; front and hit-tested window both checked |
| M3 `own_origin` with `rpc.listen: 0.0.0.0` misses LAN IP / mDNS name | fixed | `browser::OwnOrigins::contains`: an unspecified listen address (`0.0.0.0`, `::`) matches **every** host on the listener's port; loopback listen matches loopback IPs, `localhost`, `*.localhost` |
| M3 TODO(210-M2) "every configured web-client origin" | fixed (partly, see Doubts) | `OwnOrigins::from_config` = `rpc.listen` (default `127.0.0.1:7777`) + `link.relay` host/port (default `wss://relay.theuth.io`; the relay host serves the paired web app, `crates/relay/src/web.rs`). TODO removed |
| M3 `computer_open` unguarded | fixed | a `url` on an own origin → `refuse_while_human_only()` (only asks the approval actor when the URL matches) |
| M3 `computer_clipboard_write` unguarded | fixed | `refuse_while_human_only()` first: refused outright while any human-only card is pending |
| M4 Signal non-owner sessions not marked | fixed | `Channel::note_thread(session, number)` → `tools::mark_non_owner_thread(session, true)` when `number` is not the owner (or no owner is set), at every `threads.bind` (inbound message, `/resume`), at `bind_agent` for the desk session, and at every `TurnStarted` of a bound session (the registry keeps the mark in memory). Same shape as Slack `note_thread` / Telegram |
| M4 reaction resolves with no owner check | fixed | `approvals::Entry.human_only`; `Pending::by_reaction` returns the `&Entry`; on a human-only entry a non-owner reaction is not resolved and the sender gets "only the owner can answer this". `/approve`/`/deny` were already `OWNER_ONLY` |
| M4 card text | fixed | `approvals::message` appends "Only the owner can answer this." for `human_only` |

## Public API

```rust
// crate browser (new module `origins`, re-exported at the crate root)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnOrigins { .. }
impl OwnOrigins {
    pub fn from_config(rt: &Runtime) -> Self;               // rpc.listen + link.relay
    pub fn new(listen: &str, relay: Option<&str>) -> Self;  // "host:port", ws(s)/http(s) origin
    pub fn contains(&self, url: &str) -> bool;              // http/https/ws/wss only
}

// crate computer (crate-private)
pub struct Ctx { .., pub origins: OwnOrigins }              // built from config at plugin start
pub(crate) fn is_own(f: &AxFront, origins: &OwnOrigins) -> bool;
pub(crate) fn drives_self(front, target, origins: &OwnOrigins) -> bool;
pub(crate) async fn guard_self(origins: &OwnOrigins, at: Option<(i32, i32)>) -> Result<(), String>;

// crate signal
pub struct approvals::Entry { .., pub human_only: bool }
pub fn approvals::Pending::by_reaction(&self, number: &str, ts: u64) -> Option<&Entry>; // was Option<ApprovalId>
```

Removed: `browser::tools::{rpc_listen, own_origin}` (private; replaced by `OwnOrigins`).

## Deviations

- `ax::is_self` itself is unchanged (pid/bundle only; it is also the "skip theuth" filter of
  `context::header`). The origin check is a separate `tools::is_own` used by the input guard, so the screen
  context header still names a browser showing the web client.
- `computer_open { app: "theuth" }` is not refused: bringing theuth forward is not input; any click/key that
  follows is refused by `guard_self`.
- `computer_clipboard_write` refuses while *any* human-only card is pending, whatever the target (the brief's
  "refuse while pending").

## Not built

- TODO(210-M2): a web-client origin served elsewhere than the RPC listener or the link relay (no config key
  exists). `OwnOrigins::new` is the place to add it.
- A browser window whose `document_url` the accessibility tree does not report (some Chromium builds without
  `AXManualAccessibility`, Firefox) is not recognised as the web client; `guard_self` then sees only a browser.
- Signal: no test that the registry receives the non-owner mark (`mark_non_owner_thread` goes to the global
  registry actor; registering one in the signal tests would race other tests). The refusal itself is covered by
  `tools::registry::tests::human_only_refuses_deny_app_press_and_non_owner_threads`.

## Doubts

- `rpc.listen` on a specific LAN IP matches only that IP, not mDNS names resolving to it (no DNS lookup in the
  guard).
- `link.relay` is matched even when Link is disabled (default relay host): refusing an acting call on
  `relay.theuth.io` while a human-only card is pending is harmless.
- `rpc.listen` with port 0 (ephemeral) matches nothing useful; the bound port is not known to these crates.

## Tests

- `browser::origins::tests::{loopback_listener_and_relay, unspecified_listener_matches_every_host_on_its_port, specific_listener_matches_its_own_ip}`
- `browser::tools::tests::own_origins_follow_rpc_listen_and_link_relay` (replaces
  `own_origin_matches_the_listener_under_loopback_names`)
- `computer::tools::tests::input_to_theuth_is_refused_while_a_human_only_approval_is_pending`: extended with
  a Safari front on a LAN-IP web client and the relay web app as hit target, and, while pending,
  `computer_open` of `http://my-mac.local:7777/` (listen `0.0.0.0:7777`) and `computer_clipboard_write` refused.
- `signal::channel::tests::human_only_card_refuses_a_non_owner_reaction`: card in a friend's thread carries the
  owner line; the friend's 👍 resolves nothing and is answered "only the owner can answer this".
- `signal::approvals::tests::reaction_needs_number_and_ts` updated for the new return type.

## Verification

Run 1 (13:03–13:07, under the lock): `CARGO_BUILD_JOBS=6 cargo test -p computer -p browser -p signal`

- All three crates and their test targets compiled, no errors, no warnings from these crates (only the
  `block v0.1.6` future-incompat notice).
- `browser` lib: 54 passed, 1 failed: `own_origins_follow_rpc_listen_and_link_relay` panicked in kameo spawn
  (`Runtime::new` needs a Tokio runtime; it was a plain `#[test]`). Fixed by making it `#[tokio::test]`.
  Cargo stopped there, so the `computer` and `signal` tests did not run.

Run 2 (`cargo test --no-fail-fast -p computer -p browser -p signal`): **unverified: lock**. Waited from
13:07 to 13:33 for `/tmp/theuth-build.lock` (held by other sessions, last by `cargo test -p slack`), then
stopped per the house rules.

`cargo clippy -p computer -p browser -p signal --all-targets -- -D warnings`: **unverified: lock** (not started).

Commands to run:

```sh
until mkdir /tmp/theuth-build.lock 2>/dev/null; do sleep 20; done; trap 'rmdir /tmp/theuth-build.lock' EXIT; CARGO_BUILD_JOBS=6 cargo test --no-fail-fast -p computer -p browser -p signal; rmdir /tmp/theuth-build.lock; trap - EXIT
until mkdir /tmp/theuth-build.lock 2>/dev/null; do sleep 20; done; trap 'rmdir /tmp/theuth-build.lock' EXIT; CARGO_BUILD_JOBS=6 cargo clippy -p computer -p browser -p signal --all-targets -- -D warnings; rmdir /tmp/theuth-build.lock; trap - EXIT
```

Cargo.lock: no new packages from this change (`browser` already depended on `url`; `computer` on `browser`).
