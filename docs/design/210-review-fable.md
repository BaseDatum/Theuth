**Review of `docs/design/210-public-share.md` (rev 5)**

## Verdict

**Needs revision.** The architecture is sound: blind SNI edge, standalone service, iroh transport, CSR from the engine, per-name wildcard. Every code anchor and crate version I spot-checked is accurate. But the "human-only" guarantee is bypassable as written, and `admission: open` has no bounds that survive a cheap attacker or even modest organic growth. A primary-source fact from September 2026 could also delete most of the DNS machinery and deserves an owner decision before P0.

## Blocker

**B1. Human-only approval is not owner-only and not unbypassable (D10, G3, §7.2 L753–766, §5.1 L302–303).**
- **Slack thread users.** §7.2 says "a reaction there is a human", but Slack lets "owner or the card's thread user" answer (`crates/slack/src/channel.rs#L1077-L1092`; test `approval_in_a_bound_thread_is_a_card_the_thread_user_can_click`, L4046). An allowed non-owner Slack user can ask for and approve publishing the owner's localhost.
- **Any RPC caller is `Answerer::Human`.** `Resolve` is always recorded as human (`crates/tools/src/approval.rs#L355-L357`, `crates/tasks/src/rpc_hub.rs#L40-L49`), and the RPC token is a file in `data_dir` (`crates/rpc-compose/src/bootstrap.rs#L116`). The §7.2 guard on `ResolveBy` does not touch this path.
- **Computer-use on theuth's own window.** `is_self` exists (`crates/computer/src/ax.rs#L89-L95`) but is used only at `context.rs#L86`. With `tools.approvals: false` or a pinned session, the agent can click "Publish" via `computer_act` or `applescript`. G3's "no policy switch… can grant it" is therefore false.
- **Settings path.** Publish, extend and flip-to-public from Settings (L748, `created_by: "settings"`) have no approval. `approved_by` "always human + approval id" contradicts that, and the share RPC surface is never specified.
- **Fix:** restrict human-only cards to `click.user == owner` in Slack/Telegram/Signal and refuse `share_publish` in non-owner threads. Make `computer_*`/`applescript` refuse theuth's own windows (at least while a human-only card is pending). Add an RPC table for share, and route the Settings publish through the same approval row. State the residual honestly: an agent with unapproved shell can read the token.

