//! 210 §5.7 (D21, D22, F-M2, F-M3): removal duties. When an engine leaves the family after
//! `mesh/share_key` exists, the lease holder revokes it at every edge in the replicated
//! `share_edges` table, waits for the vault's key to exclude it, writes a fresh mesh share key,
//! lets the row settle, and moves the mesh to the settled key at every edge with a signed
//! `Rekey`. The row (`share_rekeys`) is replicated so a new holder resumes it; the secrets live
//! only in this engine's 0600 stash (`<data_dir>/share/rekey.stash`) until every edge's mesh id
//! equals the vault's key.
//!
//! The machine runs whatever the holder's own "Allow public sharing" switch says (D22), and only
//! when `share_edges` has a live row. Every seam is a trait so the P0-6 races run against fakes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context as _;
use data_encoding::BASE64URL_NOPAD;
use serde::{Deserialize, Serialize};
use share_proto::{
    ErrorCode, PublicKey, Rekey, RekeyConflict, SecretKey, SignedRekey, limits, rekey_conflict,
};
use sqlx::Row as _;
use zeroize::Zeroizing;

use crate::store::Store;

/// After this long in `waiting_vault`, Settings shows the stalled vault rotation (§5.7).
pub const VAULT_STALL_MS: u64 = 10 * 60_000;

/// One `share_edges` row (§5.8): an edge where an engine of the mesh received a `welcome`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeRow {
    /// `"<edge>/<domain>"`.
    pub id: String,
    /// The edge's endpoint id.
    pub edge: PublicKey,
    /// Its domain.
    pub domain: String,
    /// The ticket to dial it.
    pub ticket: String,
}

impl EdgeRow {
    /// The row for `edge`/`domain`.
    pub fn new(edge: PublicKey, domain: &str, ticket: &str) -> Self {
        Self {
            id: format!("{edge}/{domain}"),
            edge,
            domain: domain.to_owned(),
            ticket: ticket.to_owned(),
        }
    }
}

/// The state column of `share_rekeys` (§5.7 table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RekeyState {
    /// `POST /v1/mesh/revoke` at each edge.
    Revoking,
    /// Waiting for the vault's key to exclude the removed engines.
    WaitingVault,
    /// Stash `K`/`K'`, compare-and-set `mesh/share_key`.
    Writing,
    /// Wait `synced()` + settle, re-read the settled key.
    Settling,
    /// `POST /v1/mesh/rekey` at each edge.
    Sending,
    /// `GET /v1/mesh` until the edge's mesh id is the settled key.
    Confirming,
    /// The edge holds a key nobody here can sign for.
    Contested,
    /// Finished.
    Done,
}

impl RekeyState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Revoking => "revoking",
            Self::WaitingVault => "waiting_vault",
            Self::Writing => "writing",
            Self::Settling => "settling",
            Self::Sending => "sending",
            Self::Confirming => "confirming",
            Self::Contested => "contested",
            Self::Done => "done",
        }
    }

    fn parse(s: &str) -> anyhow::Result<Self> {
        serde_json::from_value(serde_json::Value::String(s.to_owned()))
            .with_context(|| format!("unknown rekey state {s}"))
    }
}

/// One `share_rekeys` row: never a secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RekeyRow {
    /// UUIDv7.
    pub id: String,
    /// Removed engine ids.
    pub removed: Vec<String>,
    /// When the tombstone was seen.
    pub tombstone_ms: u64,
    /// The key the edges held before (`K`).
    pub from_pub: Option<PublicKey>,
    /// The settled key (`S`).
    pub to_pub: Option<PublicKey>,
    /// State.
    pub state: RekeyState,
    /// Edge row ids done in the current state.
    pub edges_done: Vec<String>,
    /// Failed attempts in the current state (backoff).
    pub attempts: u32,
    /// Not before.
    pub next_at_ms: u64,
    /// Last state change.
    pub updated_ms: u64,
}

