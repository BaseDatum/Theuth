//! 210 §5.2 (D20, D21): the mesh's names as the replicated `share_names` table records them,
//! and the owner's name operations: reserve (claim), release, primary, rotate, transfer.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use share_proto::{NameKind, PublicKey};
use sqlx::Row as _;

use crate::engine::{Consent, Engine};
use crate::model::{ShareState, StoppedBy};
use crate::store::Store;

/// `share_names_list` item: a name of the mesh, the replicated `share_names` row joined with
/// the edge's `NameInfo` from the last `welcome` (§5.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HeldName {
    /// The name.
    pub name: String,
    /// The edge's domain.
    pub domain: String,
    /// The edge's endpoint id.
    pub edge: Option<String>,
    /// `reserved` | `random`.
    pub kind: String,
    /// New shares use it (`share_names.is_primary`).
    pub primary: bool,
    /// The edge's state: `allocated` · `queued` · `slotted` · `valid` · `released` · `banned`;
    /// `None` when this machine has not seen the edge's view of it.
    pub state: Option<String>,
    /// Slot queue position while `queued`.
    pub queue_position: Option<u32>,
    /// Seconds until the slot.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub queue_eta_s: Option<u64>,
    /// Pending transfer (D20): the receiving mesh id.
    pub transfer_to: Option<String>,
    /// End of the transfer offer.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub transfer_until_ms: Option<u64>,
}

/// One `share_names` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareName {
    /// `"<edge>/<domain>/<name>"`.
    pub id: String,
    /// Edge endpoint id.
    pub edge: String,
    /// Edge domain.
    pub domain: String,
    /// The name.
    pub name: String,
    /// `random` | `reserved`.
    pub kind: String,
    /// New shares use it.
    pub is_primary: bool,
    /// Released at.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub released_ms: Option<u64>,
}

impl Store {
    /// Every name row (released ones included).
    pub async fn names(&self) -> anyhow::Result<Vec<ShareName>> {
        let rows = sqlx::query("SELECT * FROM share_names ORDER BY id")
            .fetch_all(self.pool())
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| ShareName {
                id: r.get("id"),
                edge: r.get("edge"),
                domain: r.get("domain"),
                name: r.get("name"),
                kind: r.get("kind"),
                is_primary: r.get::<i64, _>("is_primary") != 0,
                released_ms: r.get::<Option<i64>, _>("released_ms").map(|v| v as u64),
            })
            .collect())
    }

    /// Insert or revive a held name.
    pub async fn put_name(&self, edge: &str, domain: &str, name: &str, kind: NameKind) -> anyhow::Result<String> {
        let id = format!("{edge}/{domain}/{name}");
        let kind = match kind {
            NameKind::Reserved => "reserved",
            _ => "random",
        };
        sqlx::query(
            "INSERT INTO share_names (id, edge, domain, name, kind, is_primary, released_ms)
             VALUES (?, ?, ?, ?, ?, 0, NULL)
             ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, released_ms = NULL",
        )
        .bind(&id)
        .bind(edge)
        .bind(domain)
        .bind(name)
        .bind(kind)
        .execute(self.pool())
        .await?;
        Ok(id)
    }

    /// Make `id` the only primary name at its edge.
    pub async fn set_primary(&self, id: &str) -> anyhow::Result<()> {
        let mut tx = self.pool().begin().await?;
        let edge: Option<(String, String)> =
            sqlx::query_as("SELECT edge, domain FROM share_names WHERE id = ? AND released_ms IS NULL")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let (edge, domain) = edge.ok_or_else(|| anyhow::anyhow!("the mesh holds no name {id}"))?;
        sqlx::query("UPDATE share_names SET is_primary = 0 WHERE edge = ? AND domain = ? AND is_primary = 1 AND id <> ?")
            .bind(&edge)
            .bind(&domain)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE share_names SET is_primary = 1 WHERE id = ? AND is_primary = 0")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Mark a name released; a released primary passes primary to the newest held reserved name,
    /// else the random one.
    pub async fn release_name(&self, id: &str, now_ms: u64) -> anyhow::Result<()> {
        sqlx::query("UPDATE share_names SET released_ms = ?, is_primary = 0 WHERE id = ?")
            .bind(now_ms as i64)
            .bind(id)
            .execute(self.pool())
            .await?;
        let rows = self.names().await?;
        let Some(gone) = rows.iter().find(|n| n.id == id) else {
            return Ok(());
        };
        let held: Vec<&ShareName> = rows
            .iter()
            .filter(|n| n.released_ms.is_none() && n.edge == gone.edge && n.domain == gone.domain)
            .collect();
        if !held.iter().any(|n| n.is_primary)
            && let Some(next) = held
                .iter()
                .rev()
                .find(|n| n.kind == "reserved")
                .or_else(|| held.first())
        {
            self.set_primary(&next.id).await?;
        }
        Ok(())
    }
}