**B2. `admission: open` on the public edge is not bounded (§6.5, §9.4, §6.4).**
- **Slot exhaustion.** 45 Let's Encrypt slots per week at 4 per network means about 12 IPs take the whole week's budget, forever. IPv6 /48s and cloud IPv4s are cheap, and a FIFO queue "capped per network" does not help.
- **Relayed bucket.** A client can force relay-only, so one actor drains the global "20 relayed new keys per day" and locks out every UDP-blocked newcomer.
- **Implicit allocation.** The random name, its two DNS records and its CSR happen on first attach (§5.2 L313, §6.2 step 1), i.e. on flipping the switch, before any publish or terms acceptance. Every curious user burns a slot, and reserved-name users burn two.
- **Record ceiling not stated.** §6.4 L593 says "a paid plan"; Cloudflare Pro and Business are capped at 3,500 records per zone ([docs](https://developers.cloudflare.com/dns/manage-dns-records/)), about 1,745 held names in total.
- **No byte or bandwidth quota anywhere.** A blind, free, account-less TLS relay is an open proxy for anything that speaks TLS.
- **Fix:**
  - Allocate lazily: an explicit `POST /v1/names` at first publish or reserve, after terms.
  - Add a global names cap with a visible "edge full" state and eviction of never-published names.
  - Reserve a share of weekly slots for invite-lifted keys.
  - Add per-key and per-network byte budgets and rate caps.
  - Gate reserved names by key age.
  - Document launching `invite` and flipping `open` live once the numbers hold.

## Major

**M1. Missed alternative: Google Trust Services supports `dns-persist-01` with `policy=wildcard` since 2026-09-28** ([GTS announcement](https://developers.google.com/public-key-infrastructure/updates/september2026-dns-persist-01)).
- One static TXT at `_validation-persist.tun.zip` naming the edge's GTS account would authorise every `{<name>.D, *.<name>.D}` order with zero DNS writes.
- With no challenge TXT there is no empty non-terminal, so one `*.D` record suffices. That removes D17, the DNS token on the edge, zone-update, record quotas, deSEC write limits and the serialised authorisations.
- Let's Encrypt has not shipped it; [CertKit](https://www.certkit.io/blog/whats-taking-dns-persist-01-so-long) estimates mid-2027.
- Three things to verify in P0:
  - GTS accepts the parent record for a subdomain order.
  - Whether a PSL listing of `D` stops label pruning at `<name>.D`, which would conflict with §6.6.
  - `instant-acme` 0.8.5 has no such `ChallengeType` variant, only `Unknown(String)`.
- **Fix:** add a §11 entry and decide (Q1). At minimum keep `DnsZone`/`Ca` shaped so this can drop in.

**M2. Tenant-driven issuance through the blind edge is not in the threat table (§9.2).**
- The edge forwards a CA's TLS-ALPN-01 connection (ALPN `acme-tls/1`) to the tenant like any ClientHello. A modified client can order certificates for `x1.<name>.D…` from Let's Encrypt with its own ACME account and burn `D`'s 50-per-week budget, which is "counted across all accounts".
- Only the CAA `accounturi` in §8.2 stops this, and the doc presents that as token-theft defence. The GTS line has no `accounturi`.
- **Fix:**
  - The edge resets any ClientHello offering `acme-tls/1`; rustls's `Acceptor` exposes ALPN, and P3 custom domains divert it anyway.
  - Add `validationmethods=dns-01` to CAA.
  - Refuse to start in `open` mode unless the CAA is present.

**M3. "A stolen DNS token alone cannot get a certificate" (§8.2 L960, §9.2 L1174) is wrong for Cloudflare.**
- A zone DNS-edit token can rewrite the CAA record or repoint `<name>.D`. It holds only with deSEC RRset policies limited to TXT/CNAME.
- zone-update's Cloudflare client also calls `GET /zones?name=` (needs Zone:Read) and does `zones.remove(0)`, which panics on an empty result ([source](https://raw.githubusercontent.com/tarka/zone-update/main/src/cloudflare/mod.rs)).
- **Fix:** correct the claim, name the exact token permissions, and contain panics in the job worker.

**M4. Stream limit is on the wrong side (§5.3 L385).**
- QUIC's `max_concurrent_bidi_streams` limits incoming streams ([quinn docs](https://docs.rs/quinn/latest/quinn/struct.TransportConfig.html)). Visitor streams are opened by the edge, so the cap is set by the engine's mesh endpoint.
- That endpoint is built with `presets::Minimal` and no transport config (`crates/mesh/src/net.rs#L546-L554`; no `TransportConfig` anywhere in `crates/mesh`). I did not confirm what `presets::Minimal` yields on iroh 1.4; quinn's own default is 100, not 256.
- Receive windows and keep-alive are likewise the mesh's and shared with every mesh ALPN.
- **Fix:** set the config in `Net::bind` (and assess mesh impact) or use a dedicated endpoint. Enforce 256 with an edge-side counter and an `open_bi` timeout.

**M5. `join_network` side effects are undisclosed (§5.3 L344, §5.5 privacy).**
- It is one-way (`joined.swap(true)`), connects to the relay, and with discovery on publishes the endpoint to n0 pkarr/DNS and starts mDNS (`crates/mesh/src/net.rs#L755-L784`).
- A lone engine that flips the sharing switch becomes discoverable until restart; switching off does not undo it.
- **Fix:** dial the ticket's direct and relay addresses without publishing, or disclose it.

**M6. Source safety gaps (§5.6).**
- **`dir` symlinks.** `ServeDir` follows them, so a link inside the shared tree to `~/.aws` passes the dot-segment rule. Canonicalise and check containment per request.
- **`port` is checked only at publish time.** A no-expiry share can later point at whatever rebinds the port, including theuth's own ephemeral listeners. Check per upstream connect.
- **The own-listener registry is opt-in.** It misses child processes theuth starts (headless browser debug port, MCP servers, sandbox VNC). Also compare the listener's PID against theuth's process tree.
- **Redirects.** `Host` rewritten to `localhost:<port>` makes upstream `Location: http://localhost:…` leak to visitors; rewrite `Location`.

**M7. No name recovery or handover (§5.2 L330).**
- "Transfer is release plus a claim after the cooldown" means a new laptop or reinstall loses `sky.tun.zip` for at least 30–90 days and then races strangers. A lost key cannot even release.
- **Fix:** an old-key-signed handover (`POST /v1/names/{name}/transfer {to}`) that revokes the old certificate and skips the cooldown. State the lost-key policy.

**M8. G4 ("cannot miss it, on every client") is unmet.**
- **TUI.** `crates/tui` exists with an approvals model, but §7.4 lists no status line or stop command, and §10 omits `tui`.
- **Chat channels.** Slack, Telegram and Signal get the approval but no persistent "live" indicator or stop command. Their crates are missing from §10, though the `human_only` rendering touches all three.
- **Hidden window.** The badge is "P1 if cheap", and the tray icon may be hidden. With the window closed and the engine running, nothing shows.
- **Fix:** make the hidden-window badge and an OS notification on publish and first visitor P1. Add TUI and channel surfaces: a pinned message and a `/share stop` command.

**M9. Pre-PSL isolation is weaker than §6.6 says.**
- All tenants are same-site: `SameSite=Lax/Strict` gives no CSRF protection between `a.D` and `b.D`. Password managers that match on registrable domain will offer one tenant's credentials on another's page.
- The PSL may never accept the entry; the doc quotes the "thousands of users" bar itself.
- **Fix:** add both to §9.2 and the disclosure, and treat "never listed" as the planning case.

**M10. First-run flow is undefined.**
- Terms are shown at first publish (§7.3 L791), but name, DNS records and certificate order happen at switch-on.
- §7.3 covers `queued` but not `ordering` (1–3 minutes with two serialised authorisations).
- `terms_rev` is derived from a URL change (L939), so edited content at the same URL is missed.
- **Fix:** draw switch-on → terms → name choice → certificate → publish. Make `terms_rev` explicit and send the accepted revision in `hello`.

**M11. P1 is several projects (§10).**
- Hidden work:
  - DNS-to-sqlite reconciliation and orphan TXT cleanup after crashes.
  - Client reconnect and resume state machine.
  - Abuse operations and legal (terms, notice handling, IP-log retention).
  - Backup and restore drills for `iroh.key`.
  - A Pebble plus fake-`DnsZone` test rig.
- **Cut or defer within P1:** GTS overflow (unless M1 makes GTS primary), `invite`/`allowlist` beyond one mechanism, three reserved names (ship one), path mounts, `artifact` source, PROXY v2 to upstream, `preserve_host`, `vault_local` (m3), QR.

## Minor

- **m1.** §6.1 L506's "risk not verified" has an answer for Let's Encrypt: its recursors cap TTL at 60 s ([community thread](https://community.letsencrypt.org/t/dns01-validation-timeouts/116844)). Wait 60 s between authorisations; GTS is unknown. opentunnel keeps both values at one name ([server.md](https://raw.githubusercontent.com/anomalyco/opentunnel/dev/docs/server.md)).
- **m2.** `mesh` depending on `share-proto` for one constant (§5.3 L347) cuts against 00 §1.4–1.5. Add a generic `Net::connect(addr, alpn)`. D15 puts the default ticket beside `link.relay` in the plugin's types, while §10 puts it in `runtime`; choose `crates/share`.
- **m3.** `vault_local` is over-built. The engine identity is a 0600 file via `platform::files::create_private` (`crates/mesh/src/identity.rs#L33-L37`). Store TLS keys the same way and drop `secrets` from P1.
- **m4.** Default-edge key loss strands every client (§5.5). Ship two pinned keys (live plus cold standby) from day one instead of the P2 rotation record.
- **m5.** §6.4 L587 assumes a 300 s SOA negative TTL. I believe Cloudflare fixes it at 1800 s on non-Enterprise plans, but I did not verify this.
- **m6.** theuth merges `Env::prefixed("THEUTH_").split("__")` (`crates/theuth/src/main.rs#L1600`), so `THEUTH_SHARE__ENABLED=true` flips the master switch. The agent can also write `share.enabled` or `share.edge` in `config.yaml` for the next start. Read both only from the plugin's own store.
- **m7.** Missing numbers: global connection and file-descriptor cap, pre-`hello` connection cap and timeout, argon2 global concurrency on the host.
- **m8.** zone-update lookups ignore record type and do not URL-encode `*.sky`. Add wildcard-name create/read/delete to the P0 measurements.
- **m9.** Name expiry (30/90 days unattached) hits a user who simply switched sharing off; add a warning before expiry. Attaching whenever the switch is on gives the edge a continuous presence and IP log; consider attaching only while shares are live.
- **m10.** Q5 "per mesh" omits a cost: browsers coalesce h2 connections across `*.sky.D` on one IP and certificate, so requests for `docs.sky` can arrive at the engine serving `app.sky`. That needs 421 handling.
- **m11.** `share-edge` in the theuth workspace inherits `version.workspace` and `Cargo.lock` (as `crates/relay/Cargo.toml#L3` does), which contradicts "own release train". Give it its own version.
- **m12.** No DNSSEC mention for `D`; enable it.

## Nit

- **N1.** L23 "through the relay" reads as stale beside D2; mark it as the owner's original wording.
- **N2.** D5 says the random name exists "only while the key holds no reserved name"; §5.2 L327 keeps it until its last share stops.
- **N3.** `KIND = "theuthedge"` and the `THEUTH_SHARE_EDGE_` prefix brush 00 §1.1; a relay precedent exists, so say so.
- **N4.** `share_stop` is both a tool (§7.1) and "not a tool call" (§7.4 L832); name the two entry points.
- **N5.** No stale engine-role, family-admission or edge-`local/` references remain outside §11.12. §5.3 L385 "applied by the edge's transport config" should go with M4.

## Questions for the owner

1. **GTS `dns-persist-01` as the primary path (M1)?** It removes zone-update, the token and per-name records, but makes Google the primary CA and conflicts with "Let's Encrypt via DNS-01 + zone-update".
2. **Launch state:** `open` on day one, or `invite` with a live flip once the B2 bounds are measured?
3. **Human-only means owner-only?** Should Slack thread users ever approve a publish (B1)?
4. **Reserved names for brand-new keys,** or random-only until a key has some age?
5. **Does a name die with its machine,** or is signed handover in P1 (M7)?
6. **DNS plan:** Cloudflare Pro's 3,500 records (about 1,745 names) or deSEC's 300 writes per day. Which ceiling do you accept?

## What is good

- **Accuracy.** About 40 code anchors, 10 crate versions and release dates, the instant-acme API (`renewal_info`, `replaces`, `revoke`, `finalize_csr`, `profile`), Let's Encrypt limits, deSEC limits and the zone-update source findings all match. Not checked: the 31-link and desktop UI anchors, PSL quotes, `ppp` and `iroh-tickets` APIs, and the Kubernetes/HAProxy claims.
- **Independence holds.** `platform` has only OS-level dependencies, and the `cargo tree` gate is the right check.
- **DNS reasoning.** The RFC 4592 empty-non-terminal argument is correct, and the `DnsZone` read-back is the right response to zone-update swallowing errors.
- **Honesty.** §9.1 states the operator's impersonation power plainly; the cooldown runs to the old certificate's `not_after`; passwords and interstitial use `__Host-` cookies; live OpenUI apps are excluded for the right reason; uncertain claims are labelled.