//! 183 §6.9 (D7): the vault-key side of the mesh — the key frames on `theuth/mesh/1`
//! (`KeyEpoch`, `KeyRequest`, `KeyGrant`, `KeyAck`, `KeyRefused`), the keyring in pairing,
//! the rotation after an unpair, the member rule and the Unpair preview.
//!
//! The vault is [`Net::vault`] (the secrets plugin's [`secrets::Vault`], one per process;
//! tests hand each node its own). Every frame rides its own bi stream; a grant is answered
//! with `KeyAck` on the same stream, a request with `KeyGrant` or `KeyRefused`.

use std::collections::{HashMap, HashSet};
use std::str::FromStr as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use data_encoding::BASE32_NOPAD;
use iroh::EndpointId;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use secrets::{KeyInfo, RotationPeer, WireKey};

use crate::identity::now_ms;
use crate::net::{Msg, Net, read_msg, write_msg};
use crate::pairing::{PeerEntry, Rank};
use crate::types::{EngineId, UnpairPreview};

/// A peer connected this long without a `KeyEpoch` runs a pre-183 build (§6.9.5).
const OLD_BUILD_MS: u64 = 5_000;
/// W-34: a tombstone for a member of the current key rotates after this plus up to as much jitter.
const MEMBER_RULE_MS: u64 = 30_000;
/// One `KeyRequest` per (peer, key) this often at most.
const ASK_EVERY_MS: u64 = 30_000;
/// How long a grantor waits for `KeyAck`, a requester for the grant.
const REPLY_WAIT: Duration = Duration::from_secs(10);
/// W-36: clock skew allowed past the removed engine's last sighting.
const PREVIEW_SKEW_MS: u64 = 5 * 60_000;
/// [`tick`] period in production.
pub(crate) const TICK: Duration = Duration::from_secs(5);

/// What this engine knows of each peer's keys (`Net.keyring`).
#[derive(Default)]
pub(crate) struct State {
    peers: Mutex<HashMap<EndpointId, PeerKeys>>,
    /// W-34: tombstoned member → when to rotate.
    rule: Mutex<HashMap<EngineId, u64>>,
    /// (peer, key id) → when last asked.
    asked: Mutex<HashMap<(EndpointId, String), u64>>,
}