impl RekeyRow {
    /// A fresh row in `revoking`.
    pub fn new(removed: Vec<String>, tombstone_ms: u64) -> Self {
        Self {
            id: uuid::Uuid::now_v7().to_string(),
            removed,
            tombstone_ms,
            from_pub: None,
            to_pub: None,
            state: RekeyState::Revoking,
            edges_done: Vec::new(),
            attempts: 0,
            next_at_ms: tombstone_ms,
            updated_ms: tombstone_ms,
        }
    }

    fn enter(&mut self, state: RekeyState, now: u64) {
        self.state = state;
        self.edges_done.clear();
        self.attempts = 0;
        self.next_at_ms = now;
        self.updated_ms = now;
    }

    fn backoff(&mut self, now: u64) {
        let shift = self.attempts.min(10);
        self.attempts += 1;
        self.next_at_ms = now
            + (limits::REKEY_BACKOFF_MIN_MS << shift).min(limits::REKEY_BACKOFF_MAX_MS);
    }

    /// What Settings shows for this row at `now`, if anything (§5.7).
    pub fn attention(&self, now: u64, heal_ms: u64) -> Option<Attention> {
        match self.state {
            RekeyState::WaitingVault if now.saturating_sub(self.tombstone_ms) >= VAULT_STALL_MS => {
                Some(Attention::VaultStalled)
            }
            RekeyState::Contested if now.saturating_sub(self.updated_ms) >= heal_ms => {
                Some(Attention::Contested)
            }
            _ => None,
        }
    }
}

/// A Settings/attention item for a rekey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attention {
    /// "Vault key rotation after removing <machine> has not finished: names are revoked at the
    /// edge but not yet rekeyed."
    VaultStalled,
    /// "Your names were moved by another machine: contact <operator>."
    Contested,
}

/// `share_rekeys`/`share_edges` access on the share store.
impl Store {
    /// Live `share_edges` rows (not deleted).
    pub async fn share_edges(&self) -> anyhow::Result<Vec<EdgeRow>> {
        let rows = sqlx::query(
            "SELECT id, edge, domain, ticket FROM share_edges WHERE deleted_ms IS NULL ORDER BY id",
        )
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(EdgeRow {
                    id: r.get("id"),
                    edge: r.get::<String, _>("edge").parse().context("share_edges.edge")?,
                    domain: r.get("domain"),
                    ticket: r.get("ticket"),
                })
            })
            .collect()
    }

    /// Record (or refresh) the edge where this engine received a `welcome` (D22).
    pub async fn put_share_edge(&self, e: &EdgeRow, now_ms: u64) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO share_edges (id, edge, domain, ticket, registered_ms, deleted_ms)
             VALUES (?, ?, ?, ?, ?, NULL)
             ON CONFLICT(id) DO UPDATE SET ticket = excluded.ticket, deleted_ms = NULL
             WHERE share_edges.ticket <> excluded.ticket OR share_edges.deleted_ms IS NOT NULL",
        )
        .bind(&e.id)
        .bind(e.edge.to_string())
        .bind(&e.domain)
        .bind(&e.ticket)
        .bind(now_ms as i64)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Mark an edge row deleted (last name released there, or the edge answered an unknown mesh).
    pub async fn delete_share_edge(&self, id: &str, now_ms: u64) -> anyhow::Result<()> {
        sqlx::query("UPDATE share_edges SET deleted_ms = ? WHERE id = ? AND deleted_ms IS NULL")
            .bind(now_ms as i64)
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Write a rekey row.
    pub async fn put_rekey(&self, r: &RekeyRow) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO share_rekeys (id, removed, tombstone_ms, from_pub, to_pub, state,
               edges_done, attempts, next_at_ms, updated_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET removed = excluded.removed, from_pub = excluded.from_pub,
               to_pub = excluded.to_pub, state = excluded.state, edges_done = excluded.edges_done,
               attempts = excluded.attempts, next_at_ms = excluded.next_at_ms,
               updated_ms = excluded.updated_ms",
        )
        .bind(&r.id)
        .bind(serde_json::to_string(&r.removed)?)
        .bind(r.tombstone_ms as i64)
        .bind(r.from_pub.map(|k| k.to_string()))
        .bind(r.to_pub.map(|k| k.to_string()))
        .bind(r.state.as_str())
        .bind(serde_json::to_string(&r.edges_done)?)
        .bind(i64::from(r.attempts))
        .bind(r.next_at_ms as i64)
        .bind(r.updated_ms as i64)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Every rekey row, oldest first.
    pub async fn rekeys(&self) -> anyhow::Result<Vec<RekeyRow>> {
        let rows = sqlx::query("SELECT * FROM share_rekeys ORDER BY id")
            .fetch_all(self.pool())
            .await?;
        rows.into_iter()
            .map(|r| {
                let key = |c: &str| -> anyhow::Result<Option<PublicKey>> {
                    r.get::<Option<String>, _>(c)
                        .map(|s| s.parse().with_context(|| format!("share_rekeys.{c}")))
                        .transpose()
                };
                Ok(RekeyRow {
                    id: r.get("id"),
                    removed: serde_json::from_str(r.get("removed"))?,
                    tombstone_ms: r.get::<i64, _>("tombstone_ms") as u64,
                    from_pub: key("from_pub")?,
                    to_pub: key("to_pub")?,
                    state: RekeyState::parse(r.get("state"))?,
                    edges_done: serde_json::from_str(r.get("edges_done"))?,
                    attempts: r.get::<i64, _>("attempts") as u32,
                    next_at_ms: r.get::<i64, _>("next_at_ms") as u64,
                    updated_ms: r.get::<i64, _>("updated_ms") as u64,
                })
            })
            .collect()
    }
}

