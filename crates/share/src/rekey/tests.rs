//! P0-6 mesh races against fakes (210 §10 M2 exit): a vault replica with partitions and
//! per-row last-writer-wins, and an edge that applies `Rekey`s the way §5.7 says.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use share_proto::{EdgeIdentity, SpkiHash};

use super::*;
use crate::holder::{self, CsrHeal, Settle};
use crate::keys::{NameKey, NameKeyStore};

const SHARE_KEY: &str = "mesh/share_key";
const MIN: u64 = 60_000;

#[derive(Clone)]
struct Cell {
    version: u32,
    value: Vec<u8>,
    hlc: u64,
}

/// The replicated vault rows every engine syncs with.
#[derive(Default)]
struct Mesh {
    rows: Mutex<HashMap<String, Cell>>,
    clock: AtomicU64,
    members: Mutex<Vec<String>>,
}

/// One engine's replica: writes land locally, and reach the mesh while online or at `sync`.
/// Two writes of one `(row, version)` in a sync gap keep the later hlc, as the vault's
/// per-column LWW does.
struct Replica {
    mesh: Arc<Mesh>,
    local: Mutex<HashMap<String, Cell>>,
    online: AtomicBool,
}

impl Replica {
    fn new(mesh: &Arc<Mesh>) -> Arc<Self> {
        let r = Arc::new(Self {
            mesh: mesh.clone(),
            local: Mutex::new(HashMap::new()),
            online: AtomicBool::new(true),
        });
        r.sync();
        r
    }

    fn partition(&self) {
        self.online.store(false, Ordering::SeqCst);
    }

    fn heal(&self) {
        self.online.store(true, Ordering::SeqCst);
        self.sync();
    }

    fn sync(&self) {
        let mut g = self.mesh.rows.lock().unwrap();
        let mut l = self.local.lock().unwrap();
        for (k, c) in l.iter() {
            let keep = match g.get(k) {
                Some(o) => (c.version, c.hlc) > (o.version, o.hlc),
                None => true,
            };
            if keep {
                g.insert(k.clone(), c.clone());
            }
        }
        *l = g.clone();
    }

    fn get(&self, row: &str) -> Option<Cell> {
        if self.online.load(Ordering::SeqCst) {
            self.sync();
        }
        self.local.lock().unwrap().get(row).cloned()
    }

    fn cas(&self, row: &str, value: Vec<u8>, expected: Option<u32>) -> Option<u32> {
        let online = self.online.load(Ordering::SeqCst);
        if online {
            self.sync();
        }
        let v = {
            let mut l = self.local.lock().unwrap();
            if l.get(row).map(|c| c.version) != expected {
                return None;
            }
            let version = expected.unwrap_or(0) + 1;
            let hlc = self.mesh.clock.fetch_add(1, Ordering::SeqCst) + 1;
            l.insert(row.to_owned(), Cell { version, value, hlc });
            version
        };
        if online {
            self.sync();
        }
        Some(v)
    }
}

#[async_trait::async_trait]
impl MeshKeys for Replica {
    fn current(&self) -> anyhow::Result<Option<(u32, SecretKey)>> {
        Ok(self.get(SHARE_KEY).map(|c| {
            let b: [u8; 32] = c.value.as_slice().try_into().unwrap();
            (c.version, SecretKey::from_bytes(&b))
        }))
    }

    async fn replace(&self, fresh: &SecretKey, expected: u32) -> anyhow::Result<Option<u32>> {
        Ok(self.cas(SHARE_KEY, fresh.to_bytes().to_vec(), Some(expected)))
    }

    fn vault_members(&self) -> anyhow::Result<Vec<String>> {
        Ok(self.mesh.members.lock().unwrap().clone())
    }

    fn synced(&self) -> bool {
        // `synced()` is set once per start: a partitioned engine still says synced (F-m2).
        true
    }
}

#[async_trait::async_trait]
impl NameKeyStore for Replica {
    async fn get(&self, domain: &str, name: &str) -> anyhow::Result<Option<NameKey>> {
        Ok(Replica::get(self, &crate::keys::vault_row(domain, name))
            .map(|c| serde_json::from_slice(&c.value).unwrap()))
    }

    async fn create(&self, domain: &str, name: &str, key: NameKey) -> anyhow::Result<NameKey> {
        let row = crate::keys::vault_row(domain, name);
        self.cas(&row, serde_json::to_vec(&key)?, None);
        Ok(NameKeyStore::get(self, domain, name).await?.expect("row"))
    }
}

struct NoWait;

#[async_trait::async_trait]
impl Settle for NoWait {
    async fn settle(&self) {}
}