/// `share_names_reserve`: claim a reserved name (`PUT /v1/names/{name}`); the first reserved
/// name becomes primary (§5.2). TERMS-FIRST: the edge's terms are checked before `consent`
/// (the owner's press, or the approval row it raises) is taken, so an unaccepted revision
/// answers the terms refusal and never raises a card the edge would refuse anyway.
pub async fn reserve<F, Fut>(engine: &Engine, name: &str, consent: F) -> anyhow::Result<HeldName>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<Consent>>,
{
    share_proto::names::check_reserved::<&str>(name, &[]).map_err(|e| anyhow::anyhow!(e.reason()))?;
    engine.terms_precheck().await?;
    consent().await?;
    engine.terms_ready().await?;
    let (granted, info) = engine
        .with_control(|c, info| async move { Ok((c.claim(name).await?, info)) })
        .await?;
    let edge = info.edge_id.to_string();
    let id = engine
        .store
        .put_name(&edge, &info.domain, &granted.name, NameKind::Reserved)
        .await?;
    engine.store.set_primary(&id).await?;
    refresh(engine).await;
    held(engine, &granted.name).await
}

/// After a claim or release: the link's welcome lists the names as the edge now holds them.
/// The `share_names` row already records the change, so a failed refresh only leaves the
/// edge's state of the name unknown until the next `hello`.
async fn refresh(engine: &Engine) {
    if let Err(e) = engine.refresh_welcome().await {
        tracing::warn!("share: welcome refresh after a name change: {e:#}");
    }
}

/// `share_names_release`: stop the name's shares on every machine, then `DELETE` it.
pub async fn release(engine: &Engine, name: &str) -> anyhow::Result<share_proto::NameReleased> {
    let (_, info) = engine.attached_control().await?;
    let suffix = format!("{name}.{}", info.domain);
    for s in engine.store.list(platform::time::now_ms()).await? {
        if s.state != ShareState::Stopped && (s.host == suffix || s.host.ends_with(&format!(".{suffix}"))) {
            engine.stop(&s.id, StoppedBy::User).await?;
        }
    }
    // Redials once when the edge closed the tunnel under us (after Stop all, E2E1 finding 4).
    let r = engine
        .with_control(|c, _| async move { c.release(name).await })
        .await?;
    let id = format!("{}/{}/{name}", info.edge_id, info.domain);
    engine.store.release_name(&id, platform::time::now_ms()).await?;
    refresh(engine).await;
    Ok(r)
}

/// `share_names_primary`.
pub async fn primary(engine: &Engine, name: &str) -> anyhow::Result<HeldName> {
    let rows = engine.store.names().await?;
    let id = match rows.iter().find(|n| n.name == name && n.released_ms.is_none()) {
        Some(r) => r.id.clone(),
        // Held at the edge (welcome) without a row yet: record it first.
        None => {
            let h = held(engine, name).await?;
            let kind = if h.kind == "reserved" { NameKind::Reserved } else { NameKind::Random };
            let edge = h.edge.clone().unwrap_or_default();
            engine.store.put_name(&edge, &h.domain, name, kind).await?
        }
    };
    engine.store.set_primary(&id).await?;
    held(engine, name).await
}

/// `share_names_transfer` (D20): `POST /v1/names/{name}/transfer {to_mesh}`.
pub async fn transfer(engine: &Engine, name: &str, to_mesh: &str) -> anyhow::Result<u64> {
    let to = parse_mesh(to_mesh)?;
    engine
        .with_control(|c, _| async move { Ok(c.transfer(name, to).await?.transfer_until_ms) })
        .await
}

