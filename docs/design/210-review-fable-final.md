**Final review: `docs/design/210-public-share.md` rev 7**

## Verdict

**Ready to build P0 after small fixes.** There are no blockers and the settled owner decisions are designed consistently. The fixes P0 needs are F-M1 and a rewrite of the P0 cell as a checklist. §5.7 and the two-CA accounting need a rev 7.1 patch (F-M2 to F-M7) before P1 workers start, because a builder would otherwise invent the rekey and key-creation state machines.

Not read: about 150 characters of the P0 gates cell and about 1,900 characters of the P1 row (L1881–L1882 exceed 2,000 characters and my reader cut them), and §11.7–§11.12 only skimmed.

## First review's B/M findings

| # | Status | Note |
|---|---|---|
| B1 | fixed | §7.2 table, share RPC table and the stated RPC-token residual are in the text. `osascript` from an unapproved shell falls under the same residual. `browser_*` on theuth's own web client is unaddressed (F-m7). |
| B2 | fixed within owner decisions | Lazy allocation, names cap, invite share and byte budgets are present. Remaining gaps are F-M8. |
| M1 | fixed | §6.0; draft-01 §6.3 confirms a record at the parent with `policy=wildcard` covers `*.dept.example.com`. |
| M2 | fixed | `acme-tls/1` reset, four CAA lines, `require_caa`. GTS enforcement is still unverified and correctly sits in P0 check 5. |
| M3 | fixed | Claim corrected at L1501; panic boundary in §6.1. |
| M4 | fixed | Per-connection transport config plus edge counter. I could not check the iroh 1.4.1 line cites locally. |
| M5 | fixed | `dial_alpn` without `join_network`; the remaining inference is flagged for P0. |
| M6 | fixed | Canonicalisation, per-connect port and PID check, `Location` rewrite. A TOCTOU remains (F-m8). |
| M7 | superseded | A new machine needs no handover. Dropping the recovery code opens F-M6. |
| M8 | fixed (text) | TUI, channels, badge and notifications are in §7.4 and §10. |
| M9 | fixed | §6.6, §9.1, §9.2. |
| M10 | partial | Flow is drawn and `terms_rev` is explicit, but first contact is circular (F-M1). |
| M11 | partial, regressed | Rev 6 cut P1; rev 7 added mesh identity, vault rows, rekey, two CAs and multi-engine routing (F-M9). |

## New findings

**Blockers:** none.

**F-M1. First `hello` is circular, and clock skew has no diagnosis (§5.3 L441, L446; §5.7 L624, L643).**
- `Delegation.domain` comes "from `welcome`", but `welcome` only answers a `hello` that already carries a valid delegation. The ticket carries no domain.
- A hub clock more than 5 minutes ahead makes every delegation of the mesh fail with nothing to show the user.
- Fix: add a pre-hello `GET /v1/info → {domain, edge, now_ms, min_client, terms_url, terms_rev, full}` under the pre-`hello` caps, or drop `domain` from the body. Add `403 {code: delegation_not_yet_valid, now_ms}`, have the hub sign with `issued_ms = min(own, edge now)`, and show "clock is off".

**F-M2. The removal trigger misses machines the edge never saw (§5.7 L658–L661).**
- The lease holder "compares the engines the edge has seen (`GET /v1/mesh`) with the live peers". A removed machine that never attached still holds `mesh/share_key` from the vault. It can sign its own delegation, take labels, or send a `Rekey`.
- Fix: rekey on every tombstone written after `mesh/share_key` exists, at every edge where the mesh is registered. `GET /v1/mesh` only supplies the revoke list.