/// The local stash of rekey secrets (§5.7): `old` (`K`) and `pending` (`K'`) per rekey, in a
/// 0600 file written with `platform::files::write_private`. Never replicated.
pub struct Stash {
    path: PathBuf,
    lock: Mutex<()>,
}

#[derive(Default, Serialize, Deserialize)]
struct StashFile {
    entries: Vec<StashEntry>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StashEntry {
    rekey: String,
    role: String,
    secret: String,
}

impl Stash {
    /// The stash under `<data_dir>/share/rekey.stash`.
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self::at(data_dir.join("share").join("rekey.stash"))
    }

    /// A stash at `path`.
    pub fn at(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    fn load(&self) -> anyhow::Result<StashFile> {
        match std::fs::read(&self.path) {
            Ok(b) => Ok(serde_json::from_slice(&b).context("rekey stash")?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StashFile::default()),
            Err(e) => Err(e).context("rekey stash"),
        }
    }

    fn save(&self, f: &StashFile) -> anyhow::Result<()> {
        if f.entries.is_empty() {
            match std::fs::remove_file(&self.path) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e).context("rekey stash"),
            }
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = Zeroizing::new(serde_json::to_vec(f)?);
        platform::files::write_private(&self.path, &bytes).context("rekey stash")
    }

    fn edit<R>(&self, f: impl FnOnce(&mut StashFile) -> R) -> anyhow::Result<R> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut file = self.load()?;
        let r = f(&mut file);
        self.save(&file)?;
        Ok(r)
    }

    /// Store `secret` for `rekey` as `role` (`old` | `pending`), replacing an earlier one.
    pub fn put(&self, rekey: &str, role: &str, secret: &SecretKey) -> anyhow::Result<()> {
        let secret = BASE64URL_NOPAD.encode(&secret.to_bytes());
        self.edit(|f| {
            f.entries.retain(|e| !(e.rekey == rekey && e.role == role));
            f.entries.push(StashEntry {
                rekey: rekey.to_owned(),
                role: role.to_owned(),
                secret,
            });
        })
    }

    /// The secret stored for `rekey`/`role`.
    pub fn get(&self, rekey: &str, role: &str) -> anyhow::Result<Option<SecretKey>> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        self.load()?
            .entries
            .iter()
            .find(|e| e.rekey == rekey && e.role == role)
            .map(|e| decode_secret(&e.secret))
            .transpose()
    }

    /// Drop one entry.
    pub fn drop_role(&self, rekey: &str, role: &str) -> anyhow::Result<()> {
        self.edit(|f| f.entries.retain(|e| !(e.rekey == rekey && e.role == role)))
    }

    /// Drop every entry of `rekey`.
    pub fn drop_rekey(&self, rekey: &str) -> anyhow::Result<()> {
        self.edit(|f| f.entries.retain(|e| e.rekey != rekey))
    }

    /// Every stashed secret (any rekey, any role).
    pub fn secrets(&self) -> anyhow::Result<Vec<SecretKey>> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        self.load()?
            .entries
            .iter()
            .map(|e| decode_secret(&e.secret))
            .collect()
    }

    /// The stashed secret whose public half is `key`.
    pub fn find(&self, key: &PublicKey) -> anyhow::Result<Option<SecretKey>> {
        Ok(self.secrets()?.into_iter().find(|s| s.public() == *key))
    }

    /// Nothing stashed.
    pub fn is_empty(&self) -> anyhow::Result<bool> {
        let _g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        Ok(self.load()?.entries.is_empty())
    }
}