#[derive(Default)]
struct PeerKeys {
    /// Start of its current (or last) control connection.
    since_ms: u64,
    /// It sent `KeyEpoch` on that connection.
    spoke: bool,
    current: Option<KeyInfo>,
    held: Vec<String>,
    acked: HashSet<String>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Keys order by `(epoch, initiator, id)` (§6.9.3; the id breaks a tie of two fresh vaults).
fn above(a: &KeyInfo, b: &KeyInfo) -> bool {
    (a.epoch, &a.initiator, &a.id) > (b.epoch, &b.initiator, &b.id)
}

// --- pairing (§6.9.9) -----------------------------------------------------

/// `PairHello.keyring` / `PairAck.keyring`: the current key and every unretired one, with the
/// joiner added to each key's members.
pub(crate) fn pairing_keyring(net: &Net, joiner: &EngineId) -> Vec<WireKey> {
    let Some(vault) = net.vault() else {
        return Vec::new();
    };
    let mut ring = vault.keyring();
    for k in &mut ring {
        if !k.info.members.contains(&joiner.0) {
            k.info.members.push(joiner.0.clone());
        }
    }
    ring
}

/// The side `key_owner` says adopts: the owner's keyring (a pre-183 owner sends only
/// `vault_key`), its first key current, the rest retained, then the sweep.
pub(crate) async fn adopt_pairing(
    net: &Net,
    from: &str,
    vault_key: Option<&str>,
    mut keyring: Vec<WireKey>,
) -> bool {
    let Some(vault) = net.vault() else {
        return false;
    };
    if keyring.is_empty() {
        let Some(key) = vault_key.and_then(|k| {
            BASE32_NOPAD
                .decode(k.to_ascii_uppercase().as_bytes())
                .ok()
                .and_then(|k| <[u8; 32]>::try_from(k.as_slice()).ok())
        }) else {
            tracing::warn!("mesh: {from} sent no readable vault key");
            return false;
        };
        keyring.push(WireKey {
            info: KeyInfo {
                id: secrets::vault::key_id(&key),
                ..KeyInfo::default()
            },
            key: zeroize::Zeroizing::new(BASE32_NOPAD.encode(&key)),
        });
    }
    for k in &mut keyring {
        if !k.info.members.contains(&net.id.0) {
            k.info.members.push(net.id.0.clone());
        }
    }
    match vault.adopt_keyring(keyring).await {
        Ok(()) => {
            tracing::info!("mesh: adopted the vault keyring from {from}");
            // R1 (S5): this engine's other machines learn the key now, not on their next
            // connect (they ask for it: it orders above theirs, or the owner rotates above).
            broadcast_epoch(net).await;
            true
        }
        Err(e) => {
            tracing::warn!("mesh: adopting the vault key from {from}: {e:#}");
            false
        }
    }
}

/// The key owner's side after a pairing (W-35): if the adopter had other engines and its
/// current epoch is ≥ ours, those engines would never take our key as current, so rotate once,
/// above both (183 R1, S5: `max(theirs, ours) + 1`), so the bigger mesh's lineage wins.
pub(crate) fn after_pairing(net: &Arc<Net>, theirs: Option<&Rank>, their_keyring: &[WireKey]) {
    let (Some(vault), Some(rank), Some(their)) = (net.vault(), theirs, their_keyring.first())
    else {
        return;
    };
    if rank.paired_peers == 0 || their.info.epoch < vault.current_key().epoch {
        return;
    }
    tracing::info!(
        "mesh: {} brought a mesh at vault-key epoch {}: rotating so both converge",
        rank.id,
        their.info.epoch
    );
    let (net, floor) = (net.clone(), their.info.epoch);
    tokio::spawn(async move {
        if let Err(e) = rotate_now(&net, Vec::new(), floor, false).await {
            tracing::warn!("mesh: vault-key rotation after pairing: {e}");
        }
    });
}

// --- frames ---------------------------------------------------------------

/// Right after the `Peers` frame on every control connection, both sides (§6.9.7). A peer
/// that already spoke `KeyEpoch` in this process stays a 183 build (R1, S9: its frame can land
/// before this runs on a second connection).
pub(crate) async fn on_connect(net: &Arc<Net>, conn: &Connection) {
    lock(&net.keyring.peers)
        .entry(conn.remote_id())
        .or_default()
        .since_ms = now_ms();
    send_epoch(net, conn).await;
}

async fn send_epoch(net: &Net, conn: &Connection) {
    let Some(vault) = net.vault() else {
        return;
    };
    let msg = Msg::KeyEpoch {
        current: vault.current_key(),
        held: vault.held_keys(),
    };
    if let Ok((mut send, _recv)) = conn.open_bi().await
        && write_msg(&mut send, &msg).await.is_ok()
    {
        let _ = send.finish();
    }
}

/// `KeyEpoch` to every connected peer: our current key moved (a grant, a pairing).
async fn broadcast_epoch(net: &Net) {
    for (_, c) in net.control_conns() {
        send_epoch(net, &c).await;
    }
}

/// A key frame from a peer, on its own bi stream (spawned by `drive`).
pub(crate) async fn on_frame(
    net: Arc<Net>,
    peer: EndpointId,
    msg: Msg,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let Some(vault) = net.vault() else {
        tracing::debug!("mesh: no vault; ignoring {msg:?} from {}", peer.fmt_short());
        return;
    };
    match msg {
        Msg::KeyEpoch { current, held } => {
            tracing::info!(
                "mesh: KeyEpoch from {}: key {} (epoch {})",
                peer.fmt_short(),
                current.id,
                current.epoch
            );
            let mine = vault.current_key();
            let mut want = Vec::new();
            if above(&current, &mine) {
                want.push(current.id.clone());
            }
            for id in vault.missing_keys() {
                if held.contains(&id) && !want.contains(&id) {
                    want.push(id);
                }
            }
            {
                let mut peers = lock(&net.keyring.peers);
                let p = peers.entry(peer).or_default();
                p.spoke = true;
                p.current = Some(current);
                p.held = held;
            }
            for id in want {
                request(&net, peer, &id).await;
            }
            sync_peers(&net, now_ms()).await;
        }
        Msg::KeyRequest { id } => {
            let refuse = |reason: &str| Msg::KeyRefused {
                id: id.clone(),
                reason: reason.to_owned(),
            };
            // A tombstoned asker (its tombstone arrived after its connection did, or by the
            // replica behind every merge) gets nothing: the store's live set decides (R1, V-M1).
            if !net
                .refresh_paired()
                .await
                .is_ok_and(|live| live.contains(&peer))
            {
                tracing::info!(
                    "mesh: KeyRequest from unpaired {} refused",
                    peer.fmt_short()
                );
                let _ = write_msg(&mut send, &refuse("not paired")).await;
                return;
            }
            let Some(mut k) = vault.keyring().into_iter().find(|k| k.info.id == id) else {
                let _ = write_msg(&mut send, &refuse("unknown key")).await;
                return;
            };
            if !k.info.members.contains(&peer.to_string()) {
                k.info.members.push(peer.to_string());
                // Only gains a member (W-34: the engines it was granted to).
                let _ = vault.adopt_key(k.clone()).await;
            }
            grant_on(&net, peer, &mut send, &mut recv, k).await;
        }
        Msg::KeyGrant { key, removed } => {
            if let Some(id) = receive_grant(&net, peer, key, removed).await
                && write_msg(&mut send, &Msg::KeyAck { id }).await.is_ok()
            {
                let _ = send.finish();
            }
        }
        Msg::KeyAck { id } => acked(&net, peer, id).await,
        Msg::KeyRefused { id, reason } => {
            tracing::info!("mesh: {} refused key {id}: {reason}", peer.fmt_short());
        }
        other => tracing::debug!("mesh: ignoring {other:?} from {}", peer.fmt_short()),
    }
}

/// Send `key` with our tombstones on `send`, then wait for the `KeyAck`.
async fn grant_on(
    net: &Net,
    peer: EndpointId,
    send: &mut SendStream,
    recv: &mut RecvStream,
    key: WireKey,
) {
    let removed: Vec<PeerEntry> = net
        .store
        .peer_entries()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(PeerEntry::removed)
        .collect();
    let id = key.info.id.clone();
    tracing::info!("mesh: KeyGrant {id} to {}", peer.fmt_short());
    if write_msg(send, &Msg::KeyGrant { key, removed })
        .await
        .is_err()
    {
        return;
    }
    let _ = send.finish();
    if let Ok(Ok(Msg::KeyAck { id })) = tokio::time::timeout(REPLY_WAIT, read_msg(recv)).await {
        acked(net, peer, id).await;
    } else {
        tracing::debug!("mesh: no KeyAck for {id} from {}", peer.fmt_short());
    }
}

/// A grant: the tombstones first, then the key (W-30). `Some(id)` = stored.
async fn receive_grant(
    net: &Arc<Net>,
    peer: EndpointId,
    key: WireKey,
    removed: Vec<PeerEntry>,
) -> Option<String> {
    let vault = net.vault()?;
    // R1 (V-M1): the grantor and the tombstones against the store, not the in-memory set.
    if !net
        .refresh_paired()
        .await
        .is_ok_and(|live| live.contains(&peer))
    {
        tracing::warn!("mesh: KeyGrant from unpaired {} ignored", peer.fmt_short());
        return None;
    }
    if let Err(e) = net
        .merge_peers(&removed, Some(&EngineId(peer.to_string())))
        .await
    {
        tracing::warn!("mesh: tombstones with a key grant: {e:#}; key not stored");
        return None;
    }
    for r in &removed {
        if let Ok(eid) = EndpointId::from_str(&r.id.0) {
            net.close_conns(&eid).await;
        }
    }
    let id = key.info.id.clone();
    tracing::info!(
        "mesh: KeyGrant {id} from {}: {} tombstone(s) applied before the key",
        peer.fmt_short(),
        removed.len()
    );
    match vault.adopt_key(key).await {
        Ok(current) => {
            if current {
                broadcast_epoch(net).await;
            }
            Some(id)
        }
        Err(e) => {
            tracing::warn!("mesh: key {id} from {}: {e:#}", peer.fmt_short());
            None
        }
    }
}

async fn acked(net: &Net, peer: EndpointId, id: String) {
    // R1 (S2): the first copy of the new key elsewhere ends the sweep hold.
    if let Some(v) = net.vault()
        && v.current_key().id == id
    {
        v.release_sweep();
    }
    lock(&net.keyring.peers)
        .entry(peer)
        .or_default()
        .acked
        .insert(id);
    sync_peers(net, now_ms()).await;
}

/// `KeyRequest{id}` to `peer`; a grant is stored (tombstones first) and acknowledged.
async fn request(net: &Arc<Net>, peer: EndpointId, id: &str) {
    let now = now_ms();
    {
        let mut asked = lock(&net.keyring.asked);
        let key = (peer, id.to_owned());
        if asked
            .get(&key)
            .is_some_and(|&t| now < t.saturating_add(ASK_EVERY_MS))
        {
            return;
        }
        asked.insert(key, now);
    }
    let Some(conn) = net.control_conn(&peer) else {
        return;
    };
    let Ok((mut send, mut recv)) = conn.open_bi().await else {
        return;
    };
    tracing::info!("mesh: KeyRequest {id} to {}", peer.fmt_short());
    if write_msg(&mut send, &Msg::KeyRequest { id: id.to_owned() })
        .await
        .is_err()
    {
        return;
    }
    match tokio::time::timeout(REPLY_WAIT, read_msg(&mut recv)).await {
        Ok(Ok(Msg::KeyGrant { key, removed })) => {
            if let Some(id) = receive_grant(net, peer, key, removed).await {
                let _ = write_msg(&mut send, &Msg::KeyAck { id }).await;
                let _ = send.finish();
            }
        }
        Ok(Ok(Msg::KeyRefused { id, reason })) => {
            tracing::info!("mesh: {} refused key {id}: {reason}", peer.fmt_short());
        }
        other => tracing::debug!("mesh: KeyRequest {id} to {}: {other:?}", peer.fmt_short()),
    }
}

/// The current key as `KeyGrant` to every connected peer the store holds paired (§6.9.5).
async fn push_grants(net: &Arc<Net>) {
    let Some(key) = net.vault().and_then(|v| v.keyring().into_iter().next()) else {
        return;
    };
    let Ok(live) = net.refresh_paired().await else {
        return;
    };
    for (peer, conn) in net.control_conns() {
        if !live.contains(&peer) {
            continue;
        }
        let (net, key) = (net.clone(), key.clone());
        tokio::spawn(async move {
            if let Ok((mut send, mut recv)) = conn.open_bi().await {
                grant_on(&net, peer, &mut send, &mut recv, key).await;
            }
        });
    }
}

// --- rotation -------------------------------------------------------------

/// Mint above `floor` for the store's paired set plus this engine, grant to the connected
/// peers, and sweep once a peer acked the key or [`secrets::vault::SWEEP_HOLD_MS`] passed (R1,
/// S2: no row is re-sealed under a key only this machine holds while another is reachable).
/// `reset_devices` rides in the rotation record (D11).
async fn rotate_now(
    net: &Arc<Net>,
    removed: Vec<String>,
    floor: u32,
    reset_devices: bool,
) -> Result<KeyInfo, String> {
    let vault = net.vault().ok_or("secrets is not running")?;
    let live = net.refresh_paired().await.map_err(|e| format!("{e:#}"))?;
    let mut members: Vec<String> = live.iter().map(ToString::to_string).collect();
    members.sort();
    members.push(net.id.0.clone());
    let info = vault
        .rotate(&net.id.0, members, removed, floor, reset_devices)
        .await
        .map_err(|e| format!("{e:#}"))?;
    lock(&net.keyring.rule).clear();
    let net = net.clone();
    tokio::spawn(async move {
        push_grants(&net).await;
        sync_peers(&net, now_ms()).await;
        let until = now_ms() + secrets::vault::SWEEP_HOLD_MS + 1_000;
        loop {
            if let Err(e) = vault.maintain(now_ms()).await {
                tracing::warn!("mesh: vault sweep after the rotation: {e:#}");
                return;
            }
            let sweeping = vault
                .rotation()
                .is_some_and(|r| r.phase == secrets::RotationPhase::Sweeping);
            if !sweeping || now_ms() > until {
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    Ok(info)
}

/// `RemovePeer` after the tombstone, the peer push and the lease recompute (W-28): rotate
/// the vault key for the remaining machines. `Err` leaves the unpair standing.
pub(crate) async fn after_remove(
    net: &Arc<Net>,
    removed: &EngineId,
    reset_devices: bool,
) -> Result<(), String> {
    rotate_now(net, vec![removed.0.clone()], 0, reset_devices)
        .await
        .map(|_| ())
}

/// `vault_rotate` (§6.9.12): a rotation without an unpair, or the Retry of a failed one.
pub(crate) async fn rotate(net: &Arc<Net>) -> Result<secrets::RotationState, String> {
    rotate_now(net, Vec::new(), 0, false).await?;
    net.vault()
        .and_then(|v| v.rotation())
        .ok_or_else(|| "no rotation record".into())
}

/// Mesh start: the legacy key's members are the paired set at the first boot on this build
/// (§9.5, §6.9.12), so tombstones written before 183 never trigger the member rule.
pub(crate) async fn start(net: &Net) {
    let Some(vault) = net.vault() else {
        return;
    };
    let mut members: Vec<String> = net.paired_ids().iter().map(ToString::to_string).collect();
    members.push(net.id.0.clone());
    if let Err(e) = vault.set_legacy_members(members).await {
        tracing::warn!("mesh: vault-key members: {e:#}");
    }
}

/// Every [`TICK`] (`now` from the caller, so tests drive the clock): the member rule (W-34),
/// `KeyRequest`s for keys local rows name but we lack (W-38), and the rotation's peer list.
pub(crate) async fn tick(net: &Arc<Net>, now: u64) {
    // R1 (V-M1), R3: pick up `peers` rows the replica wrote behind every merge; a tombstone
    // closes every connection of that engine within one tick, vault or not.
    if let Err(e) = net.refresh_paired().await {
        tracing::debug!("mesh: paired set: {e:#}");
    }
    let Some(vault) = net.vault() else {
        return;
    };
    let current = vault.current_key();
    let tombstoned: Vec<EngineId> = net
        .store
        .peer_entries()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.removed() && p.id != net.id && current.members.contains(&p.id.0))
        .map(|p| p.id)
        .collect();
    let due: Vec<String> = {
        let mut rule = lock(&net.keyring.rule);
        rule.retain(|id, _| tombstoned.contains(id));
        for id in &tombstoned {
            rule.entry(id.clone())
                .or_insert_with(|| now + MEMBER_RULE_MS + rand::random_range(0..MEMBER_RULE_MS));
        }
        rule.iter()
            .filter(|&(_, &at)| now >= at)
            .map(|(id, _)| id.0.clone())
            .collect()
    };
    if !due.is_empty() {
        tracing::info!(
            "mesh: the current vault key was granted to unpaired {}: rotating (W-34)",
            due.join(", ")
        );
        if let Err(e) = rotate_now(net, due, 0, false).await {
            tracing::warn!("mesh: member-rule rotation: {e}");
        }
    }
    let missing = vault.missing_keys();
    if !missing.is_empty() {
        for (peer, _) in net.control_conns() {
            for id in &missing {
                request(net, peer, id).await;
            }
        }
    }
    sync_peers(net, now).await;
}

/// Is `peer` a pre-183 build: connected [`OLD_BUILD_MS`] (its last connection) without `KeyEpoch`.
fn old_build(p: Option<&PeerKeys>, now: u64) -> bool {
    p.is_some_and(|p| !p.spoke && p.since_ms > 0 && now >= p.since_ms + OLD_BUILD_MS)
}

/// The rotation's `peers` (§14.7) and removed names (183 R2) into the vault's record.
async fn sync_peers(net: &Net, now: u64) {
    let Some(vault) = net.vault() else {
        return;
    };
    if vault.rotation().is_none() {
        return;
    }
    let current = vault.current_key().id;
    let entries = net.store.peer_entries().await.unwrap_or_default();
    let list: Vec<RotationPeer> = {
        let peers = lock(&net.keyring.peers);
        entries
            .iter()
            .filter(|e| !e.removed() && e.id != net.id)
            .map(|e| {
                let eid = EndpointId::from_str(&e.id.0).ok();
                let p = eid.and_then(|id| peers.get(&id));
                RotationPeer {
                    id: e.id.0.clone(),
                    name: e.name.clone(),
                    online: eid.is_some_and(|id| net.is_connected(&id)),
                    has_key: p.is_some_and(|p| {
                        p.acked.contains(&current)
                            || p.held.contains(&current)
                            || p.current.as_ref().is_some_and(|c| c.id == current)
                    }),
                    old_build: old_build(p, now),
                }
            })
            .collect()
    };
    // 183 R2: the removed machines by name (their tombstoned rows keep it), never an id.
    let removed: Vec<String> = vault
        .rotation()
        .map(|r| r.removed)
        .unwrap_or_default()
        .iter()
        .map(|id| {
            entries
                .iter()
                .find(|e| &e.id.0 == id)
                .map(|e| e.name.clone())
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| secrets::vault::REMOVED_MACHINE.to_owned())
        })
        .collect();
    // The vault writes the record only when it changed.
    vault.set_rotation_peers(list, removed).await;
}

/// `mesh_unpair_preview` (§12.9, W-36): what the Unpair dialog lists before anything happens.
pub(crate) async fn unpair_preview(net: &Net, id: &EngineId) -> anyhow::Result<UnpairPreview> {
    let entries = net.store.peer_entries().await?;
    let Some(row) = entries.iter().find(|p| &p.id == id && !p.removed()) else {
        anyhow::bail!("unknown peer {id}");
    };
    let peer = net.peer_of(row);
    // Connected, or never seen by this process (its last sighting is unknown): list all.
    let cutoff = if peer.reachable {
        None
    } else {
        net.last_seen(&id.0).map(|t| t + PREVIEW_SKEW_MS)
    };
    let (secrets, sign_ins) = net
        .vault()
        .map(|v| v.names_written_by(cutoff))
        .unwrap_or_default();
    let now = now_ms();
    let (mut offline, mut old) = (Vec::new(), Vec::new());
    {
        let peers = lock(&net.keyring.peers);
        for e in entries
            .iter()
            .filter(|e| !e.removed() && e.id != net.id && &e.id != id)
        {
            let eid = EndpointId::from_str(&e.id.0).ok();
            if !eid.is_some_and(|i| net.is_connected(&i)) {
                offline.push(e.id.0.clone());
            }
            if old_build(eid.and_then(|i| peers.get(&i)), now) {
                old.push(e.id.0.clone());
            }
        }
    }
    Ok(UnpairPreview {
        id: peer.id,
        name: peer.name,
        connected: peer.reachable,
        last_seen_ms: peer.last_seen_ms,
        secrets,
        sign_ins,
        offline,
        old_build: old,
        // The rpc fills it from the link plugin (mesh cannot name link).
        devices: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::tests::{bind_at, node_at};
    use iroh::SecretKey;
    use secrets::Vault;

    async fn eventually(what: &str, f: impl Fn() -> bool) {
        for _ in 0..400 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("timed out waiting for {what}");
    }

    fn eid(n: &Net) -> EndpointId {
        n.endpoint.id()
    }

    /// What replication would bring: `to`'s rows replaced by the ones in `from_dir` (the §15
    /// proof's `attach … insert into vault select * from a.vault`).
    async fn copy_rows(from_dir: &std::path::Path, to: &Vault) {
        let mut c = to.pool().acquire().await.unwrap();
        sqlx::query("ATTACH DATABASE ? AS src")
            .bind(from_dir.join(secrets::store::DB).display().to_string())
            .execute(&mut *c)
            .await
            .unwrap();
        for q in [
            "DELETE FROM vault",
            "INSERT INTO vault SELECT * FROM src.vault",
            "DELETE FROM vault_current",
            "INSERT INTO vault_current SELECT * FROM src.vault_current",
            "DETACH DATABASE src",
        ] {
            sqlx::query(q).execute(&mut *c).await.unwrap();
        }
    }

    async fn swept(v: &Vault) -> bool {
        let cur = v.current_key().id;
        let (rows, _) = secrets::store::rows(v.pool()).await.unwrap();
        rows.iter().all(|r| r.key_id == cur)
    }

    /// 183 §6.9.2–§6.9.5 (the brief's first test): A, B and C paired; A unpairs C. A and B end
    /// on the new key (B through the pushed `KeyGrant`, tombstone first, then `KeyAck`); C
    /// keeps the old key: its own copy opens, a copy taken after the rotation does not.
    #[tokio::test]
    async fn unpair_rotates_for_the_remaining_machines_only() {
        let (a, da, va) = node_at("a", 1_000).await;
        let (b, _db, vb) = node_at("b", 3_000).await;
        let (c, dc, vc) = node_at("c", 4_000).await;
        b.add_peer(&a.pairing_string().await).await.unwrap();
        c.add_peer(&a.pairing_string().await).await.unwrap();
        eventually("a sees b", || a.is_connected(&eid(&b))).await;
        a.push_peers().await;
        eventually("b knows c", || b.is_paired(&eid(&c))).await;
        start(&a).await;
        eventually("a sees b and c", || {
            a.is_connected(&eid(&b)) && a.is_connected(&eid(&c))
        })
        .await;
        let k0 = va.current_key();
        assert_eq!(
            (vb.current_key().id, vc.current_key().id),
            (k0.id.clone(), k0.id.clone())
        );
        va.set("gh", "ghp-before-rotation", None, "user")
            .await
            .unwrap();
        copy_rows(da.path(), &vc).await;
        vc.refresh().await.unwrap();

        a.remove_peer(&c.id).await.unwrap();
        a.push_peers().await;
        after_remove(&a, &c.id, false).await.unwrap();
        let k1 = va.current_key();
        assert_eq!(k1.epoch, 1);
        assert!(!k1.members.contains(&c.id.0) && k1.members.contains(&b.id.0));
        assert_eq!(va.rotation().unwrap().removed, vec![c.id.0.clone()]);
        // 183 R2: the record names the removed machine, so no surface shows its id.
        eventually("a's record names c", || {
            va.rotation().is_some_and(|r| r.removed_names == ["c"])
        })
        .await;

        eventually("b holds the new key", || vb.current_key().id == k1.id).await;
        assert!(!b.is_paired(&eid(&c)), "the tombstone came first");
        eventually("b's KeyAck reached a", || {
            va.rotation()
                .is_some_and(|r| r.peers.iter().any(|p| p.id == b.id.0 && p.has_key))
        })
        .await;
        assert_eq!(
            vc.current_key().id,
            k0.id,
            "the removed engine never gets it"
        );

        // C's copy from before the rotation still opens there; one taken after does not.
        let before = Vault::open(dc.path(), secrets::KeyStore::File)
            .await
            .unwrap();
        assert_eq!(before.get("gh", 0).unwrap().as_str(), "ghp-before-rotation");
        for _ in 0..200 {
            if swept(&va).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        va.set("after", "ghp-after-rotation", None, "user")
            .await
            .unwrap();
        copy_rows(da.path(), &vc).await;
        let after = Vault::open(dc.path(), secrets::KeyStore::File)
            .await
            .unwrap();
        assert!(after.get("after", 0).is_none());
        assert!(
            after.get("gh", 0).is_none(),
            "re-sealed rows do not open on c"
        );
        assert_eq!(after.missing_keys(), vec![k1.id.clone()]);
        for n in [&a, &b, &c] {
            n.shutdown().await;
        }
    }

    /// §6.9.7, §13 rows 31–32: B is offline during the rotation. Re-sealed rows reach it first
    /// and its reads stay; on reconnect `KeyEpoch` → `KeyRequest` → `KeyGrant` (tombstone first).
    #[tokio::test]
    async fn an_offline_machine_gets_the_key_on_reconnect_and_never_reads_blank() {
        let (a, da, va) = node_at("a", 1_000).await;
        let db = tempfile::tempdir().unwrap();
        let vb = Arc::new(
            Vault::open(db.path(), secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        let sb = SecretKey::generate();
        let b = bind_at("b", 3_000, None, db.path(), sb.clone(), Some(vb.clone())).await;
        let (c, _dc, _vc) = node_at("c", 4_000).await;
        b.add_peer(&a.pairing_string().await).await.unwrap();
        c.add_peer(&a.pairing_string().await).await.unwrap();
        eventually("a sees b", || a.is_connected(&eid(&b))).await;
        a.push_peers().await;
        eventually("b knows c", || b.is_paired(&eid(&c))).await;
        va.set("gh", "ghp-shared-value", None, "user")
            .await
            .unwrap();
        copy_rows(da.path(), &vb).await;
        vb.refresh().await.unwrap();
        eventually("a sees b", || a.is_connected(&eid(&b))).await;

        b.shutdown().await;
        eventually("b is offline", || !a.is_connected(&eid(&b))).await;
        a.remove_peer(&c.id).await.unwrap();
        after_remove(&a, &c.id, false).await.unwrap();
        let k1 = va.current_key();
        // No peer is reachable to ack: the sweep waits out its hold (R1, S2).
        va.release_sweep();
        va.maintain(now_ms()).await.unwrap();
        for _ in 0..200 {
            if swept(&va).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        copy_rows(da.path(), &vb).await;
        vb.refresh().await.unwrap();
        assert_eq!(
            vb.get("gh", 0).unwrap().as_str(),
            "ghp-shared-value",
            "never blank"
        );
        assert_eq!(vb.missing_keys(), vec![k1.id.clone()]);

        let b = bind_at("b", 3_000, None, db.path(), sb, Some(vb.clone())).await;
        let cancel = tokio_util::sync::CancellationToken::new();
        tokio::spawn(b.clone().run_connector(cancel.clone()));
        eventually("b fetched the key on reconnect", || {
            vb.current_key().id == k1.id
        })
        .await;
        assert!(!b.is_paired(&eid(&c)), "the tombstone came with the key");
        vb.refresh().await.unwrap();
        assert!(vb.missing_keys().is_empty());
        assert_eq!(vb.get("gh", 0).unwrap().as_str(), "ghp-shared-value");
        cancel.cancel();
        for n in [&a, &b, &c] {
            n.shutdown().await;
        }
    }

    /// §6.9.9 (W-35): a machine pairing after a rotation, before the sweep or the retire, gets
    /// the current key and the retained one, with itself added to their members.
    #[tokio::test]
    async fn pairing_during_a_rotation_carries_the_keyring() {
        let (a, _da, va) = node_at("a", 1_000).await;
        let (d, _dd, vd) = node_at("d", 3_000).await;
        let k0 = va.current_key();
        let st = rotate(&a).await.unwrap();
        assert_eq!(st.removed, Vec::<String>::new());
        d.add_peer(&a.pairing_string().await).await.unwrap();
        assert_eq!(vd.current_key().id, st.key.id);
        assert!(vd.held_keys().contains(&k0.id), "the retained key too");
        assert!(vd.current_key().members.contains(&d.id.0));
        a.shutdown().await;
        d.shutdown().await;
    }

    /// W-34, §13 row 34: a tombstone for a member of the current key that no rotation followed
    /// (an initiator crashed before the mint, or another engine unpaired it) rotates after 30 s
    /// plus jitter, never before.
    #[tokio::test]
    async fn a_tombstoned_member_rotates_after_the_wait() {
        let (a, _da, va) = node_at("a", 1_000).await;
        let (c, _dc, _vc) = node_at("c", 4_000).await;
        c.add_peer(&a.pairing_string().await).await.unwrap();
        start(&a).await;
        assert!(va.current_key().members.contains(&c.id.0));
        let mut row = a
            .store
            .peer_entries()
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.id == c.id)
            .unwrap();
        row.removed_ms = Some(now_ms());
        a.merge_peers(&[row], None).await.unwrap();
        let t = now_ms();
        tick(&a, t).await;
        tick(&a, t + MEMBER_RULE_MS - 1).await;
        assert_eq!(va.current_key().epoch, 0, "not before 30 s");
        tick(&a, t + 2 * MEMBER_RULE_MS).await;
        let k = va.current_key();
        assert_eq!(k.epoch, 1);
        assert!(!k.members.contains(&c.id.0));
        assert_eq!(va.rotation().unwrap().removed, vec![c.id.0.clone()]);
        tick(&a, t + 5 * MEMBER_RULE_MS).await;
        assert_eq!(
            va.current_key().epoch,
            1,
            "the new key has no removed member"
        );
        a.shutdown().await;
        c.shutdown().await;
    }

    /// §6.9.10: a paired machine that never sends `KeyEpoch` (a pre-183 build) is named in the
    /// rotation's peers once it has been connected 5 s.
    #[tokio::test]
    async fn a_pre_183_peer_is_named_in_the_rotation() {
        let (a, _da, va) = node_at("a", 1_000).await;
        let dold = tempfile::tempdir().unwrap();
        let old = bind_at("old", 3_000, None, dold.path(), SecretKey::generate(), None).await;
        old.add_peer(&a.pairing_string().await).await.unwrap();
        eventually("a sees the old build", || a.is_connected(&eid(&old))).await;
        rotate(&a).await.unwrap();
        tick(&a, now_ms()).await;
        let peer = |st: &secrets::RotationState| {
            st.peers.iter().find(|p| p.id == old.id.0).cloned().unwrap()
        };
        assert!(!peer(&va.rotation().unwrap()).old_build, "not before 5 s");
        tick(&a, now_ms() + OLD_BUILD_MS + 1_000).await;
        let p = peer(&va.rotation().unwrap());
        assert!(p.old_build && p.online && !p.has_key);
        a.shutdown().await;
        old.shutdown().await;
    }

    /// 183 R1 (V-M1): C's tombstone reaches B by the replica first (written into B's store,
    /// behind every merge). A key B mints is still refused to C's `KeyRequest`, B closes C, and
    /// the next rotation's members come from the store, so C is not among them.
    #[tokio::test]
    async fn a_replica_first_tombstone_still_refuses_the_removed_machine() {
        let (b, _db, vb) = node_at("b", 1_000).await;
        let (c, _dc, vc) = node_at("c", 3_000).await;
        c.add_peer(&b.pairing_string().await).await.unwrap();
        eventually("b and c connected", || {
            b.is_connected(&eid(&c)) && c.is_connected(&eid(&b))
        })
        .await;
        let mut row = b
            .store
            .peer_entries()
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.id == c.id)
            .unwrap();
        row.removed_ms = Some(now_ms());
        b.store.put_peer(&row).await.unwrap();
        assert!(b.is_paired(&eid(&c)), "the in-memory set is stale");
        // Minted in the vault alone, so only `KeyRequest`'s own check stands between.
        let k1 = vb
            .rotate(
                &b.id.0,
                vec![b.id.0.clone()],
                vec![c.id.0.clone()],
                0,
                false,
            )
            .await
            .unwrap();
        request(&c, eid(&b), &k1.id).await;
        assert!(!vc.held_keys().contains(&k1.id), "c never gets the new key");
        eventually("b closed c", || !b.is_connected(&eid(&c))).await;
        let k2 = rotate(&b).await.unwrap().key;
        assert!(!k2.members.contains(&c.id.0));
        b.shutdown().await;
        c.shutdown().await;
    }

    /// 183 R1 (S1), R4: A unpairs C; before B hears of it, C introduces X to B. When C's
    /// tombstone lands, X is quarantined on B (listed, not paired here, its replicated row still
    /// live); a row C introduced long before its unpair stays. Keeping X pairs it again.
    #[tokio::test]
    async fn a_machine_introduced_by_a_removed_one_is_quarantined() {
        let (b, _db, _vb) = node_at("b", 1_000).await;
        let id = || EngineId(SecretKey::generate().public().to_string());
        let (c, x, y) = (id(), id(), id());
        let now = now_ms();
        let entry = |id: &EngineId, name: &str, added_ms: u64| PeerEntry {
            id: id.clone(),
            name: name.into(),
            addrs: Vec::new(),
            hub_pref: crate::types::HubPref::Eligible,
            added_ms,
            removed_ms: None,
        };
        b.merge_peers(&[entry(&c, "c", now - 3_600_000)], None)
            .await
            .unwrap();
        // Y came with C's set an hour ago; X arrives from C now.
        b.store
            .put_peer(&entry(&y, "y", now - 3_600_000))
            .await
            .unwrap();
        b.store
            .record_origin(&y, &c, now - 3_600_000)
            .await
            .unwrap();
        b.merge_peers(&[entry(&x, "x", now)], Some(&c))
            .await
            .unwrap();
        let ex = EndpointId::from_str(&x.0).unwrap();
        assert!(b.is_paired(&ex));
        // A unpaired C a second ago; its tombstone reaches B.
        let mut dead = entry(&c, "c", now - 3_600_000);
        dead.removed_ms = Some(now - 1_000);
        b.merge_peers(&[dead], None).await.unwrap();
        assert!(!b.is_paired(&ex), "X is quarantined");
        let row = b.store.peer_entries().await.unwrap();
        assert!(
            row.iter().any(|p| p.id == x && !p.removed()),
            "the replicated row is never tombstoned by a quarantine"
        );
        assert!(
            !b.store.peers().await.unwrap().iter().any(|p| p.id == x),
            "but it is no peer here: not in the status list, the lease or the paired set"
        );
        assert!(
            b.is_paired(&EndpointId::from_str(&y.0).unwrap()),
            "Y predates the unpair"
        );
        let q = b.quarantined().await.unwrap();
        assert_eq!(
            q.iter()
                .map(|q| (q.id.clone(), q.name.as_str(), q.added_by.clone()))
                .collect::<Vec<_>>(),
            vec![(x.clone(), "x", c.clone())]
        );
        b.confirm_peer(&x).await.unwrap();
        assert!(b.is_paired(&ex), "kept");
        assert!(b.quarantined().await.unwrap().is_empty());
        b.shutdown().await;
    }

    /// P2-F1: Remove on a quarantined machine tombstones it and only then clears the list
    /// entry: it is never paired in between (the paired set never moves) and leaves the list.
    #[tokio::test]
    async fn removing_a_quarantined_machine_never_pairs_it() {
        let (b, _db, _vb) = node_at("b", 1_000).await;
        let id = || EngineId(SecretKey::generate().public().to_string());
        let (c, x) = (id(), id());
        let now = now_ms();
        let entry = |id: &EngineId, added_ms: u64, removed_ms: Option<u64>| PeerEntry {
            id: id.clone(),
            name: id.0[..4].into(),
            addrs: Vec::new(),
            hub_pref: crate::types::HubPref::Eligible,
            added_ms,
            removed_ms,
        };
        b.merge_peers(&[entry(&c, now - 3_600_000, None)], None)
            .await
            .unwrap();
        b.merge_peers(&[entry(&x, now, None)], Some(&c))
            .await
            .unwrap();
        b.merge_peers(&[entry(&c, now - 3_600_000, Some(now - 1_000))], None)
            .await
            .unwrap();
        assert_eq!(b.quarantined().await.unwrap().len(), 1);
        let ex = EndpointId::from_str(&x.0).unwrap();
        let mut generation = b.peers_gen.subscribe();
        generation.mark_unchanged();
        b.remove_peer(&x).await.unwrap();
        assert!(
            !generation.has_changed().unwrap(),
            "x was never paired in between"
        );
        assert!(!b.is_paired(&ex));
        assert!(
            b.quarantined().await.unwrap().is_empty(),
            "the list entry is gone"
        );
        let rows = b.store.peer_entries().await.unwrap();
        assert!(rows.iter().any(|p| p.id == x && p.removed()), "tombstoned");
        b.shutdown().await;
    }

    /// 183 R4: A and B pair through E; N pairs through E and learns A and B from it; A retires
    /// E a moment later. Nothing E handed over at pairing is quarantined: A and B stay paired on
    /// N, A on B, nothing is listed, no row of theirs is tombstoned.
    #[tokio::test]
    async fn retiring_the_pairing_partner_unpairs_nobody() {
        let (e, _de, _ve) = node_at("e", 1_000).await;
        let (a, _da, _va) = node_at("a", 2_000).await;
        let (b, _db, _vb) = node_at("b", 3_000).await;
        let (n, _dn, _vn) = node_at("n", 4_000).await;
        a.add_peer(&e.pairing_string().await).await.unwrap();
        b.add_peer(&e.pairing_string().await).await.unwrap();
        n.add_peer(&e.pairing_string().await).await.unwrap();
        let ep = |x: &Arc<Net>| EndpointId::from_str(&x.id.0).unwrap();
        assert!(
            n.is_paired(&ep(&a)) && n.is_paired(&ep(&b)),
            "n learned a and b"
        );
        assert!(b.is_paired(&ep(&a)), "b learned a");
        a.remove_peer(&e.id).await.unwrap();
        let dead = a
            .store
            .peer_entries()
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.id == e.id)
            .unwrap();
        for x in [&n, &b] {
            x.merge_peers(std::slice::from_ref(&dead), Some(&a.id))
                .await
                .unwrap();
            assert!(!x.is_paired(&ep(&e)), "{}: e is gone", x.id);
            assert!(x.quarantined().await.unwrap().is_empty(), "{}", x.id);
        }
        assert!(
            n.is_paired(&ep(&a)) && n.is_paired(&ep(&b)),
            "a and b stay on n"
        );
        assert!(b.is_paired(&ep(&a)), "a stays on b");
        let rows = n.store.peer_entries().await.unwrap();
        for x in [&a, &b] {
            assert!(rows.iter().any(|p| p.id == x.id && !p.removed()));
        }
        for x in [e, a, b, n] {
            x.shutdown().await;
        }
    }

    /// Two meshes pair; `a`'s (three machines) owns the key. Every machine of both ends on one
    /// key, ordered above both meshes' keys.
    async fn two_meshes_converge(a_rotations: u32, b_rotations: u32) -> KeyInfo {
        let nodes = [
            node_at("a", 1_000).await,
            node_at("a2", 2_000).await,
            node_at("a3", 3_000).await,
            node_at("b", 4_000).await,
            node_at("b2", 5_000).await,
        ];
        let [(a, ..), (a2, ..), (a3, ..), (b, ..), (b2, ..)] = &nodes;
        a2.add_peer(&a.pairing_string().await).await.unwrap();
        a3.add_peer(&a.pairing_string().await).await.unwrap();
        b2.add_peer(&b.pairing_string().await).await.unwrap();
        let (va, vb) = (&nodes[0].2, &nodes[3].2);
        for _ in 0..a_rotations {
            rotate(a).await.unwrap();
        }
        for _ in 0..b_rotations {
            rotate(b).await.unwrap();
        }
        let vb2 = &nodes[4].2;
        eventually("b2 follows b", || {
            vb2.current_key().id == vb.current_key().id
        })
        .await;
        let (ea, eb) = (va.current_key().epoch, vb.current_key().epoch);
        b.add_peer(&a.pairing_string().await).await.unwrap();
        eventually("every machine on one key above both meshes'", || {
            let k = va.current_key();
            k.epoch >= ea.max(eb) && nodes.iter().all(|n| n.2.current_key().id == k.id)
        })
        .await;
        let k = va.current_key();
        for n in &nodes {
            n.0.shutdown().await;
        }
        k
    }

    /// 183 R1 (S5): the smaller mesh has the higher epoch: the owner rotates above it
    /// (`max(theirs, ours) + 1`), so the bigger mesh's lineage wins and the smaller mesh's
    /// other machine follows.
    #[tokio::test]
    async fn pairing_into_a_smaller_mesh_with_a_higher_epoch_converges() {
        let k = two_meshes_converge(0, 3).await;
        assert_eq!(k.epoch, 4);
    }

    /// 183 R1 (S5): the other direction: the owner's epoch is already the higher; no extra
    /// rotation, everyone takes the owner's key.
    #[tokio::test]
    async fn pairing_into_a_smaller_mesh_with_a_lower_epoch_converges() {
        let k = two_meshes_converge(3, 0).await;
        assert_eq!(k.epoch, 3);
    }

    /// 183 R1 (S9): a peer whose `KeyEpoch` landed before another connection's `on_connect` is
    /// not called an older build 5 s later.
    #[tokio::test]
    async fn a_peer_that_spoke_is_never_called_an_older_build() {
        let (a, _da, va) = node_at("a", 1_000).await;
        let (b, _db, _vb) = node_at("b", 3_000).await;
        b.add_peer(&a.pairing_string().await).await.unwrap();
        eventually("b spoke", || {
            lock(&a.keyring.peers)
                .get(&eid(&b))
                .is_some_and(|p| p.spoke)
        })
        .await;
        rotate(&a).await.unwrap();
        let conn = a.control_conn(&eid(&b)).unwrap();
        on_connect(&a, &conn).await;
        tick(&a, now_ms() + OLD_BUILD_MS + 1_000).await;
        let st = va.rotation().unwrap();
        assert!(!st.peers.iter().find(|p| p.id == b.id.0).unwrap().old_build);
        a.shutdown().await;
        b.shutdown().await;
    }
}