**F-M3. The rekey state machine is not specified against the vault's real semantics (§5.7 L663–L670).**
- **Vault rotation can fail or lag.** `Mesh::unpair` only warns when the rotation fails (`crates/mesh/src/lib.rs#L984-L987`), and the lease holder may see the tombstone before the new vault key is current. Step 1's "sealed under the rotated vault key, so the removed machine never reads it" is then false.
- **`set_if_version` is a local compare-and-set.** `write_row`'s own comment says two engines in one sync gap mint the same `(name, version)` id and per-column LWW picks one ciphertext (`crates/secrets/src/vault.rs#L894-L896`). L615 credits the dual-hub guard to the CAS; it is the lease predicate (`rpc_support.rs#L9-L11`).
- **Two holders can exist.** `lease.rs#L449` tests two holders after a partition heal. Both can write version v+1 and both can send a `Rekey`. The edge takes the first, LWW may keep the other, and the mesh is locked out with the winning secret overwritten.
- **Crash and offline cases are missing.** Nothing covers a crash between steps 1 and 2, or the edge being unreachable at removal.
- Fix:
  - Gate step 1 on the rotation record for that removal being complete.
  - Write the new key, wait for `synced()`, re-read, and send the `Rekey` for the row's settled value.
  - Keep old and pending secrets in a local 0600 stash until `GET /v1/mesh` equals the vault key; heal by chaining a second `Rekey`.
  - Persist a pending-rekey row and retry it.

**F-M4. Name-key creation is not lease-holder-only, so the same race applies (§6.2 step 1, L860–L865).**
- "A race loser re-reads, uses the winner's key and posts nothing" holds only inside one process. Two machines publishing first in one sync gap both succeed locally and post different CSRs.
- The edge keeps the first CSR and LWW may keep the other row. Step 5's mismatch rule (L884–L886) then waits forever, and on a first issuance there is no old key pair to serve.
- The edge's answer to a different CSR without `rotate` for a name that already has a valid certificate is unspecified (L869 covers only a running job).
- Fix: create through the lease holder (`share.name_key.ensure`), as rotation already is. Put the SPKI hash of the edge's CSR in `welcome.names[]`. Define `409 {code: csr_mismatch, spki}`. Add a heal rule: mismatch for more than N minutes with no rotation pending makes the holder rotate.

**F-M5. Lease-holder duties contradict the per-machine switch and per-machine edge (D15 L137; §5.2 L406–L410; §8.1 L1372–L1375; §5.7 L658).**
- The hub must dial the edge on lease gain, on every peer event and hourly, even when its own "Allow public sharing" is off. The doc elsewhere says nothing dials until the switch is on and that attachment is only "while needed".
- The edge choice is a non-replicated per-machine setting, so the hub does not know which edges to rekey at.
- Fix: keep a replicated `share_edges` row per (edge, domain) the mesh registered at. State that holder duties run regardless of the local switch, only when such a row exists and only on removal events, not hourly. Disclose it in Settings.

**F-M6. A lost rekey race has no basis for recovery (§5.7 L676–L680; D20).**
- Thief and owner both hold the old key, so `admin recover` "on evidence the operator accepts" has no cryptographic evidence to weigh in a system without accounts.
- The window runs from compromise to unpair, which is days for a stolen laptop.
- Rev 6's recovery code was dropped on the grounds that the key cannot be lost; it also served as this tiebreak.
- Fix: at mesh registration store `recovery_pub` at the edge, the hash of a code shown once to the owner and never written to the vault. `admin recover` then requires it. Owner question 1.

**F-M7. Two-CA accounting has three gaps (§6.0 L751–L758; §6.3; §6.5).**
- **Loss of GTS is not covered for existing names.** Renewals "stay with that CA". If GTS or Path P is lost for longer than the renewal slack, every GTS name becomes a new certificate at Let's Encrypt: 45 per week, and at most 1,200 names by the Porkbun record cap. Above roughly 200 names Path Z cannot carry the fleet. The doc discusses only new names (L714–L716).
- **The overflow rule exports an attack.** A job overflows after 1 h of spent GTS budget. A burst on the open half of GTS (40 per hour) then drains Let's Encrypt's 22 open slots per week and writes Porkbun records.
- **GTS's quota counts all orders.** Renewals and rotations draw on the same 100 per hour; the budget table calls it a new-name budget and sets no renewal priority.
- Fix:
  - Make the first fallback GTS over `dns-01` (already configurable as `cas.gts.validation`; CAA method changes with it), then Let's Encrypt. Owner question 2.
  - Overflow only on GTS failure or when Let's Encrypt's ETA is shorter.
  - Renewals take the GTS bucket first.
  - Move Let's Encrypt names back to GTS at renewal to free records.