/// An edge as §5.7 describes it.
struct Edge {
    id: EdgeIdentity,
    st: Mutex<EdgeSt>,
}

struct EdgeSt {
    mesh: PublicKey,
    /// old → new of every applied rekey.
    moved: HashMap<PublicKey, PublicKey>,
    revoked: Vec<PublicKey>,
    rekeys: u32,
    down: bool,
    spki: Option<SpkiHash>,
}

impl Edge {
    fn new(mesh: PublicKey) -> Arc<Self> {
        Arc::new(Self {
            id: EdgeIdentity {
                edge: fresh_secret().public(),
                domain: "tun.zip".into(),
            },
            st: Mutex::new(EdgeSt {
                mesh,
                moved: HashMap::new(),
                revoked: Vec::new(),
                rekeys: 0,
                down: false,
                spki: None,
            }),
        })
    }

    fn row(&self) -> EdgeRow {
        EdgeRow::new(self.id.edge, &self.id.domain, "ticket")
    }

    fn mesh(&self) -> PublicKey {
        self.st.lock().unwrap().mesh
    }

    fn revoked(&self) -> Vec<PublicKey> {
        self.st.lock().unwrap().revoked.clone()
    }

    /// Where `key` leads now: `Ok` = it is the mesh, `Err(to)` = it moved, `None` = unknown.
    fn follow(st: &EdgeSt, key: PublicKey) -> Option<Result<PublicKey, PublicKey>> {
        if key == st.mesh {
            return Some(Ok(key));
        }
        let mut k = key;
        let mut seen = 0;
        while let Some(n) = st.moved.get(&k) {
            k = *n;
            seen += 1;
            if seen > 16 {
                break;
            }
        }
        (k == st.mesh && k != key).then_some(Err(k))
    }

    /// `POST /v1/names/{name}/csr`: first CSR sticks; another without `rotate` is `csr_mismatch`.
    fn csr(&self, der: &[u8], rotate: bool) -> Result<(), SpkiHash> {
        let spki = share_proto::check_csr(der, "sky", "tun.zip").unwrap().spki;
        let mut st = self.st.lock().unwrap();
        match st.spki {
            Some(s) if s != spki && !rotate => Err(s),
            _ => {
                st.spki = Some(spki);
                Ok(())
            }
        }
    }
}

struct Edges(Vec<Arc<Edge>>);

impl Edges {
    fn of(&self, row: &EdgeRow) -> anyhow::Result<&Arc<Edge>> {
        let e = self.0.iter().find(|e| e.row().id == row.id).context("no such edge")?;
        anyhow::ensure!(!e.st.lock().unwrap().down, "edge unreachable");
        Ok(e)
    }
}

#[async_trait::async_trait]
impl RekeyEdges for Edges {
    async fn revoke(&self, edge: &EdgeRow, key: &SecretKey, engines: &[PublicKey]) -> anyhow::Result<()> {
        let e = self.of(edge)?;
        let mut st = e.st.lock().unwrap();
        anyhow::ensure!(st.mesh == key.public(), "hello: not this edge's mesh");
        st.revoked.extend_from_slice(engines);
        Ok(())
    }

    async fn rekey(&self, edge: &EdgeRow, _key: &SecretKey, signed: &SignedRekey) -> anyhow::Result<RekeyAnswer> {
        let e = self.of(edge)?;
        let r = Rekey::verify(signed, &e.id).map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut st = e.st.lock().unwrap();
        match Edge::follow(&st, r.old) {
            Some(Ok(_)) => {
                st.moved.insert(r.old, r.new);
                st.mesh = r.new;
                st.revoked = r.revoke;
                st.rekeys += 1;
                Ok(RekeyAnswer::Accepted)
            }
            Some(Err(to)) => Ok(RekeyAnswer::Rekeyed(to)),
            None => anyhow::bail!("403 bad_signature: unknown mesh"),
        }
    }

    async fn mesh_id(&self, edge: &EdgeRow, key: &SecretKey) -> anyhow::Result<Result<PublicKey, PublicKey>> {
        let e = self.of(edge)?;
        let st = e.st.lock().unwrap();
        Edge::follow(&st, key.public()).context("403 bad_delegation: unknown mesh")
    }
}

struct Engine {
    id: String,
    store: Store,
    stash: Stash,
    vault: Arc<Replica>,
    _dir: tempfile::TempDir,
}

