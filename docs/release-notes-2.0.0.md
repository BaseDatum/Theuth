# theuth 2.0.0 — release notes (draft)

*Draft written 2026-09-25 from the as-built sections of designs 67, 68 and 69. Sky makes the version-bump commit (Cargo.toml, crates/desktop/tauri.conf.json, crates/desktop/gen/apple/desktop_iOS/Info.plist, Cargo.lock).*

## Why 2.0

Until 1.17, theuth was one engine on one computer. 2.0 makes it a **mesh**: every engine you pair keeps a copy of your conversations, documents, agents, tasks, schedules, secrets and knowledge, and any one of them can serve your phone. A single laptop behaves exactly as before — the mesh is what pairing a second engine adds, never a tax on the first.

The first start migrates your data one way (see *Upgrading*), which is why this is a major version.

## What's new

### Agents over a leaderless mesh (design 67)
- Pair engines with `t mesh pair` / `t mesh add [--ssh|--docker]`; engines find each other on the LAN (mDNS) or through the iroh relay; all traffic is QUIC/TLS 1.3 keyed to each engine's Ed25519 identity — a relay only forwards ciphertext.
- Named agents with charters, journals and budgets; a task ledger; desks that follow an elected **hub**; delegation to coding harnesses on any engine (`agent: "omp@build-A"`); Slack/Signal follow the hub; escalation up the reporting line.
- Vault rows, OAuth grants, knowledge, schedules, skills and MCP files replicate; schedules tick only on the hub.

### Sessions everywhere (design 68)
- Every durable session is written by one owner engine and copied to all others. Open any conversation from any engine or device; typing into it takes ownership (or forwards to the agent's desk); an engine that vanishes releases its sessions after 300 s.
- Artifacts, spill files and attachments are fetched over the mesh by hash.

### Devices, documents and peers on the mesh (design 69)
- Your phone or web client pairs with a **mesh**, not a computer: the hub answers for a shared mesh identity, so when the hub changes the device reconnects to the same key without re-pairing (≈30 s after an abrupt loss). One device can hold several meshes (work, personal). Manage devices under Settings › Mesh › Devices or `t mesh devices`.
- Documents replicate to every engine; the live editor runs on the hub and editors on other engines forward to it; share links name the mesh and survive a hub change.
- Agents on different engines can message each other (`send_message` follows the session owner).
- Replication is echo-guarded and counted (`mesh_status.traffic`, `.replica`, `doc_stats`); a sustained storm logs a warning.

## Upgrading — read before the first start

1. **Quit the app / stop every `t`**, then back up `~/.theuth` (you can skip `models/`, `jobs/`, `cache/`): the first start runs one-way migrations and a 1.17 binary cannot open the result.
2. What the first start changes: `sessions.sqlite` migration 0023 (owner/epoch, session log, error ids); `documents/index.sqlite` gains `hlc`; every bound document directory is **renamed** to its deterministic id; new stores appear (`link.sqlite`, `mesh.sqlite`, `replica`, `roster`, `tasks`, `documents/mirror/`); `link/devices.yaml` is imported once.
3. New config keys (all defaulted; a lone engine needs none): `mesh.name`, `mesh.relay`, `mesh.discovery`, `mesh.bind`, `mesh.availability_flush_secs`, `mesh.storm_frames_per_sec`, `sessions.*`, `roster.*`, `documents.snapshot_quiet_ms`.

## Known limits
- The iroh relay at `iroh.relay.theuth.io` is configured but **not yet deployed**: two engines meet on a LAN or when publicly reachable; internet-wide pairing waits for the relay.
- Devices still use the 31 relay (Noise); moving them to iroh is a follow-up (iOS needs the local-network permission and a multicast entitlement).
- Downgrade requires the backup from step 1.
- Two pre-existing PTY tests fail under a zsh login shell (test-only).