fn decode_secret(s: &str) -> anyhow::Result<SecretKey> {
    let bytes = Zeroizing::new(BASE64URL_NOPAD.decode(s.as_bytes()).context("stash secret")?);
    let arr: [u8; 32] = bytes.as_slice().try_into().context("stash secret length")?;
    Ok(SecretKey::from_bytes(&arr))
}

/// A fresh 32-byte Ed25519 secret from getrandom (via UUIDv4, as the rest of the crate).
pub fn fresh_secret() -> SecretKey {
    let a = uuid::Uuid::new_v4().into_bytes();
    let b = uuid::Uuid::new_v4().into_bytes();
    let mut k = Zeroizing::new([0u8; 32]);
    k[..16].copy_from_slice(&a);
    k[16..].copy_from_slice(&b);
    SecretKey::from_bytes(&k)
}

/// The vault side: `mesh/share_key`, the vault key's members and the sync flag.
#[async_trait::async_trait]
pub trait MeshKeys: Send + Sync {
    /// `mesh/share_key` as this engine's replica sees it: `(version, secret)`.
    fn current(&self) -> anyhow::Result<Option<(u32, SecretKey)>>;
    /// `set_if_version("mesh/share_key", fresh, Some(expected))`: `Ok(None)` = another writer.
    async fn replace(&self, fresh: &SecretKey, expected: u32) -> anyhow::Result<Option<u32>>;
    /// The current vault key's members (`vault.current_key().members`).
    fn vault_members(&self) -> anyhow::Result<Vec<String>>;
    /// `mesh::synced()`.
    fn synced(&self) -> bool;
}

/// What an edge answered to a rekey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RekeyAnswer {
    /// `200 {mesh}`.
    Accepted,
    /// `409 mesh_rekeyed {to}`.
    Rekeyed(PublicKey),
}

/// The edge side of the machine, per `share_edges` row. `key` is the mesh key the call is
/// delegated under (the connection's `hello`).
#[async_trait::async_trait]
pub trait RekeyEdges: Send + Sync {
    /// `POST /v1/mesh/revoke {engines}` under `key`.
    async fn revoke(&self, edge: &EdgeRow, key: &SecretKey, engines: &[PublicKey])
    -> anyhow::Result<()>;
    /// `POST /v1/mesh/rekey`. Needs no delegation of its own beyond the two signatures, but the
    /// connection still `hello`s under `key`.
    async fn rekey(
        &self,
        edge: &EdgeRow,
        key: &SecretKey,
        signed: &SignedRekey,
    ) -> anyhow::Result<RekeyAnswer>;
    /// `GET /v1/mesh` `.mesh`, under `key`. `Ok(Err(to))` when the `hello` itself answered
    /// `mesh_rekeyed {to}` (the edge moved past `key`).
    async fn mesh_id(&self, edge: &EdgeRow, key: &SecretKey)
    -> anyhow::Result<Result<PublicKey, PublicKey>>;
}