impl Engine {
    async fn new(mesh: &Arc<Mesh>, edges: &[Arc<Edge>]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("share.sqlite")).await.unwrap();
        for e in edges {
            store.put_share_edge(&e.row(), 1).await.unwrap();
        }
        Self {
            id: fresh_secret().public().to_string(),
            stash: Stash::in_data_dir(dir.path()),
            store,
            vault: Replica::new(mesh),
            _dir: dir,
        }
    }

    fn rekeyer<'a>(&'a self, edges: &'a Edges) -> Rekeyer<'a> {
        Rekeyer {
            store: &self.store,
            stash: &self.stash,
            keys: &*self.vault,
            edges,
            settle_ms: MIN,
            heal_ms: 60 * MIN,
        }
    }

    async fn state(&self) -> RekeyState {
        self.store.rekeys().await.unwrap()[0].state
    }
}

/// A mesh with `mesh/share_key` minted and the members `ids`; returns `K`.
fn mint(mesh: &Arc<Mesh>, first: &Replica, ids: &[&str]) -> SecretKey {
    let k = fresh_secret();
    first.cas(SHARE_KEY, k.to_bytes().to_vec(), None).unwrap();
    *mesh.members.lock().unwrap() = ids.iter().map(|s| s.to_string()).collect();
    k
}

/// P0-6: the hub's own sharing switch is off; it still revokes and rekeys at every edge in
/// `share_edges`, and an engine without a `share_edges` row does nothing.
#[tokio::test]
async fn rekey_runs_with_the_hub_switch_off_only_where_registered() {
    let mesh = Arc::new(Mesh::default());
    let (e1, e2) = (Edge::new(fresh_secret().public()), Edge::new(fresh_secret().public()));
    let hub = Engine::new(&mesh, &[e1.clone(), e2.clone()]).await;
    let removed = fresh_secret().public().to_string();
    let k = mint(&mesh, &hub.vault, &[&hub.id]);
    for e in [&e1, &e2] {
        e.st.lock().unwrap().mesh = k.public();
    }
    assert!(!hub.store.enabled().await.unwrap(), "switch off");
    let edges = Edges(vec![e1.clone(), e2.clone()]);
    let rk = hub.rekeyer(&edges);

    let row = rk.on_removal(vec![removed.clone()], 0).await.unwrap().expect("row");
    assert_eq!(row.state, RekeyState::Revoking);
    // Same removal again (subscription + RemovedPeers): one row.
    rk.on_removal(vec![removed.clone()], 1).await.unwrap();
    assert_eq!(hub.store.rekeys().await.unwrap().len(), 1);

    assert!(rk.run(0).await.unwrap().is_empty());
    assert_eq!(hub.state().await, RekeyState::Settling, "revoked, vault ok, written");
    for e in [&e1, &e2] {
        assert_eq!(e.revoked(), vec![removed.parse::<PublicKey>().unwrap()]);
        assert_eq!(e.mesh(), k.public(), "not before the settle");
    }
    let path = hub._dir.path().join("share").join("rekey.stash");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let done = rk.run(MIN).await.unwrap();
    let (_, s) = hub.vault.current().unwrap().unwrap();
    assert_eq!(done, vec![s.public()]);
    assert_ne!(s.public(), k.public());
    for e in [&e1, &e2] {
        assert_eq!(e.mesh(), s.public(), "GET /v1/mesh equals the vault key");
        assert_eq!(e.revoked(), vec![removed.parse::<PublicKey>().unwrap()]);
    }
    // The stash outlives `done` by the heal window, then goes.
    assert!(path.exists());
    rk.prune(MIN + 60 * MIN).await.unwrap();
    assert!(!path.exists());

    // No `share_edges` row: no duty, no row.
    let lone = Engine::new(&mesh, &[]).await;
    let none = Edges(Vec::new());
    assert!(lone.rekeyer(&none).on_removal(vec![removed], 0).await.unwrap().is_none());
    assert!(lone.store.rekeys().await.unwrap().is_empty());
}

