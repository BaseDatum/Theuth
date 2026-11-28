# 210 as built — TERMS-FIRST (terms before any claim; the owner's press is the consent)

Design: `docs/design/210-public-share.md` §5.2, §7.2, §7.6; layout of Settings › Sharing from
`docs/design/213-sharing-hub.md` §2 (Public shares / Domains tabs, unchanged here).

## The bug

Settings › Sharing › reserve a name → approval card → Allow → toast `terms_required`. `share_names_reserve` raised the
consent card and claimed at the edge without checking the edge's terms; the edge refuses claims and CSR posts with
`403 terms_required` while the session's `hello` lacks the live `terms_rev` (§7.6). Settings had no way to accept the
terms before a publish was refused. A second, latent bug: the refusal text names the edge by **domain**
(`accept the terms of tun.zip first …`), the desktop sent that domain to `share_terms_accept`, and the engine stored
it under the domain while every reader keys by the edge's **endpoint id** — so an accepted revision was never found.

## Files

| path | what |
|---|---|
| `crates/share/src/engine.rs` | `Consent::of_press`, `Consent::press_or_ask`, `PRESS`; `terms_precheck`, `terms_ready`, `edge_terms`; `current_terms` accepts an edge without terms; `accept_terms` stores a domain under the endpoint id; tests |
| `crates/share/src/names.rs` | `reserve(engine, name, consent)` checks terms before taking consent; `rotate` checks terms before the new key |
| `crates/share/src/model.rs` | `EdgeTerms` (wire) |
| `crates/share/src/rpc_share.rs` | `share_terms_get`; publish / settings_set / terms_accept / names_reserve / names_transfer use `press_or_ask` |
| `crates/desktop/ui/src/components/settings/Sharing.tsx` | `TermsSection` (Public shares tab), `useShareError`, press-path labels |
| `crates/desktop/ui/src/components/settings/shared.tsx` | `useAction` takes `onError` |
| `crates/desktop/ui/src/store/public-shares.ts` | `termsRefusal`, `termsToast` |

## Behaviour

- **Terms first.** `Engine::terms_precheck` (edge configured, attached, `info.terms_rev` accepted; the same refusal
  text as publish, `accept the terms of <domain> first (<url>, revision <rev>): …`) runs before any consent in
  `names::reserve`; no card is raised while terms are unaccepted. After consent, `terms_ready` re-sends `hello` with
  the accepted revision when the session's welcome said `terms_ok: false`, so the claim is not refused. `rotate` (a CSR
  post, also terms-gated at the edge) runs `terms_ready` before a new name key replaces the vault row. Transfer is not
  terms-gated at the edge (`POST …/transfer`); the receiving mesh claims with `PUT`, i.e. `reserve`, which is checked.
  The agent's publish already prechecks (`publish_precheck`, unchanged). An edge with an empty `terms_rev` requires
  nothing (as the edge's own `terms()` check).
- **`share_terms_get`** (read-only, additive): `Option<EdgeTerms { edge, domain, url, rev, accepted }>`; `null` while
  sharing is off or no edge is configured (nothing dials then), else attaches like `share_names_check`.
  `share_terms_accept` takes `EdgeTerms.edge` (endpoint id) or the attached edge's domain.
- **Settings › Sharing › Domains › Terms** (moved here 2026-11-27, directly above name reservation; it is not on Public shares) always shows the edge's terms once sharing is on: link + revision +
  Accept while not accepted, `Accepted revision X` after. A refusal parsed from any Sharing action (reserve, primary,
  rotate, release, transfer, the publish form, the switch) — the engine's text or a raw `terms_required` — opens the
  Accept step (`askTerms` when the text carries url/revision, and a refetch of `share_terms_get`) and toasts
  `Accept the terms of <edge> first: Settings › Sharing › Domains › Terms`; the raw code is never shown. A reserve refusal also renders that sentence inline under the Reserve control.

## Decision: the owner's press is the consent

Owner: "I reserve a name in Settings and then have to approve an action I just intentionally took."

What `rpc_core::owner(ext)` proves: only that the connection is **not a share-link guest**. It admits every local
WebSocket that presented the bearer token (`<data_dir>/token`: the desktop, the CLI, an agent's unapproved shell that
read the file), every paired device over Noise, and the **internal peer** (`RpcPeer::internal()`: the MCP owner
console, the ACP agent half, the TUI, and every call another engine of the family forwards via `rpc_forward`).

The rpc layer already has the narrowest marker for "a human pressed this": 171 §12.1 invariant 1, as
`cells_guard::press` uses it — an authenticated peer that is neither a guest nor the internal peer. `Consent::of_press`
applies exactly that (and refuses a closed peer; it does not accept the `Forwarded` exception cells makes, so a
family-forwarded call asks). No new marker was added: `X-Theuth-Client` is self-declared and proves nothing.

- Press (desktop/app socket, paired device): `share_publish` (Settings form), `share_settings_set` (on / edge),
  `share_terms_accept`, `share_names_reserve`, `share_names_transfer` act on the click; the share's
  `approved_by` is `{approval_id: <fresh v7 uuid>, surface: "press"}`, so the local-approval binding (FIX4 #1) works.
- Not a press (internal peer: MCP console, ACP half, TUI, forwarded): the human-only approval row as before.
- The agent's tool path (`share_publish` tool, `HUMAN_ONLY`) is unchanged: the registry raises the human-only row and
  the body acts only on `Consent::of_call`; it never sees the RPC press.
- Release, primary and rotate were never carded; release and rotate keep the UI's own confirm line only.
- Stop / Stop all and switching off stay ungated.
- Residual, unchanged in kind: a process holding the bearer token is treated as the owner's client. It could already
  resolve its own human-only row through `resolve_approval` (the 210 §7.2 residual documented on `Approval`'s
  `Resolve`), so skipping the card for it grants nothing the card withheld; the guarantee still rests on shell
  commands being approved or sandboxed (Settings warns when sharing is on and approvals are off). The TUI, being the
  internal peer, still gets cards: it cannot be told apart from the agent halves sharing that peer.

## Tests

- `engine::tests::reserve_refuses_unaccepted_terms_before_the_consent` — terms text, consent closure never called,
  `edge_terms`, accept by domain, consent called once terms are accepted, `None` while off.
- `engine::tests::only_the_owners_client_presses_without_a_card` — local socket and paired device press; internal,
  guest and missing peer do not; a non-press raises the human-only row.
- `engine::tests::the_agent_publish_tool_still_needs_the_approval_row` — the tool refuses outside the registry's row.
- `Sharing.test.ts` — Terms row before any publish and Accept; off / old engine; reserve terms refusal → Accept step
  and plain toast; raw `terms_required` → the same toast. `public-shares.test.ts` — `termsRefusal`, `termsToast`.