/// The result of one [`Rekeyer::step`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// The row changed; step again.
    Progress,
    /// Nothing to do before `next_at_ms` (or until an outside change: sync, vault rotation).
    Wait,
    /// Reached `done`: renew delegations for live peers and rotate name keys (§5.7, §6.3).
    Done {
        /// The settled mesh key.
        mesh: PublicKey,
    },
}

/// The rekey state machine over its seams.
pub struct Rekeyer<'a> {
    /// The share store (rows).
    pub store: &'a Store,
    /// The local stash.
    pub stash: &'a Stash,
    /// The vault.
    pub keys: &'a dyn MeshKeys,
    /// The edges.
    pub edges: &'a dyn RekeyEdges,
    /// `rekey_settle_secs` in ms.
    pub settle_ms: u64,
    /// `rekey_heal_secs` in ms: contested rows surface after it; done rows keep their stash for it.
    pub heal_ms: u64,
}

fn parse_removed(removed: &[String]) -> Vec<PublicKey> {
    removed.iter().filter_map(|e| e.parse().ok()).collect()
}

impl Rekeyer<'_> {
    /// A removal event (§5.7 trigger): a tombstone for `removed` seen at `now`. Starts a row when
    /// `mesh/share_key` exists and `share_edges` has a live row; otherwise nothing (no edge ever
    /// knew the mesh). The caller is the lease holder; its sharing switch is not consulted (D22).
    pub async fn on_removal(&self, removed: Vec<String>, now: u64) -> anyhow::Result<Option<RekeyRow>> {
        if self.keys.current()?.is_none() || self.store.share_edges().await?.is_empty() {
            return Ok(None);
        }
        let open = self.store.rekeys().await?;
        if let Some(r) = open
            .iter()
            .find(|r| r.state != RekeyState::Done && removed.iter().all(|e| r.removed.contains(e)))
        {
            return Ok(Some(r.clone()));
        }
        let row = RekeyRow::new(removed, now);
        self.store.put_rekey(&row).await?;
        Ok(Some(row))
    }

    /// Advance every open row until each waits or finishes; returns the settled keys of rows that
    /// finished in this call.
    pub async fn run(&self, now: u64) -> anyhow::Result<Vec<PublicKey>> {
        let mut done = Vec::new();
        for mut row in self.store.rekeys().await? {
            for _ in 0..32 {
                match self.step(&mut row, now).await? {
                    Step::Progress => continue,
                    Step::Wait => break,
                    Step::Done { mesh } => {
                        done.push(mesh);
                        break;
                    }
                }
            }
        }
        Ok(done)
    }

    async fn save(&self, row: &RekeyRow) -> anyhow::Result<Step> {
        self.store.put_rekey(row).await?;
        Ok(Step::Progress)
    }

    async fn fail(&self, row: &mut RekeyRow, now: u64, what: &str, e: anyhow::Error) -> anyhow::Result<Step> {
        tracing::warn!("share: rekey {} {what}: {e:#}", row.id);
        row.backoff(now);
        self.store.put_rekey(row).await?;
        Ok(Step::Wait)
    }

    /// One transition of `row` at `now`, persisted.
    pub async fn step(&self, row: &mut RekeyRow, now: u64) -> anyhow::Result<Step> {
        if matches!(row.state, RekeyState::Done) || now < row.next_at_ms {
            return Ok(Step::Wait);
        }
        let edges = self.store.share_edges().await?;
        let pending: Vec<&EdgeRow> = edges
            .iter()
            .filter(|e| !row.edges_done.contains(&e.id))
            .collect();
        match row.state {
            RekeyState::Revoking => {
                let Some((_, k)) = self.keys.current()? else {
                    anyhow::bail!("mesh/share_key is gone");
                };
                let removed = parse_removed(&row.removed);
                for e in pending {
                    if let Err(err) = self.edges.revoke(e, &k, &removed).await {
                        return self.fail(row, now, &format!("revoke at {}", e.id), err).await;
                    }
                    row.edges_done.push(e.id.clone());
                    self.store.put_rekey(row).await?;
                }
                row.enter(RekeyState::WaitingVault, now);
                self.save(row).await
            }
            RekeyState::WaitingVault => {
                let members = self.keys.vault_members()?;
                if row.removed.iter().any(|e| members.contains(e)) {
                    return Ok(Step::Wait);
                }
                row.enter(RekeyState::Writing, now);
                self.save(row).await
            }
            RekeyState::Writing => {
                let Some((v, k)) = self.keys.current()? else {
                    anyhow::bail!("mesh/share_key is gone");
                };
                // A crash after the write: the stash's `old` names `K`; a vault key that is no
                // longer `K` means the write happened (ours or another holder's).
                let old = match self.stash.get(&row.id, "old")? {
                    Some(old) => old,
                    None => {
                        self.stash.put(&row.id, "old", &k)?;
                        k.clone()
                    }
                };
                row.from_pub = Some(old.public());
                if k.public() == old.public() {
                    let fresh = fresh_secret();
                    self.stash.put(&row.id, "pending", &fresh)?;
                    if self.keys.replace(&fresh, v).await?.is_none() {
                        self.stash.drop_role(&row.id, "pending")?;
                    }
                }
                row.enter(RekeyState::Settling, now);
                self.save(row).await
            }
            RekeyState::Settling => {
                if !self.keys.synced() || now < row.updated_ms + self.settle_ms {
                    return Ok(Step::Wait);
                }
                let Some((_, s)) = self.keys.current()? else {
                    anyhow::bail!("mesh/share_key is gone");
                };
                row.to_pub = Some(s.public());
                row.enter(RekeyState::Sending, now);
                self.save(row).await
            }
            RekeyState::Sending => self.sending(row, pending, now).await,
            RekeyState::Confirming | RekeyState::Contested => self.confirming(row, pending, now).await,
            RekeyState::Done => Ok(Step::Wait),
        }
    }

    /// The settled key `S` (must still be the vault's key; else settle again).
    fn settled(&self, row: &mut RekeyRow, now: u64) -> anyhow::Result<Option<SecretKey>> {
        let Some((_, s)) = self.keys.current()? else {
            anyhow::bail!("mesh/share_key is gone");
        };
        if Some(s.public()) != row.to_pub {
            row.enter(RekeyState::Settling, now);
            row.updated_ms = now;
            return Ok(None);
        }
        Ok(Some(s))
    }

    async fn sending(&self, row: &mut RekeyRow, pending: Vec<&EdgeRow>, now: u64) -> anyhow::Result<Step> {
        let Some(s) = self.settled(row, now)? else {
            return self.save(row).await;
        };
        let old = match self.stash.get(&row.id, "old")? {
            Some(k) => k,
            None => match row.from_pub.and_then(|p| self.stash.find(&p).transpose()) {
                Some(k) => k?,
                // A new holder resuming another engine's row has no `K`: the edge must answer
                // with its current id, which it can only confirm or chain from its own stash.
                None => s.clone(),
            },
        };
        let stash_pubs: Vec<PublicKey> = self.stash.secrets()?.iter().map(|k| k.public()).collect();
        let revoke = parse_removed(&row.removed);
        for e in pending {
            let body = Rekey::new(&e.domain, e.edge, old.public(), s.public(), revoke.clone(), now);
            let answer = match body.sign(&old, &s) {
                Ok(signed) => self.edges.rekey(e, &old, &signed).await,
                Err(err) => Err(anyhow::anyhow!("{err}")),
            };
            match answer {
                Ok(RekeyAnswer::Accepted) => {}
                Ok(RekeyAnswer::Rekeyed(to)) => match rekey_conflict(&to, &s.public(), &stash_pubs) {
                    RekeyConflict::Confirming => {}
                    RekeyConflict::Chain { from } => {
                        if let Err(err) = self.chain(e, &body, from, &s).await {
                            return self.fail(row, now, &format!("chain at {}", e.id), err).await;
                        }
                    }
                    RekeyConflict::Contested => {
                        tracing::warn!("share: rekey {} contested at {}: edge holds {to}", row.id, e.id);
                        row.enter(RekeyState::Contested, now);
                        return self.save(row).await;
                    }
                },
                Err(err) => return self.fail(row, now, &format!("rekey at {}", e.id), err).await,
            }
            row.edges_done.push(e.id.clone());
            self.store.put_rekey(row).await?;
        }
        row.enter(RekeyState::Confirming, now);
        self.save(row).await
    }

    async fn chain(&self, e: &EdgeRow, body: &Rekey, from: PublicKey, s: &SecretKey) -> anyhow::Result<()> {
        let from_secret = self.stash.find(&from)?.context("chain: secret left the stash")?;
        let signed = body
            .chain(from)
            .sign(&from_secret, s)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        match self.edges.rekey(e, &from_secret, &signed).await? {
            RekeyAnswer::Accepted => Ok(()),
            RekeyAnswer::Rekeyed(to) if to == s.public() => Ok(()),
            RekeyAnswer::Rekeyed(to) => anyhow::bail!("edge moved on to {to}"),
        }
    }

    /// The edge's current mesh id, asked under each `probes` key in turn: the key the edge holds
    /// answers its id, a key it moved past answers `mesh_rekeyed {to}`; a key it never knew
    /// fails (a real edge would not reveal anything for it). `None`: no probe got an answer.
    async fn edge_current(&self, e: &EdgeRow, probes: &[SecretKey]) -> Option<PublicKey> {
        for k in probes {
            match self.edges.mesh_id(e, k).await {
                Ok(Ok(id)) | Ok(Err(id)) => return Some(id),
                Err(err) => tracing::debug!("share: mesh probe at {}: {err:#}", e.id),
            }
        }
        None
    }

    /// Probe keys, oldest knowledge first: the stash (the old key moved past answers `to`), then
    /// `S` itself.
    fn probes(&self, s: &SecretKey) -> anyhow::Result<Vec<SecretKey>> {
        let mut p = self.stash.secrets()?;
        p.retain(|k| k.public() != s.public());
        p.push(s.clone());
        Ok(p)
    }

    /// Every engine any rekey row names: the revoke list a chained or healing `Rekey` carries
    /// (the edge *replaces* its revoke list with it).
    async fn revoke_all(&self) -> anyhow::Result<Vec<PublicKey>> {
        let mut all: Vec<String> = self
            .store
            .rekeys()
            .await?
            .into_iter()
            .flat_map(|r| r.removed)
            .collect();
        all.sort();
        all.dedup();
        Ok(parse_removed(&all))
    }

    async fn confirming(&self, row: &mut RekeyRow, pending: Vec<&EdgeRow>, now: u64) -> anyhow::Result<Step> {
        let Some(s) = self.settled(row, now)? else {
            return self.save(row).await;
        };
        let probes = self.probes(&s)?;
        for e in pending {
            let Some(id) = self.edge_current(e, &probes).await else {
                return self
                    .fail(row, now, &format!("confirm at {}", e.id), anyhow::anyhow!("no mesh answer"))
                    .await;
            };
            if Some(id) == row.from_pub && row.state == RekeyState::Confirming {
                // The edge never applied it (another holder's row said done): send again.
                row.enter(RekeyState::Sending, now);
                return self.save(row).await;
            }
            if id != s.public() {
                // Heal rule: our stash holds the edge's current secret → chain it on.
                if self.stash.find(&id)?.is_some() {
                    let body = Rekey::new(&e.domain, e.edge, id, s.public(), self.revoke_all().await?, now);
                    if let Err(err) = self.chain(e, &body, id, &s).await {
                        return self.fail(row, now, &format!("heal at {}", e.id), err).await;
                    }
                } else {
                    // Another engine must heal; a contested row surfaces after the heal time.
                    row.backoff(now);
                    self.store.put_rekey(row).await?;
                    return Ok(Step::Wait);
                }
            }
            row.edges_done.push(e.id.clone());
            self.store.put_rekey(row).await?;
        }
        row.enter(RekeyState::Done, now);
        self.store.put_rekey(row).await?;
        Ok(Step::Done { mesh: s.public() })
    }

    /// The heal rule for any engine, holder or not (§5.7): when this engine's stash holds the
    /// secret of an edge's current mesh id while the vault's settled key differs, send the
    /// chained `Rekey` itself. Run on every sync (peer-set merge) while the stash is not empty.
    /// Returns the edges healed.
    pub async fn heal(&self, now: u64) -> anyhow::Result<Vec<String>> {
        if self.stash.is_empty()? || !self.keys.synced() {
            return Ok(Vec::new());
        }
        let Some((_, s)) = self.keys.current()? else {
            return Ok(Vec::new());
        };
        let probes = self.probes(&s)?;
        let mut healed = Vec::new();
        for e in self.store.share_edges().await? {
            let Some(id) = self.edge_current(&e, &probes).await else {
                continue;
            };
            if id != s.public() && self.stash.find(&id)?.is_some() {
                let body = Rekey::new(&e.domain, e.edge, id, s.public(), self.revoke_all().await?, now);
                self.chain(&e, &body, id, &s).await?;
                healed.push(e.id.clone());
            }
        }
        Ok(healed)
    }

    /// Drop the stash of rows that are `done` since `heal_ms`: until then a secret this engine
    /// wrote in a partition may still be what an edge holds, and only this engine can heal it.
    pub async fn prune(&self, now: u64) -> anyhow::Result<()> {
        for r in self.store.rekeys().await? {
            if r.state == RekeyState::Done && now >= r.updated_ms + self.heal_ms {
                self.stash.drop_rekey(&r.id)?;
            }
        }
        Ok(())
    }
}

/// Map an edge error to a rekey answer: `409 mesh_rekeyed {to}` → [`RekeyAnswer::Rekeyed`].
pub fn rekey_answer(
    r: Result<share_proto::RekeyAccepted, crate::edge::EdgeError>,
) -> anyhow::Result<RekeyAnswer> {
    match r {
        Ok(_) => Ok(RekeyAnswer::Accepted),
        Err(crate::edge::EdgeError::Http { body, .. })
            if body.code == ErrorCode::MeshRekeyed && body.to.is_some() =>
        {
            Ok(RekeyAnswer::Rekeyed(body.to.expect("checked")))
        }
        Err(e) => Err(e.into()),
    }
}

/// Removed engines this engine has not run duties for, from the mesh's `RemovedPeers` view
/// (MESH1 doubt: a tombstone can arrive while this engine was not running). `known` are the
/// engines named by any existing row.
pub fn unhandled_removals(removed: &[String], rows: &[RekeyRow]) -> Vec<String> {
    let known: BTreeMap<&str, ()> = rows
        .iter()
        .flat_map(|r| r.removed.iter().map(|e| (e.as_str(), ())))
        .collect();
    removed
        .iter()
        .filter(|e| !known.contains_key(e.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests;