/// P0-6: the vault rotation after the unpair fails. The edge revokes at once; the rekey waits in
/// `waiting_vault`, Settings shows it after 10 min, and it resumes once the rotation lands.
/// An unreachable edge backs off and resumes.
#[tokio::test]
async fn removal_with_a_failed_vault_rotation_waits_and_resumes() {
    let mesh = Arc::new(Mesh::default());
    let edge = Edge::new(fresh_secret().public());
    let hub = Engine::new(&mesh, std::slice::from_ref(&edge)).await;
    let removed = fresh_secret().public().to_string();
    let k = mint(&mesh, &hub.vault, &[&hub.id, &removed]);
    edge.st.lock().unwrap().mesh = k.public();
    let edges = Edges(vec![edge.clone()]);
    let rk = hub.rekeyer(&edges);

    edge.st.lock().unwrap().down = true;
    rk.on_removal(vec![removed.clone()], 0).await.unwrap().unwrap();
    rk.run(0).await.unwrap();
    let row = &hub.store.rekeys().await.unwrap()[0];
    assert_eq!((row.state, row.attempts, row.next_at_ms), (RekeyState::Revoking, 1, MIN));
    edge.st.lock().unwrap().down = false;
    rk.run(MIN).await.unwrap();
    assert_eq!(edge.revoked().len(), 1, "revoked before any vault work");
    assert_eq!(hub.state().await, RekeyState::WaitingVault);

    rk.run(5 * MIN).await.unwrap();
    let row = &hub.store.rekeys().await.unwrap()[0];
    assert_eq!(row.state, RekeyState::WaitingVault);
    assert_eq!(row.attention(5 * MIN, 60 * MIN), None);
    assert_eq!(row.attention(11 * MIN, 60 * MIN), Some(Attention::VaultStalled));
    assert_eq!(hub.vault.current().unwrap().unwrap().1.public(), k.public(), "no rekey yet");

    // The rotation finally excludes the removed engine.
    *mesh.members.lock().unwrap() = vec![hub.id.clone()];
    rk.run(12 * MIN).await.unwrap();
    assert_eq!(hub.state().await, RekeyState::Settling);
    assert_eq!(rk.run(13 * MIN).await.unwrap().len(), 1);
    assert_eq!(edge.mesh(), hub.vault.current().unwrap().unwrap().1.public());
}

/// P0-6: two holders after a partition (each its own holder, F-m2) both rekey. The one whose
/// ciphertext lost the LWW sent first, so the edge holds a key the vault no longer has; the
/// other sees `409 {to}` it cannot sign for (contested) until the loser heals by chaining from
/// its stash. Every rekey ends with `GET /v1/mesh` = the vault's key.
#[tokio::test]
async fn rekey_after_a_partition_heal_with_two_holders_chains() {
    let mesh = Arc::new(Mesh::default());
    let edge = Edge::new(fresh_secret().public());
    let a = Engine::new(&mesh, std::slice::from_ref(&edge)).await;
    let b = Engine::new(&mesh, std::slice::from_ref(&edge)).await;
    let removed = fresh_secret().public().to_string();
    let k = mint(&mesh, &a.vault, &[&a.id, &b.id]);
    b.vault.sync();
    edge.st.lock().unwrap().mesh = k.public();
    let edges = Edges(vec![edge.clone()]);
    let (ra, rb) = (a.rekeyer(&edges), b.rekeyer(&edges));

    a.vault.partition();
    b.vault.partition();
    for e in [&a, &b] {
        e.rekeyer(&edges).on_removal(vec![removed.clone()], 0).await.unwrap().unwrap();
    }
    // Both write in the sync gap; B sends while partitioned, so the edge moves K → Kb.
    ra.run(0).await.unwrap();
    rb.run(0).await.unwrap();
    assert_eq!(rb.run(MIN).await.unwrap().len(), 1, "B done against its own view");
    let kb = b.vault.current().unwrap().unwrap().1.public();
    assert_eq!(edge.mesh(), kb);

    // Heal with A's write winning instead: make A's cell the later one.
    {
        let mut l = a.vault.local.lock().unwrap();
        l.get_mut(SHARE_KEY).unwrap().hlc = 1_000;
    }
    a.vault.heal();
    b.vault.heal();
    let ka = a.vault.current().unwrap().unwrap().1.public();
    assert_eq!(b.vault.current().unwrap().unwrap().1.public(), ka, "settled on A's key");
    assert_ne!(ka, kb);

    // A sends K → Ka: the edge answers `to = Kb`, which A cannot sign for.
    ra.run(MIN).await.unwrap();
    assert_eq!(a.state().await, RekeyState::Contested);
    assert_eq!(edge.mesh(), kb);
    let row = &a.store.rekeys().await.unwrap()[0];
    assert_eq!(row.attention(MIN + 60 * MIN, 60 * MIN), Some(Attention::Contested));

    // B's stash still holds Kb (done rows keep it for the heal window): B heals Kb → Ka.
    assert_eq!(rb.heal(2 * MIN).await.unwrap(), vec![edge.row().id]);
    assert_eq!(edge.mesh(), ka);
    assert_eq!(edge.revoked(), vec![removed.parse::<PublicKey>().unwrap()], "revoke list kept");

    // A confirms; B's own row, re-run, sees the vault moved and confirms the settled key too.
    assert_eq!(ra.run(3 * MIN).await.unwrap(), vec![ka]);
    rb.run(3 * MIN).await.unwrap();
    assert_eq!(edge.mesh(), a.vault.current().unwrap().unwrap().1.public());
}