**F-M8. Open-day bounds leave four gaps (§9.4, §8.1 L1438–L1464).**
- **Names cap.** Rows are created before any certificate. IPv6 limits stop at /48 × 4, so one hosting /32 (65,536 /48s) can fill `names_max` in hours.
- **Eviction is defeated.** A modified client registers a route, which sets `first_live_ms`, and the name is never evicted as unpublished.
- **Relayed bucket.** One relay-only actor still takes every paced slot of 20 per day.
- **Egress.** `egress_alarm` is an alarm, not a cap; meshes are free and the per-network budgets add up without limit.
- **Phishing.** A reserved name is claimable at once and gets a wildcard certificate in about 30 s. The interstitial does not bind a modified client, and one Safe Browsing flag hits all of `tun.zip`.
- Fix:
  - A global paced bucket for open new names (for example 500 per day).
  - IPv6 /32 and ASN tiers.
  - Count a name against the cap only once it has a certificate slot, and evict uncertified names after 1 h.
  - Set `first_live_ms` on first visitor bytes, not on a route.
  - A hard egress throttle for non-invited meshes at 100% of pace.
  - A brand blocklist file for reserved names.
  - Safe Browsing and Search Console monitoring as a launch prerequisite and a brake trigger.

**F-M9. P0 is not a crisp list and P1 is larger than the P1 that M11 objected to (§10 L1881–L1882).**
- Both are single cells over 2,000 characters with no item ids, pass criteria or order.
- P0 needs owner inputs it does not name: GTS staging EAB, a Porkbun key, API access on `tun.zip`, and staging records in the live zone.
- Fix: number P0-1 to P0-n, each with a pass or fail line and what it decides. Slice P1 into ordered milestones with unchanged scope:
  1. Single machine, Path P, consent UI.
  2. Mesh identity and rekey.
  3. Path Z and the slot queue.
  4. Admission, limits and operations, then launch.
- A builder must currently invent:
  - both sqlite DDLs;
  - the slot-queue fairness and ETA algorithm across two CAs;
  - where bytes are metered and how bandwidth is shaped;
  - the reconnect state machine;
  - the admin-socket protocol;
  - everything in F-M1 to F-M5.

**Minor**

- **F-m1.** `Delegation.serial` (L628) is checked nowhere, and "increasing per mesh" needs replicated state across hub handover. Define it (edge refuses a lower serial per engine) or drop it.
- **F-m2.** The "12 h" cliff (L654) is really 12 to 24 h. The doc never says whether a partitioned machine elects itself holder and self-issues. Say which from `lease.rs`.
- **F-m3.** `require_caa` refuses to start without the edge's `accounturi` in CAA, but the account exists only after a first start (L1497, L2152). Add `theuth-share-edge acme register`, which prints the URIs.
- **F-m4.** P0 check 2 (PSL interaction, L737) cannot be tested by a spike, because nobody controls a listed suffix. Answer it from GTS support or the Baseline Requirements text, or drop it as a P0 gate.
- **F-m5.** Porkbun is confirmed as registrar and DNS host, but the text is still conditional. Remove the "if registered at Porkbun", "registered elsewhere" and "tell the maintainers where" lines (§13 L2117–L2121, §6.4 L936). The dedicated-account advice now means moving the domain; owner question 3.
- **F-m6.** Stale per-key language and one wrong path:
  - L203 "the key's … budgets" should say mesh.
  - L1758 "ban by name or key".
  - `limits.delegation_max_ttl` (L644, L1771) is top-level in the config (L1405).
  - "Invite-first" at L46 and the §14 B2 row is historical but reads as current.
  - §6.5 (c) still argues for GTS as the overflow CA before reversing itself (L999–L1006).
  - §13 lists the PSL pull request without Q10's hold.
- **F-m7.** `browser_*` shares the user's sign-in. If theuth's web client is reachable there, the agent can click Publish outside the `computer_*` guard. Refuse the engine's own origin while a human-only card is pending.
- **F-m8.** `dir` canonicalises and then `ServeDir` opens: a symlink swap can race. Serve the canonical path or open with no-follow.
- **F-m9.** `certs_per_net_week: 4`, "at most half from the open share" (L1441, L1806) contradicts "an invited mesh leaves the per-network counts" (L1854).
- **F-m10.** Does Pebble offer `dns-persist-01`? If not, the test rig covers Path P only through the fake `Ca`. Say so.

**Nits**