/// `share_names_rotate` (§6.3): only the lease holder rotates (a non-holder forwards the
/// whole call to it): a new name key of the next generation replaces the vault row, the row
/// settles, and its CSR is posted with `?rotate=true`. Returns the generation now in the row.
pub async fn rotate(engine: &Engine, name: &str) -> anyhow::Result<u32> {
    match crate::family::holder().await? {
        crate::family::Holder::Peer(h) => {
            crate::family::call(&h, "share_names_rotate", serde_json::json!({ "name": name })).await
        }
        crate::family::Holder::Me => {
            // The CSR post needs the edge's terms: refuse before a new key replaces the row.
            engine.terms_ready().await?;
            let (_, info) = engine.attached_control().await?;
            let key = crate::wiring::rotate_name_key_as_holder(&info.domain, name).await?;
            engine.rotate_cert(name, &key).await?;
            Ok(key.r#gen)
        }
    }
}

/// `share_names_list`: every row joined with the edge's view; names the edge reports that have
/// no row yet (an M1 random name) are listed too, the only one primary when none is.
pub async fn list(engine: &Engine) -> anyhow::Result<Vec<HeldName>> {
    let rows = engine.store.names().await?;
    let seen = engine.welcome_names();
    let info_of = |edge: &str, domain: &str, name: &str| {
        seen.as_ref().and_then(|(e, d, names)| {
            (e == edge && d == domain)
                .then(|| names.iter().find(|n| n.name == name).cloned())
                .flatten()
        })
    };
    let state = |i: &share_proto::NameInfo| {
        serde_json::to_value(i.state)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
    };
    let mut out: Vec<HeldName> = rows
        .iter()
        .map(|r| {
            let i = info_of(&r.edge, &r.domain, &r.name);
            HeldName {
                name: r.name.clone(),
                domain: r.domain.clone(),
                edge: Some(r.edge.clone()),
                kind: r.kind.clone(),
                primary: r.is_primary,
                state: if r.released_ms.is_some() {
                    Some("released".into())
                } else {
                    i.as_ref().and_then(state)
                },
                queue_position: i.as_ref().and_then(|i| i.queue.map(|q| q.position)),
                queue_eta_s: i.as_ref().and_then(|i| i.queue.map(|q| q.eta_s)),
                transfer_to: i.as_ref().and_then(|i| i.transfer_to.map(|k| k.to_string())),
                transfer_until_ms: i.as_ref().and_then(|i| i.transfer_until_ms),
            }
        })
        .collect();
    if let Some((edge, domain, names)) = &seen {
        let no_primary = !out.iter().any(|h| h.primary && h.domain == *domain);
        for (k, i) in names
            .iter()
            .filter(|i| !rows.iter().any(|r| r.name == i.name && r.domain == *domain))
            .enumerate()
        {
            out.push(HeldName {
                name: i.name.clone(),
                domain: domain.clone(),
                edge: Some(edge.clone()),
                kind: if i.kind == NameKind::Reserved { "reserved" } else { "random" }.into(),
                primary: no_primary && k == 0,
                state: state(i),
                queue_position: i.queue.map(|q| q.position),
                queue_eta_s: i.queue.map(|q| q.eta_s),
                transfer_to: i.transfer_to.map(|k| k.to_string()),
                transfer_until_ms: i.transfer_until_ms,
            });
        }
    }
    Ok(out)
}

/// The listed entry for `name`.
pub async fn held(engine: &Engine, name: &str) -> anyhow::Result<HeldName> {
    list(engine)
        .await?
        .into_iter()
        .find(|h| h.name == name)
        .ok_or_else(|| anyhow::anyhow!("the mesh holds no name {name}"))
}

/// A pasted transfer target: a mesh id (the receiving mesh's Settings shows it), bare or
/// inside surrounding text/URL (the first token that parses as an endpoint id wins).
pub fn parse_mesh(s: &str) -> anyhow::Result<PublicKey> {
    let s = s.trim();
    if let Ok(k) = s.parse() {
        return Ok(k);
    }
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .find_map(|t| t.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("no mesh id in {s:?}: paste the receiving mesh's id from its Settings › Sharing"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn primary_moves_to_the_newest_reserved_on_release() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("share.sqlite")).await.unwrap();
        let r = s.put_name("e", "tun.zip", "abcdef", NameKind::Random).await.unwrap();
        s.set_primary(&r).await.unwrap();
        let sky = s.put_name("e", "tun.zip", "sky", NameKind::Reserved).await.unwrap();
        s.set_primary(&sky).await.unwrap();
        let p: Vec<_> = s.names().await.unwrap().into_iter().filter(|n| n.is_primary).collect();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].name, "sky");
        s.release_name(&sky, 5).await.unwrap();
        let rows = s.names().await.unwrap();
        assert_eq!(rows.iter().find(|n| n.is_primary).unwrap().name, "abcdef");
        assert_eq!(rows.iter().find(|n| n.name == "sky").unwrap().released_ms, Some(5));
    }
}
