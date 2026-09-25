# 69 — Devices pair to the mesh

*Status: design v0.2, 2026-09-25 (v0.2: recon corrections — second identity beside the engine's, no-redial on lease loss, pending id in the QR, per-mesh device keys are new work, failover ≈ 30 s, PSK 5 min). Follows 67 (mesh, hub lease) and 68 (sessions everywhere). Sky's rulings of 2026-09-25: serving paired devices is a **hub duty** with automatic failover, so the device registry is **replicated**; devices pair to **hubs (meshes), not engines**, so one device connects to several meshes (work, personal). Supersedes 31 §6 (per-engine `devices.yaml`) and closes 67 loose end 3.2's first half; the transport (31 relay vs iroh) is left as it is.*

## 0. Decisions

| # | Decision | Why |
|---|---|---|
| 1 | **A mesh has an identity of its own**: a second Noise static key pair (`mesh/device_key`, a vault row sealed with the mesh data key, reserved `mesh/` prefix so `{{secret:…}}` cannot expand it), created by the first engine that pairs a device with `set_if_version(expected = None)` (a race loser re-reads). **The engine's own link identity and uplink stay** — document-share URLs (`/d#1.<engine>.…`) and `nodes` dial `/h/<engine id>` | devices must reconnect to "the same thing" after a hub change; an engine key cannot do that, and the engine key has other users |
| 2 | **Serving devices is a hub duty**: the hub — and only the hub — runs a *second* uplink registering the mesh device id at the relay (`/h/<mesh device id>`) and answers device connections. Lease following is Slack's pattern (own `Follower` + reconcile with a `CancellationToken`), `Follower` moves from roster to `mesh` so `link` need not depend on roster. **On loss the mesh uplink's token is cancelled and its supervisor does not redial** — the relay's `register` replaces the previous holder, so a loser that kept redialing would steal the registration back and the two hubs would flap. `holder: None` is a loss; a lone engine is elected hub by the mesh, so it serves | Sky: "paired to device should be a hub duty and it needs to just fail over automatically" |
| 3 | **The device registry is a replicated record**: `<data_dir>/link.sqlite`, table `devices(id TEXT PK, name, platform, static_pub, paired_ms, last_seen_ms, revoked_ms, hlc)` attached to the stage‑1 record replica (single PK, nullable columns, revoke = `revoked_ms`, never delete — a delete loses to a newer column write); `last_seen_ms` written on session close only (each write is a replicated batch). Pairing: `pending(id TEXT PK, psk_sealed, name, created_ms, expires_ms, hlc)` — the PSK is **sealed with the mesh key** (the replicated file is plaintext at rest), TTL 5 min as today. `devices.yaml` is imported once as active rows and then ignored | the new hub must recognise every device the old hub knew, at once |
| 4 | **Devices pair to meshes**: the QR/string carries the *mesh* device id, relay URL, a one‑time PSK **and a pending id** (Noise psk2 is mixed in after msg1, so the hub cannot tell which of several replicated pending PSKs a device holds from msg1 alone — the device sends the pending id in `PairHello`, which tolerates unknown fields; a hint-less device falls back to "exactly one live pending row"). Any engine can show the QR (it reads the vault row and writes the pending row); the hub accepts. Format stays `1.<pub‖secret>.<name>` + a 4th part for the pending id (older apps reject > 3 parts — acceptable, they must update anyway). The app keeps a list of **meshes** (name, members, presence) | Sky: "devices should pair to hubs, so one device can connect to multiple meshes, e.g. work, personal" |
| 5 | **Local stays local**: the desktop app on a machine that runs an engine talks to that engine directly as today (`LOCAL`); it also shows the meshes it is paired to as a device. A device on the LAN of a non‑hub engine still goes via the hub | no regression for the one‑laptop user; LAN‑direct for remote devices is the iroh lift |
| 6 | **Failover is invisible to the user**: when the hub's uplink dies the relay sends the device `CLOSE 4 HARNESS_GONE` and parks its socket; the device shows "reconnecting" and re-dials `/h/<mesh device id>`; the new hub registers within a heartbeat of taking the lease (abrupt loss: ≈ 30 s for the lease itself, graceful release: seconds); the Noise handshake is against the same static key, so nothing on the device changes; open sessions resume through 68's front door on the new hub. A graceful handover closes device conns with a new reason `HUB_MOVED 5` (relay `frame.rs` + UI `CLOSE_REASON`), treated like 4 | |
| 7 | **Transport unchanged**: 31 relay + Noise IK/IKpsk2. The mesh identity is transport‑agnostic — when devices move to iroh (67 loose end 3.2) the hub binds a second iroh endpoint with the same key material | one change at a time; web clients need the relay anyway |
| 8 | **Mesh name**: a replicated `mesh_meta(key TEXT PK, value, hlc)` record in `mesh.sqlite` (`name`, default the first engine's `mesh.name`), added to the `mesh_status` reply (which today has neither mesh nor self name) and shown on the device and Settings › Mesh | a device with two meshes must tell them apart |
| 9 | **Per-mesh device keys**: the app has one device key today (`identity/device`); 69 adds `identity/<mesh id>` so a device is a different identity to each mesh | one device, several meshes, no cross-mesh linkability |

## 1. Shape

```mermaid
sequenceDiagram
    participant P as Phone
    participant R as 31 relay
    participant A as Engine A (hub)
    participant B as Engine B
    rect rgba(0,0,0,0)
    Note over P,B: pair - any engine shows the QR, the row replicates
    B->>B: t mesh devices add → mesh device id + PSK → QR
    P->>R: connect /h/<mesh device id>
    R->>A: deliver (A holds the registration)
    P->>A: Noise IKpsk2 with the PSK
    A->>A: devices row: pending_psk matches → device active (replicated to B)
    end
    rect rgba(0,0,0,0)
    Note over P,B: hub changes - A goes to sleep, B takes the lease
    A-->>R: registration dropped
    B->>R: register /h/<mesh device id> (same static key from the vault row)
    P->>R: uplink ping fails → reconnect /h/<mesh device id>
    R->>B: deliver
    P->>B: Noise IK against the same key → accepted from the replicated devices row
    Note over P,B: the phone never learned that A left
    end
```

## 2. Hub duty

`link` follows the lease itself (Slack's pattern): a `mesh::lease::Follower` fed by `MeshEvent::Lease` (self id via `lookup_when::<mesh::Mesh>` — link starts before mesh and replica), and a reconcile that holds `Option<CancellationToken>` for the mesh uplink. Gain: load `mesh/device_key` from the vault, start the second uplink at `/h/<mesh device id>`, accept devices against the replicated `devices` rows. Loss: cancel the token — the supervisor does **not** redial — and close device conns with `HUB_MOVED 5`. Two hubs for a few seconds (67 §4.4) means two registrations racing at the relay; the relay keeps the latest; the loser learns it lost the lease and stops; devices reconnect once.

A lone engine is its own hub (Follower default `holding: true`), so single‑engine behaviour is unchanged: it registers, it serves.

## 3. Registry

`<data_dir>/link.sqlite` (link gains sqlx + migrations, the roster store shape): `devices` and `pending` attached via `lookup_when::<replica::Replica>` (local-only on failure). `t mesh devices list|add|revoke`, RPC `devices_list`, `device_revoke`, `device_pair_start` (returns the QR payload incl. pending id), `LinkStatus` gains `mesh: Option<{device_id, connected}>` beside the unchanged `harness_id`. `devices.yaml` is imported once as active rows and then ignored (kept on disk, unread).

Security: a device row carries only the device's static public key and metadata; the PSK is one‑time and expires; revocation replicates, so a revoked device is refused by any hub within one replication hop.

## 4. The device side

- The app stores **meshes**: `Harness` gains `kind: 'engine' | 'mesh'` and `meshName`; rows of kind `mesh` dial `/h/<mesh device id>`; `harnesses.ts` keeps its shape (add/remove/active/presence, IndexedDB `state`) and the list UI shows meshes with members + presence from `mesh_status` (which gains `mesh_meta.name` and self name). Existing engine rows keep working (document guests, old pairings).
- One device, several meshes: a per-mesh device key `identity/<mesh id>` (new — today there is one `identity/device`); switch by tapping; sessions/tasks/agents shown are the active mesh's.
- Local engine on this machine: still the `LOCAL` entry; the same app can be local for one mesh and a device of another.

## 5. Build

Contract: `link.sqlite` + `devices`/`pending` tables + replica attach, `mesh/device_key` vault row and the reserved prefix, `Follower` moved to `mesh`, `mesh_meta` + `mesh_status` fields, `HUB_MOVED 5`, RPC/CLI stubs, `LinkStatus.mesh`, QR 4th part + `PairHello.pending` — 1 omp. Then in parallel: **K** link crate (mesh identity registration, replicated registry, pairing flow, HubMoved) ; **D** device app (meshes list, pairing UI, failover UX, member presence); **Z** smoke: two engines + a real phone or the desktop in web mode — pair via engine B's QR, connect through hub A, kill A, watch the device land on B with an open session intact.

## 6. Out of scope

Devices over iroh (67 loose end 3.2, unchanged); per‑device permissions beyond pair/revoke; multi‑user meshes.