- **F-N1.** Every `vault.rs` anchor is one line early: the function is L458–L481, `set_if_version` L704–L718, prefixes L249–L268, `list` L618–L627.
- **F-N2.** `crates/secrets/src/lib.rs#L906` is a log line; `replica::attach` is at L900.
- **F-N3.** `share-proto` needs a base64 encoder and big-array serde for `[u8; 64]`; "serde, postcard, iroh-base, iroh-tickets only" is not quite true.
- **F-N4.** Porkbun's docs list DNSSEC DS management, restore points and a sandbox (`pk1_sb_` keys). Use the sandbox for the first Path Z test. "No API call turns the toggle on" (L1511) looks outdated.
- **F-N5.** Readability. A core spec of about 700 lines is reachable:
  - Cut each D-row to two sentences (D2, D5, D14, D18 and D21 run 1,500–2,000 characters and repeat §4.3, §5.2, §5.7).
  - Move §4.6 and §6.1's source notes to an appendix.
  - Move §8 to `deploy/share-edge/README.md`.
  - Move §11, §6.6, §6.8, §14, the status line and §1's revision paragraphs to an appendix or changelog.
  - Strip "rev N" narration from normative text.
  - State lazy allocation, Porkbun key scoping and `421` once each; they appear four to six times.

## Verified correct

- **Porkbun** ([OpenAPI spec](https://porkbun.com/api/json/v3/spec), [DNS guide](https://porkbun.com/llms/dns), [getting started](https://porkbun.com/llms/guides/getting-started)): 2,500 records per zone (`ZONE_RECORD_LIMIT`); API access opt-in per domain; keys restrictable to domains, IPs and the `dns` scope; minimum TTL "typically 600"; 20 requests per 2 s in observe mode.
- **GTS** ([announcement](https://developers.google.com/public-key-infrastructure/updates/september2026-dns-persist-01)): record format and 8-day reuse as the doc states.
- **CAA label**: [draft-01](https://www.ietf.org/archive/id/draft-ietf-acme-dns-persist-01.txt) §8.1 registers the method label `dns-persist-01`, so the CAA value is right. GTS's CPS says only that it "may" limit issuance per RFC 8657, so enforcement stays a P0 check.
- **421 and h2 coalescing**: coalescing needs certificate cover plus the same IP, both true within one name and never across meshes. Routing on SNI with a `421` on `:authority` at the engine is correct, and the HTTP/1.1 fallback is sound.
- **Code anchors**: `rpc_support.rs#L9-L23`, `rotation.rs#L854-L876`, `keys.rs#L481-L491`, `lib.rs#L974-L990`, `store.rs#L161-L178`, `rpc_forward.rs`, and `net.rs` L546, L755 and L851 all match.
- **Vault row unit**: one JSON blob in one ciphertext column is a sound unit; the chain staying out of the vault is right.

## Remaining owner questions

1. **Lost rekey race.** Restore an owner-held recovery code as the tiebreak, or accept the operator's judgement with no evidence (F-M6)?
2. **Fallback when Path P fails or GTS is lost.** GTS over `dns-01` first, or Let's Encrypt directly at about 45 names a week (F-M7)?
3. **Porkbun account.** Move `tun.zip` to a dedicated account, or keep it where it is with a key scoped to the domain, `dns` and source IPs?
4. **Hub with sharing off.** May it dial the edge for rekey duties, with a disclosure (F-M5)?
5. **§12 Q8–Q18.** The proposed defaults are acceptable for P0; only Q16 (model-initiated publishing) and Q17 (log retention) need an answer before P1 UI and terms work.

## Ready for P0

- [ ] F-M1 patched: `/v1/info` or no `domain` in the delegation, plus the clock error code.
- [ ] P0 cell rewritten as numbered items with pass criteria; check 2 re-scoped (F-m4).
- [ ] Owner provides: GTS staging and production EAB, a scoped Porkbun key (sandbox key first), API access switched on for `tun.zip`, and permission to put staging persist and CAA records in the live zone.
- [ ] `acme register` bootstrap decided (F-m3).
- [ ] P0 measures the vault row between two engines, including one deliberate concurrent first publish (F-M4), and a rekey with the hub's switch off (F-M5).
- [ ] Before P1: §5.7 rev 7.1 (F-M2 to F-M6), §6 accounting (F-M7), §9.4 additions (F-M8), P1 milestones (F-M9).