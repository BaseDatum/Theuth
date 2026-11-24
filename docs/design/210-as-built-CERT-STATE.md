# 210 as built — CERT-STATE (Settings › Sharing: unknown vs no certificate, names status, Certificates)

Design: `docs/design/210-public-share.md` §5.2 (presence), §5.3 (control API), §7.4 item 4, §7.6. Builds on
`210-as-built-UI-M2.md`.

## Bug

A held `sky.tun.zip` with a valid `*.sky.tun.zip` certificate read "Reserved · No certificate yet" while the Edge row
read "Not contacted yet". `names::list` took `HeldName.state` only from the last `welcome`
(`Engine::welcome_names`), which is `None` while detached; `heldStateText` printed `null` as "No certificate yet";
the Certificates section was a stub.

## Design change (second pass)

The first pass made `share_names_list` attach (Settings open as a need). Replaced: `hello` is a full attach (an
attached engine, may evict another, consumes admission). Settings now uses a read-only, one-shot
`POST /v1/names/status` that runs `hello`'s checks and attaches nothing.

## Changes

| path | what |
|---|---|
| `crates/share-proto/src/lib.rs` | `paths::NAMES_STATUS = "/v1/names/status"` |
| `crates/share-proto/src/api.rs` | `NamesStatusRequest {v, client, delegation}`, `NamesStatus {now_ms, mesh, names: Vec<NameInfo>}` |
| `crates/share-edge/src/control.rs` | `hello`'s pre-admission checks split out as `vet_delegation` (protocol, `min_client`, `Delegation::verify` against the tunnel's engine key and the edge identity, `mesh != engine`, one mesh per connection) and `vet_mesh` (bans of mesh/key/net, rekeyed chain → `409 mesh_rekeyed {to}`, revoked); `welcome.names` built by `held_name_infos` (names + slot queue positions); new handler `names_status` uses all three, no session; `hello` calls the same functions (paused/draining check still between them) |
| `crates/share-edge/src/{config,state,admin}.rs`, `deploy/share-edge/config.example.yaml` | `limits.names_status_per_net_min` (default 12), keyed limiter `Edge::names_status_rate` per `Tunnel::net`, swept with the others, restart-required on reload |
| `crates/share-edge/tests/mesh.rs` | `names_status_reads_the_names_without_attaching` |
| `crates/share/src/edge.rs` | `Control::names_status`; `edge::names_status(dialer, delegation, ticket)`: dial, `GET /v1/info` (edge id must match the ticket), the delegation a `hello` would carry, the status, close |
| `crates/share/src/engine.rs` | `Engine::names_status()` (one-shot, caches `(at_ms, edge, domain, names)`), `detached_names()` (switch on only; cache under `NAMES_STATUS_FRESH_MS` = 30 s, else `names_status()` within `NAMES_STATUS_WAIT` = 5 s; any failure → `None`); `installed_not_after`; first-pass `settings_open`, `SETTINGS_ATTACH_HOLD`, the settings term in `refresh_need`, `last_state` and its republish removed; test `detached_names_list_uses_the_names_status_and_never_attaches` (replaces `settings_open_needs_the_edge_only_with_the_switch_on`); `cert_event_replaces_the_installed_chain` also checks the listed state and `cert_not_after_ms` |
| `crates/share/src/names.rs` | `list`: the welcome while attached, else `detached_names()`; `HeldName.cert_not_after_ms`: installed chain's `notAfter`, else the edge's (`welcome` or names status); `None` for released rows |
| `crates/share/src/rpc_share.rs` | `share_names_list` doc; the handler only lists again |
| `crates/desktop/ui/src/components/settings/Sharing.tsx` | `heldStateText`: `null`/`undefined` → "Certificate state unknown — edge not contacted"; `certText`; `CertificatesSection` (one row per non-released held name: `<name>.D · *.<name>.D`, state, "certificate until <date>"; the pending-shares line kept); TODO removed; the first pass's refetch-on-`attached` effect removed (the detached list already carries the states) |
| `crates/desktop/ui/src/components/settings/Sharing.test.ts` | +3 tests (state text, Certificates rows, Domains with unknown and known names) |
| `docs/design/210-public-share.md` | §5.2 presence: Settings does not attach; §5.3: the endpoint beside `/v1/info` |

## Behaviour

- **Edge.** `POST /v1/names/status` is answered on a connection without `hello`, like `/v1/info`, and counts against
  the per-tunnel control rate and `names_status_per_net_min`. Refusals are `hello`'s pre-admission ones. No
  `attach` transaction, admission or invite, engine-key quota, engine count, tunnel replacement, `last_ms`, session
  or mesh binding. The pre-`hello` permit and `hello_timeout_secs` still bound the connection; the engine closes it.
- **Engine.** Detached and switch on: one status per 30 s at most, each bounded by 5 s; `need` is never set. An edge
  without the route answers `404 not_found`: states stay `None`, shown as unknown. Attached: the live welcome, as
  before.
- **Unknown ≠ none.** `state == null` means the edge's view is unknown; "No certificate yet" stays only for a
  non-null edge state the UI does not map.

## Wire

**Changed** (RPC schema): `HeldName.cert_not_after_ms` (`number | null`) and the `share_names_list` description.
`crates/theuth/tests/fixtures/wire/openrpc-v1.json` is NOT updated here; `wire_schema_snapshot` stays red until the
fixture is accepted (`just wire-schema-accept`) and the desktop bindings regenerated (`just ts-desktop`). The edge
protocol gains one route and two types (additive; an old edge answers `404`).

## Verification

- `cargo test -p share-edge --test mesh -- names_status an_engine_revoked a_banned_name`
- `cargo test -p share --lib -- detached_names cert_event_replaces names::`
- `crates/desktop/ui`: `bunx vitest run src/components/settings/Sharing.test.ts` → 20 passed; `bun run check` →
  clean. The worktree had no generated bindings: copied from the main checkout (same base commit) with
  `cert_not_after_ms` added by hand to the local, git-ignored `HeldName` binding.

## Not built / doubts

- `cargo check -p theuth` and `export_bindings` not run.
- A failed status is not cached: an unreachable edge costs each detached list call up to 5 s.
- The status needs a delegation: on a non-holder engine that is a `share.delegate` call to the lease holder, inside
  the 5 s bound.