/// The other order of the same race: the winner sends first; the loser gets `409 {to = S}`
/// after the heal and simply confirms.
#[tokio::test]
async fn rekey_loser_confirms_the_winners_key() {
    let mesh = Arc::new(Mesh::default());
    let edge = Edge::new(fresh_secret().public());
    let a = Engine::new(&mesh, std::slice::from_ref(&edge)).await;
    let b = Engine::new(&mesh, std::slice::from_ref(&edge)).await;
    let removed = fresh_secret().public().to_string();
    let k = mint(&mesh, &a.vault, &[&a.id, &b.id]);
    b.vault.sync();
    edge.st.lock().unwrap().mesh = k.public();
    let edges = Edges(vec![edge.clone()]);
    let (ra, rb) = (a.rekeyer(&edges), b.rekeyer(&edges));
    a.vault.partition();
    b.vault.partition();
    for e in [&a, &b] {
        e.rekeyer(&edges).on_removal(vec![removed.clone()], 0).await.unwrap();
    }
    ra.run(0).await.unwrap();
    rb.run(0).await.unwrap();
    // B's write is later: B wins the LWW; B sends first too.
    rb.run(MIN).await.unwrap();
    let kb = edge.mesh();
    a.vault.heal();
    b.vault.heal();
    assert_eq!(a.vault.current().unwrap().unwrap().1.public(), kb);
    // A's settled key changed under it: it settles again, sends, gets `to = S`, confirms.
    assert_eq!(ra.run(MIN).await.unwrap(), vec![kb]);
    assert_eq!(edge.mesh(), kb);
    assert_eq!(edge.st.lock().unwrap().rekeys, 1, "one rekey applied");
}

/// P0-6: two machines publish a first name in one sync gap. Each is its own holder, so each
/// creates a name key and posts its CSR; the edge keeps the first and answers the second
/// `csr_mismatch`. After the sync the vault keeps one row; within `csr_heal_secs` the holder
/// rotates the name to it, so the edge's SPKI equals the vault row on every machine.
#[tokio::test]
async fn first_name_in_one_sync_gap_heals_to_the_vault_row() {
    let mesh = Arc::new(Mesh::default());
    let edge = Edge::new(fresh_secret().public());
    let a = Replica::new(&mesh);
    let b = Replica::new(&mesh);
    a.partition();
    b.partition();
    let ka = holder::ensure_name_key(&*a, &NoWait, "tun.zip", "sky").await.unwrap();
    let kb = holder::ensure_name_key(&*b, &NoWait, "tun.zip", "sky").await.unwrap();
    assert_ne!(ka, kb, "two keys in one sync gap");
    edge.csr(&ka.csr_der, false).unwrap();
    let mismatch = edge.csr(&kb.csr_der, false).unwrap_err();
    a.heal();
    b.heal();
    let row_a = NameKeyStore::get(&*a, "tun.zip", "sky").await.unwrap().unwrap();
    let row_b = NameKeyStore::get(&*b, "tun.zip", "sky").await.unwrap().unwrap();
    assert_eq!(row_a, row_b, "one settled row");
    assert_eq!(row_a, kb, "the later write won");
    let row_spki = holder::row_spki(&row_a, "sky", "tun.zip").unwrap();
    assert_ne!(mismatch, row_spki, "the edge holds the losing key");

    let since = match holder::csr_heal(Some(&mismatch), &row_spki, None, 0, holder::CSR_HEAL_MS) {
        CsrHeal::Wait { since } => since,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        holder::csr_heal(Some(&mismatch), &row_spki, Some(since), holder::CSR_HEAL_MS, holder::CSR_HEAL_MS),
        CsrHeal::Rotate
    );
    edge.csr(&row_a.csr_der, true).unwrap();
    let edge_spki = edge.st.lock().unwrap().spki.unwrap();
    for r in [&a, &b] {
        let row = NameKeyStore::get(&**r, "tun.zip", "sky").await.unwrap().unwrap();
        assert_eq!(holder::row_spki(&row, "sky", "tun.zip").unwrap(), edge_spki);
    }
    // A later ensure returns the settled row, never a third key.
    assert_eq!(holder::ensure_name_key(&*a, &NoWait, "tun.zip", "sky").await.unwrap(), row_a);
}

#[test]
fn unhandled_removals_skip_known_rows() {
    let row = RekeyRow::new(vec!["x".into()], 0);
    assert_eq!(unhandled_removals(&["x".into(), "y".into()], &[row]), vec!["y".to_string()]);
}
