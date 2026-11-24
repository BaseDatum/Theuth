# 210 as built — CERT-STATE (Settings › Sharing: unknown vs no certificate, attach on open, Certificates)

Design: `docs/design/210-public-share.md` §5.2 ("Settings › Sharing open" is a need), §7.4 item 4, §7.6. Builds on
`210-as-built-UI-M2.md`.

## Bug

A held `sky.tun.zip` with a valid `*.sky.tun.zip` certificate read "Reserved · No certificate yet" while the Edge row
read "Not contacted yet". `names::list` takes `HeldName.state` only from the last `welcome`
(`Engine::welcome_names`), which is `None` while detached; `heldStateText` printed `null` as "No certificate yet";
nothing on the page made the engine attach; the Certificates section was a stub.

## Changes

| path | what |
|---|---|
| `crates/share/src/engine.rs` | `SETTINGS_ATTACH_HOLD` (60 s); fields `settings_open: AtomicBool`, `last_state: Mutex<Option<(EdgeState, Option<i64>)>>`; `Engine::settings_open()`; `refresh_need` keeps the need while `settings_open` and the switch is on; `Attachment::state` records `last_state`; `installed_not_after(name_host)`; tests `settings_open_needs_the_edge_only_with_the_switch_on`, `cert_event_replaces_the_installed_chain` (+ list asserts state `valid` and `cert_not_after_ms`) |
| `crates/share/src/names.rs` | `HeldName.cert_not_after_ms: Option<u64>`: the installed chain's `notAfter` (as its welcome recorded it), else the last welcome's `NameInfo.not_after`; `None` for released rows |
| `crates/share/src/rpc_share.rs` | `share_names_list` calls `settings_open()` first (non-blocking), then lists |
| `crates/desktop/ui/src/components/settings/Sharing.tsx` | `heldStateText`: `null`/`undefined` → "Certificate state unknown — edge not contacted"; `certText`; `CertificatesSection` (one row per non-released held name: `<name>.D · *.<name>.D`, state, "certificate until <date>"; pending-shares line kept); `DomainsTab` invalidates the names query when `share.status` turns `attached`; TODO removed |
| `crates/desktop/ui/src/components/settings/Sharing.test.ts` | +3 tests (state text, Certificates rows, detached→attached refetch) |

## Behaviour

- **Attach on open.** Settings › Sharing reads `share_names_list` on mount. With the switch on and an edge client
  running, `settings_open()` sets the client's need (§5.2) and returns at once; the need holds 60 s after the first
  read, then `refresh_need` drops it unless a share needs the edge. Switch off: nothing is dialled, and turning the
  switch off drops the Settings need at the next `refresh_need`. Attaching sends `hello` only: no name, record or slot
  (§5.2 lazy allocation).
- **Refetch.** `welcome` sets the link before the client reports `attached`, so the `share.status@1 attached` event
  is the UI's cue to read the names again. When the engine was already attached before the page opened, the call
  republishes the last `share.status`, so the Edge row stops saying "Not contacted yet".
- **Unknown ≠ none.** `state == null` means no welcome on this engine; "No certificate yet" stays only for a non-null
  edge state the UI does not map.

## Wire

**Changed**: `HeldName` gained `cert_not_after_ms` (`number | null`) and the `share_names_list` description changed.
`crates/theuth/tests/fixtures/wire/openrpc-v1.json` is NOT updated here; `wire_schema_snapshot` stays red until the
fixture is accepted (`just wire-schema-accept`) and the desktop bindings regenerated (`just ts-desktop`).

## Verification

- `cargo test -p share --lib -- settings_open_needs cert_event_replaces names::` → 3 passed.
- `crates/desktop/ui`: `bunx vitest run src/components/settings/Sharing.test.ts` → 20 passed; `bun run check` → clean.
  The worktree had no generated bindings: they were copied from the main checkout (same base commit) with
  `cert_not_after_ms` added by hand to the local, git-ignored `HeldName` binding.

## Not built / doubts

- `cargo check -p theuth` and the `export_bindings` run were not run (cold target dir).
- The Settings need is a 60 s hold, not tied to the page closing: no "page closed" signal exists.
- `cert_not_after_ms` comes from the welcome that installed the chain, not from parsing the leaf.
